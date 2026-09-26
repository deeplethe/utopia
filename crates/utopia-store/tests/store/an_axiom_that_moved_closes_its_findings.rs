//! 判据一动，靠着它的 open 违规就得当场对账（#564 / 0062）。
//!
//! `axiom_violations.status = 'open'` 的承诺是「已对账于**当前**本体」。
//! 从前 `update_relation_type` 只管写：放宽一条公理，上一轮检出的 open 行
//! 原样留着——下一次导出会把一个只在旧判据下成立的发现说成是现在的。
//! 这里钉的是同一条事务里做完的三件事：
//!
//! 1. **旧判据下的检出先记下来**——动笔之前库里还是旧本体
//! 2. **写完后按新本体重检出**——新的违规同一份提交里进来
//! 3. **只在旧判据下成立的 open 行收 `criterion_changed`**——不是人裁的
//!    （decided_by 空），是公理/签名换了；早就不成立的照旧删掉，与
//!    `reasoning::run` 同一条清陈规矩
//!
//! 没动判据的编辑（label、描述）不跑检测——那是对账不必付的价。

use sqlx::PgPool;
use utopia_core::models::RelationAxioms;
use utopia_store::{ontology, reasoning};
use uuid::Uuid;

struct Fixture {
    org: Uuid,
    kb: Uuid,
    thing: Uuid,
    person: Uuid,
    /// state + asymmetric
    reports_to: Uuid,
    /// state，带 domain=[person]
    works_for: Uuid,
}

const NONE: RelationAxioms = RelationAxioms {
    functional: false,
    inverse_functional: false,
    transitive: false,
    symmetric: false,
    asymmetric: false,
    irreflexive: false,
    inverse_of: None,
    sub_property_of: None,
};

fn ax() -> RelationAxioms {
    RelationAxioms {
        asymmetric: true,
        ..NONE
    }
}

async fn seed(pool: &PgPool) -> anyhow::Result<Fixture> {
    let (org, ws, kb, thing, person, reports_to, works_for) = (
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
    );
    sqlx::query("INSERT INTO organizations (id, name) VALUES ($1, 'moved-axiom')")
        .bind(org)
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO workspaces (id, org_id, name) VALUES ($1, $2, 'moved-axiom')")
        .bind(ws)
        .bind(org)
        .execute(pool)
        .await?;
    sqlx::query(
        "INSERT INTO knowledge_bases (id, workspace_id, name) VALUES ($1, $2, 'moved-axiom')",
    )
    .bind(kb)
    .bind(ws)
    .execute(pool)
    .await?;
    sqlx::query(
        "INSERT INTO entity_types (id, kb_id, key, label) VALUES
            ($1, $2, 'thing', 'Thing'), ($3, $2, 'person', 'Person')",
    )
    .bind(thing)
    .bind(kb)
    .bind(person)
    .execute(pool)
    .await?;
    sqlx::query(
        "INSERT INTO relation_types
             (id, kb_id, key, label, temporal, is_asymmetric)
         VALUES ($1, $2, 'reports_to', 'reportsTo', 'state', TRUE)",
    )
    .bind(reports_to)
    .bind(kb)
    .execute(pool)
    .await?;
    sqlx::query(
        "INSERT INTO relation_types (id, kb_id, key, label, temporal)
         VALUES ($1, $2, 'works_for', 'worksFor', 'state')",
    )
    .bind(works_for)
    .bind(kb)
    .execute(pool)
    .await?;
    sqlx::query(
        "INSERT INTO relation_type_domains (relation_type_id, entity_type_id)
         VALUES ($1, $2)",
    )
    .bind(works_for)
    .bind(person)
    .execute(pool)
    .await?;
    Ok(Fixture {
        org,
        kb,
        thing,
        person,
        reports_to,
        works_for,
    })
}

async fn entity(pool: &PgPool, f: &Fixture, ty: Uuid, name: &str) -> anyhow::Result<Uuid> {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO entities (id, kb_id, type_id, canonical_name) VALUES ($1, $2, $3, $4)",
    )
    .bind(id)
    .bind(f.kb)
    .bind(ty)
    .bind(name)
    .execute(pool)
    .await?;
    Ok(id)
}

