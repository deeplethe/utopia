//! 100k 基准台（路线图 §Enterprise）。
//!
//! 第一个场景：冷 `SELECT ... WHERE`，30 跑，印 p50/p95/p99/均值/吞吐。
//! 表建在 `bench_100k_docs` 上——一次性合成表，不挂在 `documents` 上：现在
//! 还在谈形状，先别动真表。`DROP TABLE` 兜在结尾，CI 上不留。
//!
//! 跳过规矩与 `test_db` 同：`UTOPIA_DATABASE_URL` 没设就 `Ok(())`；CI 上
//! `UTOPIA_TEST_REQUIRE_DB=1` 把它变成"本该跑的没跑"的失败。
//!
//! 没有 criterion、没有 divan：`Instant::now()` 加一个排序挑百分位，二十行，
//! 与 `crates/utopia-store/src/vector_index.rs:181` 已有的写法一致。

use sqlx::PgPool;
use std::time::Instant;

const ROWS: i64 = 100_000;
const ITERATIONS: usize = 30;

/// 与 `crates/utopia-store/src/test_db.rs:14` 走同一条路。
async fn connect() -> anyhow::Result<Option<PgPool>> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(None);
    };
    Ok(Some(PgPool::connect(&url).await?))
}

/// 合成一张 `bench_100k_docs` 并灌满。**不动 `documents` / `chunks`**——
/// 那两张表在 30 个 store 连库测试之间是共享的，塞 100k 行进去等于把
/// 别人的测试也拖慢两秒。CI 安全在结尾的 `DROP TABLE`。
async fn seed(pool: &PgPool) -> anyhow::Result<()> {
    sqlx::query(
        "CREATE TABLE bench_100k_docs (
            id          BIGSERIAL PRIMARY KEY,
            kb_id       BIGINT      NOT NULL,
            kind        SMALLINT    NOT NULL,
            name        TEXT        NOT NULL,
            body        TEXT        NOT NULL,
            n           DOUBLE PRECISION NOT NULL,
            created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
         )",
    )
    .execute(pool)
    .await?;

    // 5000 一批：100k 行除以 5000 = 20 批。VALUES 拼出来后再绑参数，
    // **不要走 COPY**：连库测试环境里有别的连接，COPY 会和它们抢锁。
    let mut tx = pool.begin().await?;
    let mut buf = String::with_capacity(64 * 1024);
    let mut count: i64 = 0;
    for batch in 0..(ROWS / 5000) {
        buf.clear();
        buf.push_str("INSERT INTO bench_100k_docs (kb_id, kind, name, body, n) VALUES ");
        for i in 0..5000 {
            let id = batch * 5000 + i;
            if i > 0 {
                buf.push(',');
            }
            // 确定性种子：固定整数常量。同硬件两跑相同分布。
            let k = ((id.wrapping_mul(0x1000_0001_i64)) % 8) as i16;
            let kb = (id.wrapping_mul(0x9E37_79B9_7F4A_7C15_u64 as i64)) % 4 + 1;
            buf.push_str(&format!(
                "({}, {}, 'doc-{}', 'body {}', {})",
                kb,
                k,
                id,
                id,
                (id as f64) * 0.001
            ));
        }
        sqlx::query(&buf).execute(&mut *tx).await?;
        count += 5000;
    }
    tx.commit().await?;
    assert_eq!(count, ROWS, "没灌满");
    Ok(())
}

async fn teardown(pool: &PgPool) {
    // `IF EXISTS`：跑到一半失败也走得掉；不留残表给下一个测试。
    let _ = sqlx::query("DROP TABLE IF EXISTS bench_100k_docs")
        .execute(pool)
        .await;
}

fn percentile(sorted_ms: &[f64], p: f64) -> f64 {
    let idx = ((sorted_ms.len() as f64 - 1.0) * p).round() as usize;
    sorted_ms[idx]
}

#[tokio::test]
async fn cold_select_at_100k_rows() -> anyhow::Result<()> {
    let Some(pool) = connect().await? else {
        return Ok(());
    };
    seed(&pool).await?;
    let result = run(&pool).await;
    teardown(&pool).await;
    result
}

async fn run(pool: &PgPool) -> anyhow::Result<()> {
    let mut samples_ms: Vec<f64> = Vec::with_capacity(ITERATIONS);
    let mut rows_returned: i64 = 0;
    let stmt = "SELECT id, name FROM bench_100k_docs WHERE kb_id = $1 AND kind = $2";

    // 第一次不算——planner 第一发的 prepare 成本被算进冷路径，但冷与暖
    // 的差别正是想量出来的，混在一起就把信号冲淡了。
    let _ = sqlx::query_as::<_, (i64, String)>(stmt)
        .bind(1_i64)
        .bind(0_i16)
        .fetch_all(pool)
        .await?;

    for i in 0..ITERATIONS {
        let kb = (i as i64).wrapping_mul(0x9E37_79B9) % 4 + 1;
        let k = ((i as i64).wrapping_mul(0x1000_0001) % 8) as i16;
        let started = Instant::now();
        let rows = sqlx::query_as::<_, (i64, String)>(stmt)
            .bind(kb)
            .bind(k)
            .fetch_all(pool)
            .await?;
        let elapsed = started.elapsed().as_secs_f64() * 1000.0;
        samples_ms.push(elapsed);
        rows_returned += rows.len() as i64;
    }

    samples_ms.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let mean = samples_ms.iter().sum::<f64>() / samples_ms.len() as f64;
    let throughput = 1000.0 / mean;

    println!();
    println!("| scenario     | n  | p50 (ms) | p95 (ms) | p99 (ms) | mean (ms) | throughput |");
    println!("|--------------|----|----------|----------|----------|-----------|------------|");
    println!(
        "| cold_select  | {:2} |  {:7.2} |  {:7.2} |  {:7.2} |   {:6.2}  | {:6.1} q/s |",
        ITERATIONS,
        percentile(&samples_ms, 0.50),
        percentile(&samples_ms, 0.95),
        percentile(&samples_ms, 0.99),
        mean,
        throughput
    );
    println!();
    println!("rows_returned_total = {rows_returned}");

    Ok(())
}
