//! 迁移 0114：这一列之前，#975 的两条路已经把终点挪到了时段的尽头，行上却没记下原文说的是
//! 哪个时段。照物化的来路造行，再把这一列抹掉——那正是它出现之前的样子。回填的每条规则一行：
//!
//! - 标上，都关在时段的尽头：Lin「works for … from 2024-03-01」再「left in 2024」（收尾那条路）；
//!   Mei 两句反过来（精化那条路）；Ada 规则读的「was on contract with … until 2024」（蕴含行）；
//!   Zhou「from 2023-06-01」加「left on 2023-06-01」（同一天开始又结束）；Tao 同 Lin，起点人挪到
//!   了 4 月（还在 2024 年里）
//! - 不标：Qian「joined in 2024」（标 start，大小写不同存成了两条）加「until 2025」，到 2025 年
//!   为止；joined 的绑定翻成 end 之后也不标——起点只来自这两条，同一句话不算另有陈述。Sun 的
//!   终点人改过；Wu 是人写的，没有来源陈述
//!
//! 第二遍在绑定翻转之后跑，此外什么都不改。迁移的语句不按 kb 过滤，改的是全库的行，所以这个
//! 测试在 tests/ 顶层自成一个二进制（与 0099、0101 同理）。没有 `UTOPIA_DATABASE_URL` 时跳过
//! 而不是失败。自建自拆，绝不碰已有的库。

use sqlx::PgPool;
use uuid::Uuid;

const MIGRATION: &str =
    include_str!("../../../migrations/0114_an_end_that_names_a_period_says_so.sql");

