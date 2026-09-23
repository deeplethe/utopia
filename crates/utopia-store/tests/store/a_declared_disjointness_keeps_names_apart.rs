//! 0016 B3：`owl:disjointWith` 进消解——本体声明了互斥的两个类，同名也不进审阅队列。
//!
//! 消解判「两个类能不能指同一个东西」有三层：硬表 `CONFUSABLE_TYPE_KEYS`、类层级
//! （同一支系当易混，#226）、本体声明的互斥。这里守的是**声明优先于前两层**：
//!
//! 1. 没声明时行为不变：organization vs project 照硬表进队列，corporation vs
//!    federal_agency 照类层级（共有非根祖先 organization）进队列。
//! 2. 声明 organization ⟂ project 之后，同名的 organization / project 分开，不进队列。
//! 3. 声明 corporation ⟂ agency 之后，federal_agency（agency 的子类）跟 corporation
//!    也分开——**互斥是继承的**，声明在父类上就够。
//!
//! 没有 `UTOPIA_DATABASE_URL` 时跳过而不是失败。自建自拆，绝不碰已有的库。

use sqlx::PgPool;
use utopia_store::{ontology, resolution};
use uuid::Uuid;

struct Fx {
    org: Uuid,
    kb: Uuid,
    organization: Uuid,
    project: Uuid,
    corporation: Uuid,
    agency: Uuid,
    federal_agency: Uuid,
}

async fn seed(pool: &PgPool) -> anyhow::Result<Fx> {
    let (org, ws, kb) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    // 顶上要有一个根：类层级那条规则里「共有的祖先」不算根（schema.org 里万物皆
    // Thing，算上它 Person 与 Organization 也成了一家），所以 organization 得有父类
    let (thing, organization, project, corporation, agency, federal_agency) = (
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
    );
    sqlx::query("INSERT INTO organizations (id, name) VALUES ($1, 'disjoint-test')")
        .bind(org)
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO workspaces (id, org_id, name) VALUES ($1, $2, 'disjoint-test')")
        .bind(ws)
        .bind(org)
        .execute(pool)
        .await?;
    sqlx::query(
        "INSERT INTO knowledge_bases (id, workspace_id, name) VALUES ($1, $2, 'disjoint-test')",
    )
    .bind(kb)
    .bind(ws)
    .execute(pool)
    .await?;
    for (id, key) in [
        (thing, "thing"),
        (organization, "organization"),
        (project, "project"),
        (corporation, "corporation"),
        (agency, "agency"),
        (federal_agency, "federal_agency"),
    ] {
        sqlx::query("INSERT INTO entity_types (id, kb_id, key, label) VALUES ($1, $2, $3, $3)")
            .bind(id)
            .bind(kb)
            .bind(key)
            .execute(pool)
            .await?;
    }
    for (child, parent) in [
        (organization, thing),
        (project, thing),
        (corporation, organization),
        (agency, organization),
        (federal_agency, agency),
    ] {
        sqlx::query("INSERT INTO entity_type_parents (child_id, parent_id) VALUES ($1, $2)")
            .bind(child)
            .bind(parent)
            .execute(pool)
            .await?;
    }
    Ok(Fx {
        org,
        kb,
        organization,
        project,
        corporation,
        agency,
        federal_agency,
    })
}

async fn entity(pool: &PgPool, f: &Fx, name: &str, type_id: Uuid) -> anyhow::Result<Uuid> {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO entities (id, kb_id, type_id, canonical_name) VALUES ($1, $2, $3, $4)",
    )
    .bind(id)
    .bind(f.kb)
    .bind(type_id)
    .bind(name)
    .execute(pool)
    .await?;
    Ok(id)
}

/// 消解一条 mention，回它挂上的「类型漂移」审核对指向谁
async fn drift_reviews(
    pool: &PgPool,
    f: &Fx,
    name: &str,
    type_id: Uuid,
) -> anyhow::Result<Vec<Uuid>> {
    let r = resolution::resolve_mention(pool, f.kb, Some(type_id), name, None, None, &[]).await?;
    assert!(
        r.created,
        "a cross-type same name is a new entity: keep apart, never merge"
    );
    Ok(r.reviews
        .iter()
        .filter(|x| x.reason.starts_with("type_drift|"))
        .map(|x| x.other_id)
        .collect())
}

#[tokio::test]
async fn a_declared_disjointness_keeps_names_apart() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let f = seed(&pool).await?;

    let run = async {
        // 1. 没声明：硬表与类层级照旧
        let orion = entity(&pool, &f, "Orion", f.organization).await?;
        assert_eq!(
            drift_reviews(&pool, &f, "Orion", f.project).await?,
            vec![orion],
            "organization vs project is confusable by the hard-coded list"
        );
        let acme = entity(&pool, &f, "Acme", f.corporation).await?;
        assert_eq!(
            drift_reviews(&pool, &f, "Acme", f.federal_agency).await?,
            vec![acme],
            "corporation vs federal_agency share the ancestor organization: kin, so Review"
        );

        // 2. 声明 organization ⟂ project：硬表说易混，本体说互斥——本体赢
        ontology::set_disjoint_for(&pool, f.kb, f.organization, &[f.project]).await?;
        let _vega = entity(&pool, &f, "Vega", f.organization).await?;
        assert!(
            drift_reviews(&pool, &f, "Vega", f.project)
                .await?
                .is_empty(),
            "a declared disjointness wins over the hard-coded list"
        );

        // 3. 声明 corporation ⟂ agency：federal_agency 是 agency 的子类，互斥继承下来，
        //    类层级说一家也不算
        ontology::set_disjoint_for(&pool, f.kb, f.corporation, &[f.agency]).await?;
        let _beta = entity(&pool, &f, "Beta", f.corporation).await?;
        assert!(
            drift_reviews(&pool, &f, "Beta", f.federal_agency)
                .await?
                .is_empty(),
            "a disjointness declared on the parent reaches the child and wins over kinship"
        );
        // 反过来问也一样：表里两个方向各一行，继承沿另一头的祖先链走
        let _gamma = entity(&pool, &f, "Gamma", f.federal_agency).await?;
        assert!(
            drift_reviews(&pool, &f, "Gamma", f.corporation)
                .await?
                .is_empty(),
            "the declaration holds from either side"
        );

        // 4. 取消声明，回到没声明时的行为——编辑必须能撤
        ontology::set_disjoint_for(&pool, f.kb, f.corporation, &[]).await?;
        let delta = entity(&pool, &f, "Delta", f.corporation).await?;
        assert_eq!(
            drift_reviews(&pool, &f, "Delta", f.federal_agency).await?,
            vec![delta],
            "with the declaration gone, kinship sends the pair to Review again"
        );
        Ok::<_, anyhow::Error>(())
    }
    .await;

    let _ = sqlx::query("DELETE FROM organizations WHERE id = $1")
        .bind(f.org)
        .execute(&pool)
        .await;
    run
}
