//! `inverseOf` / `subPropertyOf` 的落库与边界——打在真库上。
//!
//! **这里守的第一条是知识库隔离。** 列上的外键写的是
//! `REFERENCES relation_types(id)`，它不认 `kb_id`：数据库层面，A 库的关系
//! 完全可以指向 B 库的关系。RDF 导入那条路天然过不去（按 IRI 在本库里查），
//! 而 HTTP 接口收的是裸 UUID——挡不住的话，拿到任意一个 id 就能让推理机
//! 跨库读公理，推出来的边会带着另一个库的语义落进这个库。
//!
//! 前端只列本库的关系。**那是界面礼貌，不是边界**：接口自己收 UUID，
//! curl 一下就绕过去了。所以校验必须在 store 层，测试也必须在这里。
//!
//! 其余三条是同一族的形状约束：属性没有逆（宾语是字面值，无从谈起）、
//! 属性不能当别人的逆、子属性不能是自己（库里有 CHECK，但撞上去是 500，
//! 得在到达 CHECK 之前给出人话）。

use sqlx::PgPool;
use utopia_core::models::RelationAxioms;
use uuid::Uuid;

/// 一个 org 底下两个库。**两个是必需的**——这组测试的主角就是跨库那条线。
async fn two_kbs(pool: &PgPool) -> anyhow::Result<(Uuid, Uuid, Uuid)> {
    let (org, ws, a, b) = (
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
    );
    sqlx::query("INSERT INTO organizations (id, name) VALUES ($1, 'link-test')")
        .bind(org)
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO workspaces (id, org_id, name) VALUES ($1, $2, 'link-test')")
        .bind(ws)
        .bind(org)
        .execute(pool)
        .await?;
    for (id, name) in [(a, "link-a"), (b, "link-b")] {
        sqlx::query("INSERT INTO knowledge_bases (id, workspace_id, name) VALUES ($1, $2, $3)")
            .bind(id)
            .bind(ws)
            .bind(name)
            .execute(pool)
            .await?;
    }
    Ok((org, a, b))
}

/// 建一条最普通的关系，不带任何链。
async fn plain(pool: &PgPool, kb: Uuid, key: &str) -> anyhow::Result<Uuid> {
    Ok(utopia_store::ontology::create_relation_type(
        pool,
        kb,
        key,
        key,
        "state",
        RelationAxioms::default(),
        "",
        "relation",
        &[],
        &[],
        None,
        None,
    )
    .await?)
}