#[tokio::test]
async fn rows_whose_end_was_moved_before_the_mark_are_marked_once() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let [org, ws, kb, person, company, works_for, meridian, rule] =
        std::array::from_fn(|_| Uuid::now_v7());
    let people =
        ["Lin", "Mei", "Ada", "Zhou", "Qian", "Sun", "Wu", "Tao"].map(|n| (n, Uuid::now_v7()));
    let entities = people.map(|(name, id)| format!("('{id}', '{kb}', '{person}', '{name}')"));
    let [lin, mei, ada, zhou, qian, sun, wu, tao] = people.map(|(_, id)| id);
    // 开放陈述，两端按时间解析写的形状：「自」只有起点，「至」只有终点，一刻两端同值。物化按
    // id 的先后读，id 照这里的顺序取
    let from = |at: &str, p: &str| format!("'{at}T00:00:00Z', '{p}', NULL, NULL");
    let until = |at: &str, p: &str| format!("NULL, NULL, '{at}T00:00:00Z', '{p}'");
    let on = |at: &str, p: &str| format!("'{at}T00:00:00Z', '{p}', '{at}T00:00:00Z', '{p}'");
    let said = [
        (lin, "works for", from("2024-03-01", "day")),
        (lin, "left", on("2024-01-01", "year")),
        (mei, "left", on("2024-01-01", "year")),
        (mei, "works for", from("2024-03-01", "day")),
        (ada, "works for", from("2024-03-01", "day")),
        (ada, "was on contract with", until("2024-01-01", "year")),
        (zhou, "works for", from("2023-06-01", "day")),
        (zhou, "left", on("2023-06-01", "day")),
        (qian, "joined", on("2024-01-01", "year")),
        (qian, "Joined", on("2024-01-01", "year")),
        (qian, "works for", until("2025-01-01", "year")),
        (sun, "works for", from("2024-03-01", "day")),
        (sun, "left", on("2024-01-01", "year")),
        (tao, "works for", from("2024-03-01", "day")),
        (tao, "left", on("2024-01-01", "year")),
    ]
    .map(|(who, phrase, span)| {
        let id = Uuid::now_v7();
        format!("('{id}', '{kb}', '{who}', '{meridian}', 'open', '{phrase}', 0.9, {span})")
    });
    // Only locally generated UUIDs and the literals above are interpolated into fixture SQL.
    sqlx::raw_sql(&format!(
        "INSERT INTO organizations (id, name) VALUES ('{org}', 'an-end-before-the-mark');
         INSERT INTO workspaces (id, org_id, name) VALUES ('{ws}', '{org}', 'an-end-before-the-mark');
         INSERT INTO knowledge_bases (id, workspace_id, name) VALUES ('{kb}', '{ws}', 'an-end-before-the-mark');
         INSERT INTO entity_types (id, kb_id, key, label, color, shape) VALUES
             ('{person}', '{kb}', 'person', 'person', '#7fd0ff', 'circle'),
             ('{company}', '{kb}', 'organization', 'organization', '#7fd0ff', 'circle');
         INSERT INTO relation_types (id, kb_id, key, label, temporal)
              VALUES ('{works_for}', '{kb}', 'works_for', 'works for', 'state');
         INSERT INTO entities (id, kb_id, type_id, canonical_name) VALUES
             ('{meridian}', '{kb}', '{company}', 'Meridian Systems'), {entities};
         INSERT INTO phrase_bindings
             (id, kb_id, phrase, subject_type_id, object_type_id, object_is_value,
              relation_type_id, direction, status, marks) VALUES
             (gen_random_uuid(), '{kb}', 'works for', '{person}', '{company}', false,
              '{works_for}', 'forward', 'bound', NULL),
             (gen_random_uuid(), '{kb}', 'left', '{person}', '{company}', false,
              '{works_for}', 'forward', 'bound', 'end'),
             (gen_random_uuid(), '{kb}', 'joined', '{person}', '{company}', false,
              '{works_for}', 'forward', 'bound', 'start');
         INSERT INTO implication_rules (id, kb_id, trigger, phrase, subject_type_id,
                                        object_type_id, conclude_property_id, reading,
                                        status, decided_by)
              VALUES ('{rule}', '{kb}', 'phrase', 'was on contract with', '{person}',
                      '{company}', '{works_for}', NULL, 'approved', 'person');
         INSERT INTO facts (id, kb_id, subject_id, object_id, layer, phrase, confidence,
                            valid_from, valid_from_precision, valid_to, valid_to_precision)
              VALUES {said};",
        entities = entities.join(", "),
        said = said.join(", "),
    ))
    .execute(&pool)
    .await?;

    let run = async {
        utopia_store::materialize::materialize(&pool, kb).await?;
        // 这一列之前的样子；Sun 的终点人改过，Tao 的起点人改过；Wu 是人写的一行，同样关在
        // 2025-01-01
        sqlx::raw_sql(&format!(
            "UPDATE facts SET end_after_period = FALSE WHERE kb_id = '{kb}';
             UPDATE facts SET corrected_ends = 'end'
              WHERE subject_id = '{sun}' AND layer = 'typed' AND invalidated_at IS NULL;
             UPDATE facts SET corrected_ends = 'start', valid_from = '2024-04-01T00:00:00Z'
              WHERE subject_id = '{tao}' AND layer = 'typed' AND invalidated_at IS NULL;
             INSERT INTO facts (id, kb_id, subject_id, predicate_id, object_id, layer, confidence,
                                valid_from, valid_from_precision, valid_to, valid_to_precision)
                  VALUES ('{}', '{kb}', '{wu}', '{works_for}', '{meridian}', 'typed', 0.9,
                          '2024-03-01T00:00:00Z', 'day', '2025-01-01T00:00:00Z', 'year');",
            Uuid::now_v7()
        ))
        .execute(&pool)
        .await?;
        sqlx::raw_sql(MIGRATION).execute(&pool).await?;
        // joined 的绑定后来翻成了 end：已经算出的那行留着（0053），回填读的是绑定现在的值
        sqlx::query(
            "UPDATE phrase_bindings SET marks = 'end' WHERE kb_id = $1 AND phrase = 'joined'",
        )
        .bind(kb)
        .execute(&pool)
        .await?;
        let again = sqlx::raw_sql(MIGRATION).execute(&pool).await?;
        // 每人活着的那一行类型化事实：终点那天、标记
        let rows: Vec<(String, String, bool)> = sqlx::query_as(
            "SELECT e.canonical_name, to_char(f.valid_to AT TIME ZONE 'UTC', 'YYYY-MM-DD'),
                    f.end_after_period
               FROM facts f JOIN entities e ON e.id = f.subject_id
              WHERE f.kb_id = $1 AND f.layer = 'typed' AND f.invalidated_at IS NULL
              ORDER BY 1",
        )
        .bind(kb)
        .fetch_all(&pool)
        .await?;
        let expected = [
            ("Ada", "2025-01-01", true),
            ("Lin", "2025-01-01", true),
            ("Mei", "2025-01-01", true),
            ("Qian", "2025-01-01", false),
            ("Sun", "2025-01-01", false),
            ("Tao", "2025-01-01", true),
            ("Wu", "2025-01-01", false),
            ("Zhou", "2023-06-02", true),
        ]
        .map(|(name, end, marked)| (name.to_string(), end.to_string(), marked));
        assert_eq!((rows, again.rows_affected()), (expected.to_vec(), 0));
        anyhow::Ok(())
    }
    .await;

    sqlx::query("DELETE FROM organizations WHERE id = $1")
        .bind(org)
        .execute(&pool)
        .await?;
    run
}
