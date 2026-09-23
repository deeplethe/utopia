//! 类型消解的近邻那一路（#514），打在真库上。
//!
//! 规矩是：消解的答案是台账的函数，与候选按什么顺序、什么时机取回来无关。
//! 所以并发地取只有在「同一个库给出同一批邻居」时才算安全。这里钉四样：
//! - **顺序**：一批主语的邻居按送进去的顺序回来，下游裁决按这个顺序读
//! - **并发上限不改答案**：上限 1 与上限 8 给出同一份结果；有没有索引也一样
//! - **SQL 里的三道门**：自己不是自己的邻居；没判出类型的不当证据；同一篇文档的要标出来
//! - **后代集合的记忆**：一批之内同一个粗类只问一次，没有粗类的不进表
//!
//! 还有一条只有连库才验得出：两个连接的小池子也跑得完一整批，不会撞上取连接的超时。

use pgvector::Vector;
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;
use std::collections::HashSet;
use utopia_store::resolution::{self, DescendantsMemo};
use utopia_store::vector_index::{self, Target};
use uuid::Uuid;

/// 这个测试独占的维度：其它连库测试用 3 维
const DIMS: usize = 5;
const TYPED: usize = 40;

fn lcg(seed: &mut u64) -> f32 {
    *seed = seed
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    ((*seed >> 33) as f32 / (1u64 << 31) as f32) - 0.5
}

struct Fixture {
    org: Uuid,
    kb: Uuid,
    thing: Uuid,
    person: Uuid,
    company: Uuid,
    employee: Uuid,
    /// 三个待消解的主语
    subjects: Vec<Uuid>,
    /// 自己的向量是 3 维的主语
    three_dims_subject: Uuid,
}

async fn seed(pool: &PgPool) -> anyhow::Result<Fixture> {
    let (org, ws, kb) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    let (thing, person, company, employee) = (
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
    );
    let tag = Uuid::now_v7();

    sqlx::query("INSERT INTO organizations (id, name) VALUES ($1, 'gather-test')")
        .bind(org)
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO workspaces (id, org_id, name) VALUES ($1, $2, 'gather-test')")
        .bind(ws)
        .bind(org)
        .execute(pool)
        .await?;
    sqlx::query(
        "INSERT INTO knowledge_bases (id, workspace_id, name) VALUES ($1, $2, 'gather-test')",
    )
    .bind(kb)
    .bind(ws)
    .execute(pool)
    .await?;
    for (id, key) in [
        (thing, "thing"),
        (person, "person"),
        (company, "company"),
        (employee, "employee"),
    ] {
        sqlx::query("INSERT INTO entity_types (id, kb_id, key, label) VALUES ($1, $2, $3, $3)")
            .bind(id)
            .bind(kb)
            .bind(key)
            .execute(pool)
            .await?;
    }
    for (child, parent) in [(person, thing), (company, thing), (employee, person)] {
        sqlx::query("INSERT INTO entity_type_parents (child_id, parent_id) VALUES ($1, $2)")
            .bind(child)
            .bind(parent)
            .execute(pool)
            .await?;
    }

    let mut seed = 0xfeed_u64;
    let mut vector = |dims: usize| -> Vector {
        Vector::from((0..dims).map(|_| lcg(&mut seed)).collect::<Vec<f32>>())
    };
    let insert = |id: Uuid, name: String, type_id: Option<Uuid>, v: Vector| {
        let pool = pool.clone();
        async move {
            sqlx::query(
                "INSERT INTO entities (id, kb_id, type_id, canonical_name, profile_embedding, profile_n)
                 VALUES ($1, $2, $3, $4, $5, 1)",
            )
            .bind(id)
            .bind(kb)
            .bind(type_id)
            .bind(name)
            .bind(v)
            .execute(&pool)
            .await
        }
    };
    let types = [person, company, employee];
    let mut typed = Vec::with_capacity(TYPED);
    for i in 0..TYPED {
        let id = Uuid::now_v7();
        insert(id, format!("typed {i}"), Some(types[i % 3]), vector(DIMS)).await?;
        typed.push(id);
    }
    let subjects: Vec<Uuid> = (0..3).map(|_| Uuid::now_v7()).collect();
    for (i, s) in subjects.iter().enumerate() {
        insert(*s, format!("subject {i}"), Some(thing), vector(DIMS)).await?;
    }
    let untyped = Uuid::now_v7();
    insert(untyped, "untyped".into(), None, vector(DIMS)).await?;
    let three_dims = Uuid::now_v7();
    insert(three_dims, "three dims".into(), Some(person), vector(3)).await?;
    let three_dims_subject = Uuid::now_v7();
    insert(
        three_dims_subject,
        "three dims subject".into(),
        Some(thing),
        vector(3),
    )
    .await?;

    // 主语 0 和 typed[0] 在同一篇文档里：一条事实，证据落在那篇文档的一块上
    let same_doc_neighbour = typed[0];
    let (doc, chunk, fact) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    sqlx::query(
        "INSERT INTO documents (id, kb_id, filename, sha256, status)
         VALUES ($1, $2, 'shared.md', $3, 'ready')",
    )
    .bind(doc)
    .bind(kb)
    .bind(format!("sha-{tag}"))
    .execute(pool)
    .await?;
    sqlx::query(
        "INSERT INTO chunks (id, kb_id, document_id, seq, text) VALUES ($1, $2, $3, 0, 'a sentence')",
    )
    .bind(chunk)
    .bind(kb)
    .bind(doc)
    .execute(pool)
    .await?;
    sqlx::query("INSERT INTO facts (id, kb_id, subject_id, object_id) VALUES ($1, $2, $3, $4)")
        .bind(fact)
        .bind(kb)
        .bind(subjects[0])
        .bind(same_doc_neighbour)
        .execute(pool)
        .await?;
    sqlx::query(
        "INSERT INTO fact_evidence (fact_id, chunk_id, quote, document_id)
         VALUES ($1, $2, 'a sentence', $3)",
    )
    .bind(fact)
    .bind(chunk)
    .bind(doc)
    .execute(pool)
    .await?;

    Ok(Fixture {
        org,
        kb,
        thing,
        person,
        company,
        employee,
        subjects,
        three_dims_subject,
    })
}

