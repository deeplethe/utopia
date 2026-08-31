//! 数据源授权（0014）。
//!
//! 补的洞:注册是部署级动作,而挂载的守卫是 `require_kb(kb_id, Role::Admin)`
//! ——请求者自己那个库的管理员。可挂载列表返回的又是全部署每一个源。于是任何
//! 一个知识库的管理员,都能把任意生产库挂进自己库,而挂上之后该库每个 Viewer
//! 都能通过 `query_data` 对它跑只读 SQL。
//!
//! 这里钉三件事:
//!
//! - **看不见**:没授权的源不进可挂载列表
//! - **也挂不上**:列表过滤只挡「看得见」,而挂载端点是照着 id 调的——
//!   守卫必须在两侧都有,这条测的是端点那一侧
//! - **收回是真收回**:撤授权连同已挂上的一起卸掉。只删授权行的话,问数读的
//!   还是 `kb_data_sources`,等于撤销不生效——一个不生效的权限撤销比没有还危险

use sqlx::PgPool;
use utopia_store::datasources;
use uuid::Uuid;

struct Fixture {
    org: Uuid,
    /// 授权给它的工作区
    ours: Uuid,
    /// 没授权的工作区——它的库不该够得着这个源
    theirs: Uuid,
    our_kb: Uuid,
    their_kb: Uuid,
    source: Uuid,
    actor: Uuid,
}

async fn seed(pool: &PgPool) -> anyhow::Result<Fixture> {
    let org = Uuid::now_v7();
    let (ours, theirs) = (Uuid::now_v7(), Uuid::now_v7());
    let (our_kb, their_kb) = (Uuid::now_v7(), Uuid::now_v7());
    let (source, actor) = (Uuid::now_v7(), Uuid::now_v7());

    sqlx::query("INSERT INTO organizations (id, name) VALUES ($1, 'grant-test')")
        .bind(org)
        .execute(pool)
        .await?;
    for (id, name) in [(ours, "ours"), (theirs, "theirs")] {
        sqlx::query("INSERT INTO workspaces (id, org_id, name) VALUES ($1, $2, $3)")
            .bind(id)
            .bind(org)
            .bind(name)
            .execute(pool)
            .await?;
    }
    for (kb, ws, name) in [(our_kb, ours, "our-kb"), (their_kb, theirs, "their-kb")] {
        sqlx::query("INSERT INTO knowledge_bases (id, workspace_id, name) VALUES ($1, $2, $3)")
            .bind(kb)
            .bind(ws)
            .bind(name)
            .execute(pool)
            .await?;
    }
    sqlx::query(
        "INSERT INTO users (id, org_id, email, password_hash, display_name, is_admin)
         VALUES ($1, $2, $3, 'x', 'Grant Test', TRUE)",
    )
    .bind(actor)
    .bind(org)
    .bind(format!("{actor}@grant.test"))
    .execute(pool)
    .await?;
    sqlx::query(
        "INSERT INTO data_sources (id, name, engine, conn_string, created_by)
         VALUES ($1, $2, 'postgres', 'postgres://u:p@db.test:5432/w', $3)",
    )
    .bind(source)
    .bind(format!("warehouse-{source}"))
    .bind(actor)
    .execute(pool)
    .await?;

    Ok(Fixture {
        org,
        ours,
        theirs,
        our_kb,
        their_kb,
        source,
        actor,
    })
}

#[tokio::test]
async fn a_source_reaches_only_where_it_was_granted() -> anyhow::Result<()> {
    let Ok(url) = std::env::var("UTOPIA_DATABASE_URL") else {
        eprintln!("跳过：未设 UTOPIA_DATABASE_URL");
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let f = seed(&pool).await?;

    let run = async {
        // ---- 一、没授权 = 谁都够不着
        assert!(
            datasources::granted_to_workspace(&pool, f.ours)
                .await?
                .is_empty(),
            "没授权过，可挂载列表就该是空的"
        );
        assert!(
            !datasources::is_granted(&pool, f.our_kb, f.source).await?,
            "没授权，挂载端点的守卫必须说不"
        );

        // ---- 二、授权一个工作区，另一个不受影响
        datasources::grant(&pool, f.source, f.ours, f.actor).await?;
        let ours = datasources::granted_to_workspace(&pool, f.ours).await?;
        assert_eq!(ours.len(), 1, "授权过的工作区看得见它");
        assert!(
            !ours[0].summary.contains("p@"),
            "列表里只能有 host:port/db 摘要，凭据不出服务端"
        );
        assert!(
            datasources::granted_to_workspace(&pool, f.theirs)
                .await?
                .is_empty(),
            "**授权是逐工作区的**：给了一个不等于给了全部署"
        );

        // ---- 三、端点侧的守卫：列表过滤只挡「看得见」
        assert!(datasources::is_granted(&pool, f.our_kb, f.source).await?);
        assert!(
            !datasources::is_granted(&pool, f.their_kb, f.source).await?,
            "没授权的工作区，就算自己拼一个 uuid 打过来也挂不上"
        );

        // ---- 四、一个源可授权给多个工作区（多对多，不是一对多）
        datasources::grant(&pool, f.source, f.theirs, f.actor).await?;
        assert_eq!(
            datasources::grants_for_source(&pool, f.source).await?.len(),
            2,
            "同一个数仓要能同时服务多个工作区"
        );
        datasources::grant(&pool, f.source, f.theirs, f.actor).await?;
        assert_eq!(
            datasources::grants_for_source(&pool, f.source).await?.len(),
            2,
            "重复授权是幂等的"
        );

        // ---- 五、收回连同挂载一起收
        datasources::mount(&pool, f.our_kb, f.source).await?;
        datasources::mount(&pool, f.their_kb, f.source).await?;
        let unmounted = datasources::revoke(&pool, f.source, f.theirs).await?;
        assert_eq!(unmounted, 1, "撤授权要把那个工作区里已挂上的一起卸掉");
        assert!(
            datasources::mounted(&pool, f.their_kb).await?.is_empty(),
            "**留着挂载 = 撤销不生效**：问数读的是 kb_data_sources"
        );
        assert_eq!(
            datasources::mounted(&pool, f.our_kb).await?.len(),
            1,
            "另一个工作区的挂载不受牵连"
        );
        Ok::<_, anyhow::Error>(())
    }
    .await;

    sqlx::query("DELETE FROM data_sources WHERE id = $1")
        .bind(f.source)
        .execute(&pool)
        .await?;
    sqlx::query("DELETE FROM organizations WHERE id = $1")
        .bind(f.org)
        .execute(&pool)
        .await?;
    run
}