async fn fact(pool: &PgPool, f: &Fixture, (s, p, o): (Uuid, Uuid, Uuid)) -> anyhow::Result<Uuid> {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO facts (id, kb_id, subject_id, predicate_id, object_id)
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(id)
    .bind(f.kb)
    .bind(s)
    .bind(p)
    .bind(o)
    .execute(pool)
    .await?;
    Ok(id)
}

/// 某一行的现状：(status, resolution, decided_at)
async fn row_state(
    pool: &PgPool,
    id: Uuid,
) -> anyhow::Result<Option<(String, Option<String>, bool)>> {
    Ok(sqlx::query_as::<_, (String, Option<String>, bool)>(
        "SELECT status, resolution, decided_at IS NOT NULL FROM axiom_violations WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?)
}

async fn open_count(pool: &PgPool, f: &Fixture) -> anyhow::Result<i64> {
    Ok(sqlx::query_scalar(
        "SELECT count(*) FROM axiom_violations WHERE kb_id = $1 AND status = 'open'",
    )
    .bind(f.kb)
    .fetch_one(pool)
    .await?)
}

async fn cleanup(pool: &PgPool, f: &Fixture) -> anyhow::Result<()> {
    sqlx::query("DELETE FROM organizations WHERE id = $1")
        .bind(f.org)
        .execute(pool)
        .await?;
    Ok(())
}

/// 放宽一条公理：open 行收 criterion_changed，不是删也不是继续开着；
/// 把公理改回去，同一行重开——`criterion_changed` 是破约家族的一员
#[tokio::test]
async fn relaxing_an_axiom_closes_its_finding_and_reinstating_reopens_it() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let f = seed(&pool).await?;

    let result = async {
        let (a, b) = (
            entity(&pool, &f, f.thing, "A").await?,
            entity(&pool, &f, f.thing, "B").await?,
        );
        fact(&pool, &f, (a, f.reports_to, b)).await?;
        fact(&pool, &f, (b, f.reports_to, a)).await?;
        let r = reasoning::run(&pool, f.kb).await?;
        anyhow::ensure!(r.found == 1, "expected one asymmetry finding, got {r:?}");
        let (id,): (Uuid,) =
            sqlx::query_as("SELECT id FROM axiom_violations WHERE kb_id = $1 AND status = 'open'")
                .bind(f.kb)
                .fetch_one(&pool)
                .await?;

        // 改判据：asymmetric 关掉。同一事务里这行该被收编成 criterion_changed
        ontology::update_relation_type(
            &pool,
            f.kb,
            f.reports_to,
            "reportsTo",
            "state",
            NONE,
            "",
            None,
            None,
            None,
            None,
        )
        .await?;
        anyhow::ensure!(
            open_count(&pool, &f).await? == 0,
            "open finding outlived its axiom"
        );
        let (status, resolution, decided) = row_state(&pool, id).await?.unwrap();
        anyhow::ensure!(
            status == "resolved" && resolution.as_deref() == Some("criterion_changed"),
            "finding was not closed as criterion_changed: {status}/{resolution:?}"
        );
        anyhow::ensure!(decided, "decided_at should mark when it stopped being open");

        // 改回来：同一处矛盾又成立，同一行重开——不是另插一行
        ontology::update_relation_type(
            &pool,
            f.kb,
            f.reports_to,
            "reportsTo",
            "state",
            ax(),
            "",
            None,
            None,
            None,
            None,
        )
        .await?;
        let (status, resolution, _) = row_state(&pool, id).await?.unwrap();
        anyhow::ensure!(
            status == "open" && resolution.is_none(),
            "finding should reopen under the restored axiom, got {status}/{resolution:?}"
        );
        anyhow::ensure!(open_count(&pool, &f).await? == 1);
        Ok(())
    }
    .await;
    let cleanup = cleanup(&pool, &f).await;
    result.and(cleanup)
}

/// domain 换了签名就换：原来开着的 signature 违规按新判据收场
#[tokio::test]
async fn a_domain_change_closes_the_signature_finding() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let f = seed(&pool).await?;

    let result = async {
        // works_for domain=person，而一个 thing 断言了它——signature 违规
        let (acme, alice) = (
            entity(&pool, &f, f.thing, "Acme").await?,
            entity(&pool, &f, f.thing, "Alice").await?,
        );
        fact(&pool, &f, (alice, f.works_for, acme)).await?;
        let r = reasoning::run(&pool, f.kb).await?;
        anyhow::ensure!(
            r.found == 1
                && reasoning::open_violations(&pool, f.kb, 10, 0).await?[0].kind == "signature",
            "expected one signature finding"
        );

        // domain 放宽到 thing：判据没了，这行收 criterion_changed
        ontology::update_relation_type(
            &pool,
            f.kb,
            f.works_for,
            "worksFor",
            "state",
            NONE,
            "",
            None,
            None,
            Some(&[f.thing]),
            None,
        )
        .await?;
        anyhow::ensure!(
            open_count(&pool, &f).await? == 0,
            "signature finding outlived its domain"
        );
        let (status, resolution, _) = sqlx::query_as::<_, (String, Option<String>, bool)>(
            "SELECT status, resolution, decided_at IS NOT NULL FROM axiom_violations
              WHERE kb_id = $1 AND kind = 'signature'",
        )
        .bind(f.kb)
        .fetch_one(&pool)
        .await?;
        anyhow::ensure!(
            status == "resolved" && resolution.as_deref() == Some("criterion_changed"),
            "signature finding was not closed as criterion_changed: {status}/{resolution:?}"
        );
        Ok(())
    }
    .await;
    let cleanup = cleanup(&pool, &f).await;
    result.and(cleanup)
}

