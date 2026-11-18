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

fn contained(r: &utopia_store::resolution::Resolution) -> Vec<Uuid> {
    r.reviews
        .iter()
        .filter(|v| v.reason.starts_with("contains|"))
        .map(|v| v.other_id)
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
        // A second entity with the same name stays a channel-1 decision, including
        // when the mention carries Markdown. Neither candidate is excluded here.
        sqlx::query(
            "UPDATE entities SET canonical_name = '海洋探测器1号', type_id = $2 WHERE id = $1",
        )
        .bind(f.captain)
        .bind(f.device)
        .execute(&pool)
        .await?;
        sqlx::query(
            "UPDATE facts SET object_value = '{\"value\":\"海洋探测器1号\"}' WHERE subject_id = $1",
        )
        .bind(f.captain)
        .execute(&pool)
        .await?;
        for mention in ["海洋探测器1号", "**海洋探测器1号**"] {
            let r = utopia_store::resolution::resolve_mention(
                &pool,
                f.kb,
                Some(f.device),
                mention,
                None,
                Some(&same),
                None,
                &[],
            )
            .await?;
            assert!(!r.created);
            assert!(
                vector_reviews(&r).is_empty(),
                "a namesake must not be reconsidered by name vector: {:?}",
                r.reviews
            );
        }
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

/// 上一篇文档没补上的名字，下一篇照样补：`pending` 取整个库里没有向量的名字，先记的在前，
/// 不只是这篇刚记的。有向量的名字不在里面（#1296 改这条查询时守住这次重试）
#[tokio::test]
async fn a_name_left_without_a_vector_earlier_is_pending_before_a_new_one() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let f = seed(&pool, "name-vector-backlog").await?;
    let run = async {
        let mut recorded = Vec::new();
        for name in ["远洋重工", "深蓝船舶"] {
            let entity = Uuid::now_v7();
            sqlx::query(
                "INSERT INTO entities (id, kb_id, type_id, canonical_name) VALUES ($1, $2, $3, $4)",
            )
            .bind(entity)
            .bind(f.kb)
            .bind(f.device)
            .bind(name)
            .execute(&pool)
            .await?;
            let fact = names::record(&pool, f.kb, entity, name, None, None)
                .await?
                .expect("a name fact");
            recorded.push((fact, entity, name.to_string()));
        }

        assert_eq!(name_vectors::pending(&pool, f.kb, 100).await?, recorded);
        Ok::<_, anyhow::Error>(())
    }
    .await;
    teardown(&pool, &f).await?;
    run
}

/// 判过「不是一个」的一对，实体再被提到时不再排（#1104）。名字向量通道每次提到都跑，
/// 而审核表只对 pending 的行去重：从前裁决器分开过的一对，下一篇文档一提到就又是一条
/// 新的 pending。还没判的那一对不受影响，照常提议
#[tokio::test]
async fn a_pair_kept_apart_is_not_proposed_again() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let f = seed(&pool, "name-vector-kept-apart").await?;
    let run = async {
        let near: Vec<f32> = vec![0.95, 0.31, 0.0];
        let mention = || {
            utopia_store::resolution::resolve_mention(
                &pool,
                f.kb,
                Some(f.device),
                "海探1",
                None,
                Some(&near),
                None,
                &[],
            )
        };
        let first = mention().await?;
        assert!(first.created);
        for v in &first.reviews {
            utopia_store::resolution::create_review(
                &pool,
                f.kb,
                first.entity_id,
                v.other_id,
                v.score,
                &v.reason,
                v.stage,
            )
            .await?;
        }
        // 裁决器判 海探1 与 海洋探测器1号 不是一个；跟 海洋探测队长 的那一对还没判
        let (review,): (Uuid,) = sqlx::query_as(
            "SELECT id FROM resolution_reviews WHERE kb_id = $1 AND status = 'pending'
              AND $2 IN (left_id, right_id)",
        )
        .bind(f.kb)
        .bind(f.probe)
        .fetch_one(&pool)
        .await?;
        utopia_store::resolution::close_review_auto(&pool, review, "kept", "kept_apart|test")
            .await?;

        let again = mention().await?;
        assert!(!again.created, "同一个字面名字归到上一轮建的实体");
        assert_eq!(again.entity_id, first.entity_id);
        let proposed: Vec<Uuid> = vector_reviews(&again)
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        assert!(
            !proposed.contains(&f.probe),
            "分开过的一对不该再提议：{:?}",
            again.reviews
        );
        assert!(
            proposed.contains(&f.captain),
            "还没判的一对照常提议（入队时对 pending 幂等）：{:?}",
            again.reviews
        );
        Ok::<_, anyhow::Error>(())
    }
    .await;
    teardown(&pool, &f).await?;
    run
}

