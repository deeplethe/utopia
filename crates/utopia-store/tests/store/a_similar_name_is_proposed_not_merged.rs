//! 名字向量召回（0041 决定 3 通道 2，第 2 刀）：**相近的名字来报到，但只提议，不归并。**
//!
//! 库里有 海洋探测器1号，一篇新文档只写 海探1。字面召回（通道 1）碰不上它——不相等、
//! 又短于 containment 的四字门槛——从前这里静默长出第二个实体（#709）。现在 mention 的
//! 名字向量在同库的名字向量里取最近邻，够近就给裁决器排一对 `name_vector|<余弦>`；
//! mention 自己照常按字面路径走（这里是新建）。是不是一个，裁决器拿两份画像判。
//!
//! 连库才测得到：近邻是 SQL 里的 `<=>`。没有 `UTOPIA_DATABASE_URL` 时跳过，自建自拆。
//! 向量都是手摆的三维——测的是召回和提议的规矩，不是某个嵌入模型的远近观。

use sqlx::PgPool;
use utopia_store::resolution::ReviewStage;
use utopia_store::{name_vectors, names};
use uuid::Uuid;

struct Fx {
    org: Uuid,
    kb: Uuid,
    device: Uuid,
    person: Uuid,
    probe: Uuid,
    captain: Uuid,
}

async fn seed(pool: &PgPool, tag: &str) -> anyhow::Result<Fx> {
    let (org, ws, kb) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    let (device, person) = (Uuid::now_v7(), Uuid::now_v7());
    let (probe, captain) = (Uuid::now_v7(), Uuid::now_v7());
    sqlx::query("INSERT INTO organizations (id, name) VALUES ($1, $2)")
        .bind(org)
        .bind(tag)
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO workspaces (id, org_id, name) VALUES ($1, $2, $3)")
        .bind(ws)
        .bind(org)
        .bind(tag)
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO knowledge_bases (id, workspace_id, name) VALUES ($1, $2, $3)")
        .bind(kb)
        .bind(ws)
        .bind(tag)
        .execute(pool)
        .await?;
    // 两个大类：设备（type_family 认不出，None）与人（Person）
    for (id, key, label) in [(device, "device", "Device"), (person, "person", "Person")] {
        sqlx::query("INSERT INTO entity_types (id, kb_id, key, label) VALUES ($1, $2, $3, $4)")
            .bind(id)
            .bind(kb)
            .bind(key)
            .bind(label)
            .execute(pool)
            .await?;
    }
    for (id, type_id, name) in [
        (probe, device, "海洋探测器1号"),
        (captain, person, "海洋探测队长"),
    ] {
        sqlx::query(
            "INSERT INTO entities (id, kb_id, type_id, canonical_name) VALUES ($1, $2, $3, $4)",
        )
        .bind(id)
        .bind(kb)
        .bind(type_id)
        .bind(name)
        .execute(pool)
        .await?;
        let fact = names::record(pool, kb, id, name, None, None)
            .await?
            .expect("a named entity gets a name fact");
        // 两个名字的向量故意一样：只有大类能把它们分开
        name_vectors::set(pool, kb, &[(fact, id, vec![1.0, 0.0, 0.0])]).await?;
    }
    Ok(Fx {
        org,
        kb,
        device,
        person,
        probe,
        captain,
    })
}

async fn teardown(pool: &PgPool, f: &Fx) -> anyhow::Result<()> {
    sqlx::query("DELETE FROM knowledge_bases WHERE id = $1")
        .bind(f.kb)
        .execute(pool)
        .await?;
    sqlx::query("DELETE FROM organizations WHERE id = $1")
        .bind(f.org)
        .execute(pool)
        .await?;
    Ok(())
}

fn vector_reviews(r: &utopia_store::resolution::Resolution) -> Vec<(Uuid, f32)> {
    r.reviews
        .iter()
        .filter(|v| v.reason.starts_with("name_vector|"))
        .map(|v| (v.other_id, v.score))
        .collect()
}

/// 相近的名字：新建实体，给裁决器排一对，绝不静默归并
#[tokio::test]
async fn a_near_name_creates_an_entity_and_proposes_a_pair() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let f = seed(&pool, "name-vector-near").await?;
    let run = async {
        let near: Vec<f32> = vec![0.95, 0.31, 0.0];
        let r = utopia_store::resolution::resolve_mention(
            &pool,
            f.kb,
            Some(f.device),
            "海探1",
            None,
            Some(&near),
            None,
            &[],
        )
        .await?;
        assert!(r.created, "字面碰不上，mention 该新建实体");
        assert_ne!(
            r.entity_id, f.probe,
            "向量召回只提议，不能直接归并到 海洋探测器1号"
        );
        let proposed = vector_reviews(&r);
        assert!(
            proposed.iter().any(|(id, _)| *id == f.probe),
            "海洋探测器1号 该被提议：{:?}",
            r.reviews
        );
        // 夹具里两个名字的向量一样，而 Device 认不出大类、不拦 Person：海洋探测队长 也进来。
        // 这不是漏，是规矩——分不出大类时宁可多问；大类分得出时的拦截见下一条测试
        assert_eq!(
            proposed.len(),
            2,
            "Device 无大类，不拦同向量的 Person：{:?}",
            r.reviews
        );
        assert!(
            proposed.iter().all(|(_, s)| *s >= name_vectors::SIM_FLOOR),
            "分数是余弦本身"
        );
        assert!(
            r.reviews
                .iter()
                .all(|v| v.stage == ReviewStage::Adjudicating),
            "名字相近的对交给批量裁决器，不是人"
        );
        Ok::<_, anyhow::Error>(())
    }
    .await;
    teardown(&pool, &f).await?;
    run
}