/// 不动判据的编辑不跑对账：一条只在旧检测里才「成立过」的 open 行，
/// 换 label 不碰它，真改判据时按早就陈掉的处置——删掉，不是 criterion_changed
#[tokio::test]
async fn a_label_edit_leaves_the_ledger_alone() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let f = seed(&pool).await?;

    let result = async {
        let (a, b) = (
            entity(&pool, &f, f.thing, "A").await?,
            entity(&pool, &f, f.thing, "B").await?,
        );
        let f1 = fact(&pool, &f, (a, f.works_for, b)).await?;
        // 一行检出函数永远不会产出的 open 行（自环挂着 functional 键）：
        // 它早就陈了，下一次对账该删掉而不是收编
        let stray = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO axiom_violations (id, kb_id, kind, left_fact, right_fact)
             VALUES ($1, $2, 'functional', $3, $3)",
        )
        .bind(stray)
        .bind(f.kb)
        .bind(f1)
        .execute(&pool)
        .await?;

        // 只改 label：判据没动，不跑检测——陈行原样留着（检出是有代价的，
        // 不该为一次改名付）
        ontology::update_relation_type(
            &pool,
            f.kb,
            f.works_for,
            "employs",
            "state",
            NONE,
            "",
            None,
            None,
            None,
            None,
        )
        .await?;
        anyhow::ensure!(
            row_state(&pool, stray).await?.map(|r| r.0) == Some("open".into()),
            "a label-only edit must not touch violation state"
        );

        // 真改判据（domain 从 person 换成 thing）：这行不在旧基线里
        //（检出从没产出过它）——删掉，与 run() 的清陈同一条规矩
        ontology::update_relation_type(
            &pool,
            f.kb,
            f.works_for,
            "employs",
            "state",
            NONE,
            "",
            None,
            None,
            Some(&[f.thing]),
            None,
        )
        .await?;
        anyhow::ensure!(
            row_state(&pool, stray).await?.is_none(),
            "a stale row that never held under the old ontology should be deleted, not adjudicated"
        );
        Ok(())
    }
    .await;
    let cleanup = cleanup(&pool, &f).await;
    result.and(cleanup)
}

/// 父链也是判据：给类挂上父类后签名对上了，open 违规当场收编
#[tokio::test]
async fn a_new_parent_closes_the_signature_finding() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let f = seed(&pool).await?;

    let result = async {
        // works_for domain=person；thing ⊏ person——thing 断言它算违规
        let (acme, alice) = (
            entity(&pool, &f, f.thing, "Acme").await?,
            entity(&pool, &f, f.thing, "Alice").await?,
        );
        fact(&pool, &f, (alice, f.works_for, acme)).await?;
        reasoning::run(&pool, f.kb).await?;
        anyhow::ensure!(open_count(&pool, &f).await? == 1);

        // 给 thing 挂上 person 当父类：签名沿祖先链判，这就对上了
        ontology::update_entity_type(
            &pool,
            f.kb,
            f.thing,
            "Thing",
            None,
            "circle",
            &[f.person],
            "",
        )
        .await?;
        anyhow::ensure!(
            open_count(&pool, &f).await? == 0,
            "signature finding should close once the parent makes the signature hold"
        );
        let (status, resolution, _) = sqlx::query_as::<_, (String, Option<String>, bool)>(
            "SELECT status, resolution, decided_at IS NOT NULL FROM axiom_violations
              WHERE kb_id = $1 AND kind = 'signature'",
        )
        .bind(f.kb)
        .fetch_one(&pool)
        .await?;
        anyhow::ensure!(
            status == "resolved" && resolution.as_deref() == Some("criterion_changed"),
            "finding was not closed as criterion_changed: {status}/{resolution:?}"
        );
        Ok(())
    }
    .await;
    let cleanup = cleanup(&pool, &f).await;
    result.and(cleanup)
}