/// 只差一个数的名字不配对（#1104）：向量上几乎重合、字面上互相包含，却不是同一个东西。
/// 两条通道各守一次；一边没有数字的简称照常提议
#[tokio::test]
async fn names_that_carry_different_numbers_are_not_proposed() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let f = seed(&pool, "name-vector-numbers").await?;
    let run = async {
        let resolve = |name: &'static str, vector: Option<&'static [f32]>| {
            utopia_store::resolution::resolve_mention(
                &pool,
                f.kb,
                Some(f.device),
                name,
                None,
                vector,
                None,
                &[],
            )
        };
        // 名字向量：2号 的向量跟 1号 一模一样，照样不提议；没有数字的 队长 不受影响
        let second = resolve("海洋探测器2号", Some(&[1.0, 0.0, 0.0])).await?;
        let proposed: Vec<Uuid> = vector_reviews(&second)
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        assert!(
            !proposed.contains(&f.probe),
            "1号 与 2号 不该配对：{:?}",
            second.reviews
        );
        assert!(
            proposed.contains(&f.captain),
            "没有数字的名字照常提议：{:?}",
            second.reviews
        );

        // 包含关系：`migration 39` 是 `migration 395` 的子串，但不是它的简称
        let long = resolve("migration 395", None).await?;
        let short = resolve("migration 39", None).await?;
        assert!(short.created);
        assert!(
            !contained(&short).contains(&long.entity_id),
            "数字不同的包含不配对：{:?}",
            short.reviews
        );
        // 一边没有数字：简称对全称，正是这条通道要接住的
        let full = resolve("星云推理平台 2.0", None).await?;
        let brief = resolve("星云推理平台", None).await?;
        assert!(
            contained(&brief).contains(&full.entity_id),
            "简称照常配到带版本号的全称上：{:?}",
            brief.reviews
        );
        Ok::<_, anyhow::Error>(())
    }
    .await;
    teardown(&pool, &f).await?;
    run
}

/// 名字里的 `_` 按字面比，不当通配符（#1293）：`src/a_b.ts` 不包含在 `src/axb.ts::adds` 里，
/// 反过来 `a_b.ts` 也不在后者里；真包含的 `src/a_b.ts` 照常配对
#[tokio::test]
async fn an_underscore_in_a_name_is_not_a_wildcard() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let f = seed(&pool, "contain-underscore").await?;
    let run = async {
        let resolve = |name: &'static str| {
            utopia_store::resolution::resolve_mention(
                &pool,
                f.kb,
                Some(f.device),
                name,
                None,
                None,
                None,
                &[],
            )
        };
        let file = resolve("src/a_b.ts").await?;
        let other = resolve("src/axb.ts::adds").await?;
        assert!(
            !contained(&other).contains(&file.entity_id),
            "库里名字的 _ 不匹配 x：{:?}",
            other.reviews
        );
        let brief = resolve("a_b.ts").await?;
        assert_eq!(
            contained(&brief),
            vec![file.entity_id],
            "mention 的 _ 不匹配 x，真包含的照常配对：{:?}",
            brief.reviews
        );
        Ok::<_, anyhow::Error>(())
    }
    .await;
    teardown(&pool, &f).await?;
    run
}