type Neighbours = Vec<(String, Uuid, String, f64, bool)>;

fn names(list: &Neighbours) -> Vec<&str> {
    list.iter().map(|(n, ..)| n.as_str()).collect()
}

#[tokio::test]
async fn the_neighbours_come_back_in_order_and_the_gates_hold() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let f = seed(&pool).await?;

    let run = async {
        vector_index::drop(&pool, Target::EntityProfiles, DIMS).await?;

        // ---- 一、逐个问，作为基准
        let mut serial: Vec<Neighbours> = Vec::new();
        for s in &f.subjects {
            serial.push(resolution::nearest_typed_entities(&pool, f.kb, *s, 10).await?);
        }
        for (i, list) in serial.iter().enumerate() {
            assert_eq!(list.len(), 10, "主语 {i} 该有十个邻居");
            let me = format!("subject {i}");
            assert!(
                list.iter().all(|(n, ..)| *n != me),
                "自己不是自己的邻居（别的主语可以是：它们挂着 thing，是已定类的）"
            );
            assert!(
                list.iter().all(|(n, ..)| n != "untyped"),
                "没判出类型的不当证据"
            );
            assert!(
                list.iter().all(|(n, ..)| n != "three dims"),
                "另一维度的画像不是候选"
            );
        }
        // 主语 0 的邻居里，typed 0 与它同一篇文档，别的都不是
        let flagged: Vec<&str> = serial[0]
            .iter()
            .filter(|(_, _, _, _, same)| *same)
            .map(|(n, ..)| n.as_str())
            .collect();
        let has_typed0 = serial[0].iter().any(|(n, ..)| n == "typed 0");
        if has_typed0 {
            assert_eq!(flagged, vec!["typed 0"], "同一篇文档的要标出来，且只标它");
        } else {
            assert!(flagged.is_empty(), "不在邻居里就没什么可标");
        }
        // 自己的向量是 3 维的主语：没有 3 维的已定类邻居可比……除了那一条 3 维的
        let odd = resolution::nearest_typed_entities(&pool, f.kb, f.three_dims_subject, 10).await?;
        assert_eq!(
            names(&odd),
            vec!["three dims"],
            "3 维的查询只找 3 维的，也不报错"
        );

        // ---- 二、一批取：顺序与上限
        let gathered_1 =
            resolution::nearest_typed_for_each_with(&pool, f.kb, &f.subjects, 10, 1).await?;
        let gathered_8 =
            resolution::nearest_typed_for_each_with(&pool, f.kb, &f.subjects, 10, 8).await?;
        assert_eq!(gathered_1.len(), f.subjects.len());
        for i in 0..f.subjects.len() {
            assert_eq!(
                names(&gathered_1[i]),
                names(&serial[i]),
                "上限 1：第 {i} 个位置是第 {i} 个主语的邻居"
            );
            assert_eq!(
                names(&gathered_8[i]),
                names(&serial[i]),
                "上限 8：第 {i} 个位置是第 {i} 个主语的邻居"
            );
        }

        // ---- 三、两个连接的池子也跑得完六十个
        let tiny = PgPoolOptions::new()
            .max_connections(2)
            .acquire_timeout(std::time::Duration::from_secs(10))
            .connect(&url)
            .await?;
        let sixty: Vec<Uuid> = f.subjects.iter().cycle().take(60).copied().collect();
        let over_tiny = resolution::nearest_typed_for_each_with(&tiny, f.kb, &sixty, 10, 8).await?;
        assert_eq!(over_tiny.len(), 60);
        for (i, list) in over_tiny.iter().enumerate() {
            assert_eq!(names(list), names(&serial[i % 3]));
        }
        tiny.close().await;

        // ---- 四、索引就位之后同一份答案
        let built = vector_index::build(&pool, Target::EntityProfiles, DIMS).await?;
        assert!(built.created);
        assert_eq!(built.name, "entities_profile_embedding_hnsw_5");
        let indexed = resolution::nearest_typed_for_each(&pool, f.kb, &f.subjects, 10).await?;
        for i in 0..f.subjects.len() {
            assert_eq!(
                names(&indexed[i]),
                names(&serial[i]),
                "索引改了主语 {i} 的邻居"
            );
        }

        // ---- 五、后代集合按粗类记一批
        let mut memo = DescendantsMemo::default();
        let direct = |root: Uuid| {
            let pool = pool.clone();
            async move {
                Ok::<HashSet<Uuid>, anyhow::Error>(
                    resolution::descendants_of(&pool, f.kb, root)
                        .await?
                        .into_iter()
                        .collect(),
                )
            }
        };
        assert_eq!(
            memo.get(&pool, f.kb, Some(f.thing)).await?,
            direct(f.thing).await?
        );
        assert_eq!(
            memo.get(&pool, f.kb, Some(f.person)).await?,
            [f.person, f.employee].into_iter().collect::<HashSet<_>>()
        );
        assert_eq!(
            memo.get(&pool, f.kb, Some(f.company)).await?,
            direct(f.company).await?
        );
        assert_eq!(memo.len(), 3);
        assert!(
            memo.get(&pool, f.kb, None).await?.is_empty(),
            "没有粗类：没有后代这个轴"
        );
        assert_eq!(memo.len(), 3, "没有粗类的不进表");
        // 批内本体动了：记忆给的还是问过的那一份，直接问给的是新的——这就是「记住」
        let intern = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO entity_types (id, kb_id, key, label) VALUES ($1, $2, 'intern', 'intern')",
        )
        .bind(intern)
        .bind(f.kb)
        .execute(&pool)
        .await?;
        sqlx::query("INSERT INTO entity_type_parents (child_id, parent_id) VALUES ($1, $2)")
            .bind(intern)
            .bind(f.person)
            .execute(&pool)
            .await?;
        assert!(!memo
            .get(&pool, f.kb, Some(f.person))
            .await?
            .contains(&intern));
        assert!(direct(f.person).await?.contains(&intern));
        Ok::<_, anyhow::Error>(())
    }
    .await;

    let _ = vector_index::drop(&pool, Target::EntityProfiles, DIMS).await;
    sqlx::query("DELETE FROM knowledge_bases WHERE id = $1")
        .bind(f.kb)
        .execute(&pool)
        .await?;
    sqlx::query("DELETE FROM organizations WHERE id = $1")
        .bind(f.org)
        .execute(&pool)
        .await?;
    run
}