/// 删掉被别人的 `sub_property_of` 指着的关系，也是一次判据变动：
/// 0070 把那条外键建成 DEFERRABLE SET NULL——提交才落地，所以得在事务里
/// 先显式断链再对账。断链之后靠它派生的发现不该还在
#[tokio::test]
async fn deleting_a_linked_relation_reconciles_what_depended_on_it() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let f = seed(&pool).await?;

    let result = async {
        // manages 是 functional 的超属性：supervises ⊑ manages——零断言，
        // 只活在派生链上，所以它能被删
        let (manages, supervises) = (Uuid::now_v7(), Uuid::now_v7());
        sqlx::query(
            "INSERT INTO relation_types (id, kb_id, key, label, temporal, functional)
             VALUES ($1, $2, 'manages', 'manages', 'state', TRUE)",
        )
        .bind(manages)
        .bind(f.kb)
        .execute(&pool)
        .await?;
        sqlx::query(
            "INSERT INTO relation_types (id, kb_id, key, label, temporal, sub_property_of)
             VALUES ($1, $2, 'supervises', 'supervises', 'state', $3)",
        )
        .bind(supervises)
        .bind(f.kb)
        .bind(manages)
        .execute(&pool)
        .await?;
        let (alice, carol, dave) = (
            entity(&pool, &f, f.thing, "Alice").await?,
            entity(&pool, &f, f.thing, "Carol").await?,
            entity(&pool, &f, f.thing, "Dave").await?,
        );
        fact(&pool, &f, (alice, supervises, carol)).await?;
        fact(&pool, &f, (alice, supervises, dave)).await?;
        reasoning::run(&pool, f.kb).await?;
        // supervises ⊑ manages 派生出 manages(alice→carol) 与 manages(alice→dave)：
        // 两条派生撞上 functional——规则对层的发现进 ontology_defects
        let defects: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM ontology_defects
              WHERE kb_id = $1 AND kind = 'rules_disagree'",
        )
        .bind(f.kb)
        .fetch_one(&pool)
        .await?;
        anyhow::ensure!(
            defects == 1,
            "expected one rules_disagree defect, got {defects}"
        );

        // manages 零断言可以删：先断 supervises 的链再删再对账——发现随之消失，
        // 不留「一条只对已删判据成立的发现」走出提交点
        ontology::delete_relation_type(&pool, f.kb, manages).await?;
        let left: i64 =
            sqlx::query_scalar("SELECT count(*) FROM ontology_defects WHERE kb_id = $1")
                .bind(f.kb)
                .fetch_one(&pool)
                .await?;
        anyhow::ensure!(left == 0, "defect survived the deletion of its criterion");
        let dangling: i64 =
            sqlx::query_scalar("SELECT count(*) FROM relation_types WHERE sub_property_of = $1")
                .bind(manages)
                .fetch_one(&pool)
                .await?;
        anyhow::ensure!(
            dangling == 0,
            "deferred SET NULL would have hidden the link"
        );
        Ok(())
    }
    .await;
    let cleanup = cleanup(&pool, &f).await;
    result.and(cleanup)
}