/// 别名也是召回的桥：`Holmes` 是 `Sherlock Holmes` 的别名，`Mr. Holmes` 含 `Holmes`，
/// 却不含本名 `Sherlock Holmes`，也不被它包含
#[tokio::test]
async fn mr_holmes_is_paired_with_sherlock_holmes_through_its_alias_holmes() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let f = seed(&pool, "contain-alias").await?;
    let run = async {
        let resolve = |name: &'static str| {
            utopia_store::resolution::resolve_mention(
                &pool,
                f.kb,
                Some(f.device),
                name,
                None,
                None,
                None,
                &[],
            )
        };
        let detective = resolve("Sherlock Holmes").await?;
        names::record(&pool, f.kb, detective.entity_id, "Holmes", None, None).await?;
        let mr = resolve("Mr. Holmes").await?;
        assert_eq!(
            contained(&mr),
            vec![detective.entity_id],
            "别名 Holmes 被 Mr. Holmes 包含：{:?}",
            mr.reviews
        );
        Ok::<_, anyhow::Error>(())
    }
    .await;
    teardown(&pool, &f).await?;
    run
}

/// 按描述建的实体没有名字事实（0044），描述文字不是召回的桥；
/// 同样的文字落在普通实体的 `known_as` 上照常配对
#[tokio::test]
async fn a_described_entity_is_not_paired_by_containment() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let f = seed(&pool, "contain-described").await?;
    let run = async {
        let described = utopia_store::resolution::create_described(
            &pool,
            f.kb,
            "the museum Sherlock Holmes fans visit",
            None,
        )
        .await?;
        let named = utopia_store::resolution::resolve_mention(
            &pool,
            f.kb,
            Some(f.device),
            "the museum Sherlock Holmes fans visit",
            None,
            None,
            None,
            &[],
        )
        .await?;
        let mention = utopia_store::resolution::resolve_mention(
            &pool,
            f.kb,
            Some(f.device),
            "Sherlock Holmes",
            None,
            None,
            None,
            &[],
        )
        .await?;
        assert_eq!(
            contained(&mention),
            vec![named.entity_id],
            "描述建的 {described} 不配对，同文字的普通实体配对：{:?}",
            mention.reviews
        );
        Ok::<_, anyhow::Error>(())
    }
    .await;
    teardown(&pool, &f).await?;
    run
}

/// 拉丁名字只在词边界上算包含（#1104）；汉字照旧按子串匹配
#[tokio::test]
async fn a_latin_name_is_contained_only_at_word_edges() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let f = seed(&pool, "contain-word-edges").await?;
    let run = async {
        let resolve = |name: &'static str| {
            utopia_store::resolution::resolve_mention(
                &pool,
                f.kb,
                Some(f.device),
                name,
                None,
                None,
                None,
                &[],
            )
        };
        // (库里的名字, 它的别名, 新名字, 是否排成一对)
        for (stored, alias, new, want) in [
            ("SERVICE_TOKEN", None, "token", false),
            ("install_scheduled_task.ps1", None, "task", false),
            ("AreaScorecard", None, "area", false),
            ("Sherlock Holmes", None, "Holmes", true),
            ("星云科技上海研究院", None, "上海研究院", true),
            ("pump", None, "pumpkin soup", false),
            ("Watson", None, "Doctor Watson", true),
            (
                "Scotland Yard",
                Some("Inspector Lestrade"),
                "Lestrade",
                true,
            ),
            ("Gregson file", Some("gregson_notes"), "notes", false),
            ("Baker Street", Some("Hudson"), "Mrs Hudson", true),
            ("Prefect file", Some("Ford"), "Fordham Road", false),
        ] {
            let old = resolve(stored).await?;
            if let Some(alias) = alias {
                utopia_store::names::record(&pool, f.kb, old.entity_id, alias, None, None).await?;
            }
            let r = resolve(new).await?;
            assert_eq!(
                contained(&r).contains(&old.entity_id),
                want,
                "{new} against {stored}: {:?}",
                r.reviews
            );
        }
        Ok::<_, anyhow::Error>(())
    }
    .await;
    teardown(&pool, &f).await?;
    run
}