/// 大类对不上的不提议：设备的名字再像，也不是那个人
#[tokio::test]
async fn a_near_name_of_another_family_is_not_proposed() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let f = seed(&pool, "name-vector-family").await?;
    let run = async {
        // mention 是个人；海洋探测器1号 的类 Device 认不出大类（None），照规矩不拦；
        // 海洋探测队长 是 Person，同类，拦不住——所以两条都会进来。换成 Organization
        // 的 mention 才能看到 Person 被拦：
        let organization = Uuid::now_v7();
        sqlx::query("INSERT INTO entity_types (id, kb_id, key, label) VALUES ($1, $2, 'organization', 'Organization')")
            .bind(organization)
            .bind(f.kb)
            .execute(&pool)
            .await?;
        let near: Vec<f32> = vec![1.0, 0.0, 0.0];
        let r = utopia_store::resolution::resolve_mention(
            &pool,
            f.kb,
            Some(organization),
            "海洋探测公司",
            None,
            Some(&near),
            None,
            &[],
        )
        .await?;
        let proposed: Vec<Uuid> = vector_reviews(&r).into_iter().map(|(id, _)| id).collect();
        assert!(
            !proposed.contains(&f.captain),
            "Person 与 Organization 大类不同，海洋探测队长 不该被提议：{:?}",
            r.reviews
        );
        assert!(
            proposed.contains(&f.probe),
            "Device 认不出大类，不拦；海洋探测器1号 照常提议：{:?}",
            r.reviews
        );
        let _ = f.person;
        Ok::<_, anyhow::Error>(())
    }
    .await;
    teardown(&pool, &f).await?;
    run
}

/// 不够近的不提议；没给向量的照旧
#[tokio::test]
async fn a_far_name_or_no_vector_proposes_nothing() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let f = seed(&pool, "name-vector-far").await?;
    let run = async {
        let far: Vec<f32> = vec![0.0, 1.0, 0.0];
        let r = utopia_store::resolution::resolve_mention(
            &pool,
            f.kb,
            Some(f.device),
            "深海机器人",
            None,
            Some(&far),
            None,
            &[],
        )
        .await?;
        assert!(
            vector_reviews(&r).is_empty(),
            "余弦 0 低于下限，不提议：{:?}",
            r.reviews
        );
        let r = utopia_store::resolution::resolve_mention(
            &pool,
            f.kb,
            Some(f.device),
            "海探1",
            None,
            None,
            None,
            &[],
        )
        .await?;
        assert!(
            vector_reviews(&r).is_empty(),
            "没给名字向量就不走通道 2：{:?}",
            r.reviews
        );
        Ok::<_, anyhow::Error>(())
    }
    .await;
    teardown(&pool, &f).await?;
    run
}

/// 字面命中的候选是通道 1 的事：同一个名字不再经通道 2 复议
#[tokio::test]
async fn the_same_literal_name_is_not_proposed_twice() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let f = seed(&pool, "name-vector-literal").await?;
    let run = async {
        let same: Vec<f32> = vec![1.0, 0.0, 0.0];
        // 没有上下文向量：v1 路径归并到唯一的字面候选
        let r = utopia_store::resolution::resolve_mention(
            &pool,
            f.kb,
            Some(f.device),
            "海洋探测器1号",
            None,
            Some(&same),
            None,
            &[],
        )
        .await?;
        assert_eq!(r.entity_id, f.probe, "字面相等走通道 1");
        // 夹具里 海洋探测队长 的向量与之相同、大类又拦不住，它照常被提议；要看的只是
        // 已归并到的 海洋探测器1号 自己不会再被排一对
        assert!(
            !vector_reviews(&r).iter().any(|(id, _)| *id == f.probe),
            "已经归并到它，不再对它排 name_vector：{:?}",
            r.reviews
        );
        Ok::<_, anyhow::Error>(())
    }
    .await;
    teardown(&pool, &f).await?;
    run
}

/// 补向量的往返：新记的名字事实在 `pending` 里，写了向量就不在了。端到端跑出来的窟窿——
/// 之前 `pending` 按一个不存在的列排序，每篇文档的补向量都静默失败，通道 2 从未触发
#[tokio::test]
async fn a_new_name_is_pending_until_its_vector_is_set() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let f = seed(&pool, "name-vector-pending").await?;
    let run = async {
        let fresh = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO entities (id, kb_id, type_id, canonical_name) VALUES ($1, $2, $3, '远洋重工')",
        )
        .bind(fresh)
        .bind(f.kb)
        .bind(f.device)
        .execute(&pool)
        .await?;
        let fact = names::record(&pool, f.kb, fresh, "远洋重工", None, None)
            .await?
            .expect("a name fact");
        let pending = name_vectors::pending(&pool, f.kb, 100).await?;
        assert!(
            pending.iter().any(|(fid, eid, name)| *fid == fact && *eid == fresh && name == "远洋重工"),
            "新记的名字该在待补清单里：{pending:?}"
        );
        // 夹具里两个已有向量的名字不在清单里
        assert!(pending.iter().all(|(_, eid, _)| *eid != f.probe && *eid != f.captain));
        name_vectors::set(&pool, f.kb, &[(fact, fresh, vec![0.0, 0.0, 1.0])]).await?;
        let after = name_vectors::pending(&pool, f.kb, 100).await?;
        assert!(after.iter().all(|(fid, _, _)| *fid != fact), "写了向量就不该再待补");
        Ok::<_, anyhow::Error>(())
    }
    .await;
    teardown(&pool, &f).await?;
    run
}