/// 证据漂移不是身份漂移：行里留的 `path` 是上一轮拍下的组员集，检出重算
/// 时它可以变——分档比的是 `(kind, left, right)`，不是那张旧照片。一条
/// 旧判据下确实检得出的违规，不能因为存的证据换了组就当成「早就陈了」删掉
#[tokio::test]
async fn drifted_evidence_still_closes_as_criterion_changed() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let f = seed(&pool).await?;

    let result = async {
        let (a, b, c) = (
            entity(&pool, &f, f.thing, "A").await?,
            entity(&pool, &f, f.thing, "B").await?,
            entity(&pool, &f, f.thing, "C").await?,
        );
        let f1 = fact(&pool, &f, (a, f.reports_to, b)).await?;
        fact(&pool, &f, (b, f.reports_to, a)).await?;
        let f3 = fact(&pool, &f, (a, f.reports_to, c)).await?;
        reasoning::run(&pool, f.kb).await?;
        let (id,): (Uuid,) =
            sqlx::query_as("SELECT id FROM axiom_violations WHERE kb_id = $1 AND status = 'open'")
                .bind(f.kb)
                .fetch_one(&pool)
                .await?;

        // 模拟上一轮检出留下的旧证据：path 组员漂移过，端点没变——
        // 这还是同一处违规（0054：非环种类按 (kind,left,right) 认身份）
        sqlx::query("UPDATE axiom_violations SET path = $2 WHERE id = $1")
            .bind(id)
            .bind(&[f1, f3][..])
            .execute(&pool)
            .await?;

        ontology::update_relation_type(
            &pool,
            f.kb,
            f.reports_to,
            "reportsTo",
            "state",
            NONE,
            "",
            None,
            None,
            None,
            None,
        )
        .await?;
        let (status, resolution, _) = row_state(&pool, id).await?.unwrap();
        anyhow::ensure!(
            status == "resolved" && resolution.as_deref() == Some("criterion_changed"),
            "evidence drift must not turn a baseline finding into a deletion: \
             {status}/{resolution:?} (drifted path had f3={f3})"
        );
        Ok(())
    }
    .await;
    let cleanup = cleanup(&pool, &f).await;
    result.and(cleanup)
}

/// 断链同样改判据：`update_relation_type` 把 `sub_property_of` 摘掉，
/// 靠它派生的发现当场清掉——与 `delete_relation_type` 同一条规矩
#[tokio::test]
async fn unlinking_a_sub_property_reconciles_what_depended_on_it() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let f = seed(&pool).await?;

    let result = async {
        let (manages, supervises) = (Uuid::now_v7(), Uuid::now_v7());
        sqlx::query(
            "INSERT INTO relation_types (id, kb_id, key, label, temporal, functional)
             VALUES ($1, $2, 'manages', 'manages', 'state', TRUE)",
        )
        .bind(manages)
        .bind(f.kb)
        .execute(&pool)
        .await?;
        sqlx::query(
            "INSERT INTO relation_types (id, kb_id, key, label, temporal, sub_property_of)
             VALUES ($1, $2, 'supervises', 'supervises', 'state', $3)",
        )
        .bind(supervises)
        .bind(f.kb)
        .bind(manages)
        .execute(&pool)
        .await?;
        let (alice, carol, dave) = (
            entity(&pool, &f, f.thing, "Alice").await?,
            entity(&pool, &f, f.thing, "Carol").await?,
            entity(&pool, &f, f.thing, "Dave").await?,
        );
        fact(&pool, &f, (alice, supervises, carol)).await?;
        fact(&pool, &f, (alice, supervises, dave)).await?;
        reasoning::run(&pool, f.kb).await?;
        let defects: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM ontology_defects
              WHERE kb_id = $1 AND kind = 'rules_disagree'",
        )
        .bind(f.kb)
        .fetch_one(&pool)
        .await?;
        anyhow::ensure!(defects == 1, "expected one rules_disagree defect");

        // 摘掉 supervises ⊑ manages：判据变了，派生链断——发现随提交一起清
        ontology::update_relation_type(
            &pool,
            f.kb,
            supervises,
            "supervises",
            "state",
            NONE,
            "",
            None,
            None,
            None,
            None,
        )
        .await?;
        let left: i64 =
            sqlx::query_scalar("SELECT count(*) FROM ontology_defects WHERE kb_id = $1")
                .bind(f.kb)
                .fetch_one(&pool)
                .await?;
        anyhow::ensure!(
            left == 0,
            "defect survived the unlinking of its derivation chain"
        );
        Ok(())
    }
    .await;
    let cleanup = cleanup(&pool, &f).await;
    result.and(cleanup)
}