#[tokio::test]
async fn a_relation_points_only_inside_its_own_kb() -> anyhow::Result<()> {
    let Ok(url) = std::env::var("UTOPIA_DATABASE_URL") else {
        eprintln!("跳过：未设 UTOPIA_DATABASE_URL");
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let (org, kb_a, kb_b) = two_kbs(&pool).await?;

    let run = async {
        let works_at = plain(&pool, kb_a, "works_at").await?;
        let employs = plain(&pool, kb_a, "employs").await?;
        let ceo_of = plain(&pool, kb_a, "ceo_of").await?;
        // 另一个库里的关系。名字取一样的——**同名不同库，正是最容易被当成
        // 自己人的那种**
        let foreign = plain(&pool, kb_b, "employs").await?;

        // ---- 一、跨库指向：建的时候就该被拒
        let err = utopia_store::ontology::create_relation_type(
            &pool,
            kb_a,
            "leaks",
            "leaks",
            "state",
            RelationAxioms {
                inverse_of: Some(foreign),
                ..Default::default()
            },
            "",
            "relation",
            &[],
            &[],
            None,
            None,
        )
        .await
        .expect_err("指向别的知识库的关系必须被拒");
        assert!(
            format!("{err:?}").contains("unknown_relation"),
            "**拒绝的理由不能透露那个 id 存在于别处**——不区分「不存在」\
             与「在别的库」，实得 {err:?}"
        );
        let leaked: i64 =
            sqlx::query_scalar("SELECT count(*) FROM relation_types WHERE kb_id = $1 AND key = $2")
                .bind(kb_a)
                .bind("leaks")
                .fetch_one(&pool)
                .await?;
        assert_eq!(leaked, 0, "被拒的那次不该留下半行");

        // ---- 二、跨库指向：改的时候同样该被拒
        let err = utopia_store::ontology::update_relation_type(
            &pool,
            kb_a,
            works_at,
            "works at",
            "state",
            RelationAxioms {
                sub_property_of: Some(foreign),
                ..Default::default()
            },
            "",
            None,
            None,
            None,
            None,
        )
        .await
        .expect_err("改成指向别的知识库同样必须被拒");
        assert!(
            format!("{err:?}").contains("unknown_relation"),
            "实得 {err:?}"
        );

        // ---- 三、本库之内：写得进，也读得回
        utopia_store::ontology::update_relation_type(
            &pool,
            kb_a,
            works_at,
            "works at",
            "state",
            RelationAxioms {
                inverse_of: Some(employs),
                ..Default::default()
            },
            "",
            None,
            None,
            None,
            None,
        )
        .await?;
        utopia_store::ontology::update_relation_type(
            &pool,
            kb_a,
            ceo_of,
            "ceo of",
            "state",
            RelationAxioms {
                sub_property_of: Some(works_at),
                ..Default::default()
            },
            "",
            None,
            None,
            None,
            None,
        )
        .await?;
        let views = utopia_store::ontology::relation_type_views(&pool, kb_a).await?;
        let find = |id: Uuid| views.iter().find(|v| v.id == id).expect("落库");
        assert_eq!(
            find(works_at).inverse_of,
            Some(employs),
            "**视图必须回这两个值**——下拉框要显示当前选的是谁，\
             读不回来的话打开表单是空的，保存一次就把声明抹了"
        );
        assert_eq!(find(ceo_of).sub_property_of, Some(works_at));

        // ---- 四、缺省 = 清空，与上面六位公理同一条规矩
        utopia_store::ontology::update_relation_type(
            &pool,
            kb_a,
            works_at,
            "works at",
            "state",
            RelationAxioms::default(),
            "",
            None,
            None,
            None,
            None,
        )
        .await?;
        let views = utopia_store::ontology::relation_type_views(&pool, kb_a).await?;
        assert_eq!(
            views.iter().find(|v| v.id == works_at).unwrap().inverse_of,
            None,
            "不传 = 清空。一半覆盖一半保留，会让「我把逆去掉了」\
             和「我没碰逆」长得一模一样"
        );

        // ---- 五、属性：不能有链，也不能当链的目标
        let salary = utopia_store::ontology::create_relation_type(
            &pool,
            kb_a,
            "salary",
            "salary",
            "state",
            RelationAxioms::default(),
            "",
            "attribute",
            // 属性至少要挂一个类，这里借用一个现成的类
            &[class(&pool, kb_a).await?],
            &[],
            Some("number"),
            None,
        )
        .await?;
        let err = utopia_store::ontology::update_relation_type(
            &pool,
            kb_a,
            ceo_of,
            "ceo of",
            "state",
            RelationAxioms {
                inverse_of: Some(salary),
                ..Default::default()
            },
            "",
            None,
            None,
            None,
            None,
        )
        .await
        .expect_err("属性不能当逆——它的宾语是字面值，反过来指回来无从谈起");
        assert!(
            format!("{err:?}").contains("link_target_is_attr"),
            "实得 {err:?}"
        );
        let err = utopia_store::ontology::update_relation_type(
            &pool,
            kb_a,
            salary,
            "salary",
            "state",
            RelationAxioms {
                sub_property_of: Some(ceo_of),
                ..Default::default()
            },
            "",
            None,
            None,
            None,
            None,
        )
        .await
        .expect_err("属性自己也不能有父属性");
        assert!(
            format!("{err:?}").contains("attr_has_no_link"),
            "实得 {err:?}"
        );

        // ---- 六、子属性不能是自己。**库里有 CHECK，但那是 500**
        let err = utopia_store::ontology::update_relation_type(
            &pool,
            kb_a,
            ceo_of,
            "ceo of",
            "state",
            RelationAxioms {
                sub_property_of: Some(ceo_of),
                ..Default::default()
            },
            "",
            None,
            None,
            None,
            None,
        )
        .await
        .expect_err("自己不能是自己的父属性");
        assert!(
            format!("{err:?}").contains("sub_property_self"),
            "得在撞上 CHECK 之前给出人话，而不是让人看到一个 500，实得 {err:?}"
        );

        // 逆是自己**不拦**：那等于 symmetric，是合法声明。
        // R0 会提示改用 `symmetric` 更直白，但那是提示不是错误
        utopia_store::ontology::update_relation_type(
            &pool,
            kb_a,
            employs,
            "employs",
            "state",
            RelationAxioms {
                inverse_of: Some(employs),
                ..Default::default()
            },
            "",
            None,
            None,
            None,
            None,
        )
        .await?;

        Ok::<_, anyhow::Error>(())
    }
    .await;

    sqlx::query("DELETE FROM organizations WHERE id = $1")
        .bind(org)
        .execute(&pool)
        .await?;
    let _ = kb_b;
    run
}

/// 借一个类给属性当 domain——属性没有类就建不出来。
async fn class(pool: &PgPool, kb: Uuid) -> anyhow::Result<Uuid> {
    Ok(utopia_store::ontology::create_entity_type(
        pool,
        kb,
        "person",
        "Person",
        "#888888",
        "circle",
        &[],
        "",
    )
    .await?)
}