/// 删掉一个牵着判据的类，同一事务里把靠着它的发现收掉
#[tokio::test]
async fn deleting_a_domain_class_closes_the_signature_finding() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let f = seed(&pool).await?;

    let result = async {
        let (acme, alice) = (
            entity(&pool, &f, f.thing, "Acme").await?,
            entity(&pool, &f, f.thing, "Alice").await?,
        );
        fact(&pool, &f, (alice, f.works_for, acme)).await?;
        reasoning::run(&pool, f.kb).await?;
        anyhow::ensure!(open_count(&pool, &f).await? == 1);

        // person 是 works_for 的 domain：删掉它，判据跟着没了
        ontology::delete_entity_type(&pool, f.kb, f.person).await?;
        anyhow::ensure!(
            open_count(&pool, &f).await? == 0,
            "finding should close when its domain class is deleted"
        );
        let (status, resolution, _) = sqlx::query_as::<_, (String, Option<String>, bool)>(
            "SELECT status, resolution, decided_at IS NOT NULL FROM axiom_violations
              WHERE kb_id = $1 AND kind = 'signature'",
        )
        .bind(f.kb)
        .fetch_one(&pool)
        .await?;
        anyhow::ensure!(
            status == "resolved" && resolution.as_deref() == Some("criterion_changed"),
            "finding was not closed as criterion_changed: {status}/{resolution:?}"
        );
        Ok(())
    }
    .await;
    let cleanup = cleanup(&pool, &f).await;
    result.and(cleanup)
}

/// 改判一条零事实的关系成属性：它挂着的公理旗标与双向链接是关系的判据
/// 贡献，属性不该还留着——`axioms()` 不按 kind 过滤，留着等于留一套
/// 无人问的公理。同一事务里旗标清掉、链接断开、检出键集照 0062 对账
#[tokio::test]
async fn a_demoted_relation_surrenders_its_axioms() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let f = seed(&pool).await?;

    let result = async {
        let (manages, supervises) = (Uuid::now_v7(), Uuid::now_v7());
        sqlx::query(
            "INSERT INTO relation_types (id, kb_id, key, label, temporal)
             VALUES ($1, $2, 'supervises', 'supervises', 'state')",
        )
        .bind(supervises)
        .bind(f.kb)
        .execute(&pool)
        .await?;
        // manages：functional + inverse_of=supervises——零事实，够格被改判；
        // supervises.sub_property_of 补一条入向引用，改判也要把它断开
        sqlx::query(
            "INSERT INTO relation_types
                 (id, kb_id, key, label, temporal, functional, is_transitive, inverse_of)
             VALUES ($1, $2, 'manages', 'manages', 'state', TRUE, TRUE, $3)",
        )
        .bind(manages)
        .bind(f.kb)
        .bind(supervises)
        .execute(&pool)
        .await?;
        sqlx::query("UPDATE relation_types SET sub_property_of = $2 WHERE id = $1")
            .bind(supervises)
            .bind(manages)
            .execute(&pool)
            .await?;

        let converted = ontology::attribute_from_unused_relation(
            &pool,
            f.kb,
            "manages",
            &[f.thing],
            "text",
            None,
        )
        .await?
        .expect("a zero-facts relation should convert");
        anyhow::ensure!(converted == manages);
        let row: (String, bool, bool, Option<Uuid>, Option<Uuid>) = sqlx::query_as(
            "SELECT kind, functional, is_transitive, inverse_of, sub_property_of
               FROM relation_types WHERE id = $1",
        )
        .bind(manages)
        .fetch_one(&pool)
        .await?;
        anyhow::ensure!(row.0 == "attribute", "kind should flip: {}", row.0);
        anyhow::ensure!(!row.1 && !row.2, "axiom flags survived the demotion");
        anyhow::ensure!(
            row.3.is_none() && row.4.is_none(),
            "links survived the demotion"
        );
        // 入向引用同样断开：supervises 不再指向它（axioms() 的归一化不再
        // 把一个属性当派生目标）
        let still_linked: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM relation_types WHERE sub_property_of = $1)",
        )
        .bind(manages)
        .fetch_one(&pool)
        .await?;
        anyhow::ensure!(!still_linked, "incoming sub_property_of survived");
        Ok(())
    }
    .await;
    let cleanup = cleanup(&pool, &f).await;
    result.and(cleanup)
}
