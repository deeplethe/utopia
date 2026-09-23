//! 0070 的 DDL 不许依赖会话的 search_path：`CREATE FUNCTION`、
//! `CREATE TRIGGER ... ON`、`EXECUTE FUNCTION` 全部限定到 `public.*`——
//! pg_restore 会把会话 search_path 置空再灌数据，首位被占住的会话也不能
//! 把函数建到别的 schema 去。落在别处的触发器等于没有触发器。
//!
//! 三种会话下逐个验证：
//!   - 正常 search_path（默认）→ 装上；
//!   - `SET LOCAL search_path = ''` → 同样装上，函数落在 public；
//!   - `SET LOCAL search_path = 'decoy'`（先在首位摆上同名干扰物）→
//!     照样装进 public，干扰物一个不被调用。
//!
//! 没有 `UTOPIA_DATABASE_URL` 时跳过；设了地址却连不上、或建不了库——**失败**，
//! 不是跳过：地址都给了还说「没库」是假话，那个绿色等于这条检查没跑过。

use sqlx::{Acquire, PgPool};
use std::collections::{HashMap, HashSet};
use uuid::Uuid;

fn admin_url() -> Option<String> {
    let url = utopia_store::test_db::url()?;
    let (head, _) = url.rsplit_once('/')?;
    Some(format!("{head}/postgres"))
}

/// 按文件顺序跑 ≤ `through` 的迁移，各自一个事务（与 sqlx::migrate 同一形状）
async fn migrate_to(pool: &PgPool, through: i64) -> anyhow::Result<()> {
    let migrator = sqlx::migrate!("../../migrations");
    let mut conn = pool.acquire().await?;
    for m in migrator.iter().filter(|m| m.version <= through) {
        let mut tx = conn.begin().await?;
        sqlx::raw_sql(&m.sql).execute(&mut *tx).await?;
        tx.commit().await?;
    }
    Ok(())
}

/// 在 `search_path` 为 `path` 的事务里跑 0070 本体
async fn migration_70_under(pool: &PgPool, path: &str) -> Result<(), sqlx::Error> {
    let migrator = sqlx::migrate!("../../migrations");
    let m = migrator
        .iter()
        .find(|m| m.version == 70)
        .expect("0070 必须在迁移集里");
    let mut conn = pool.acquire().await?;
    let mut tx = conn.begin().await?;
    sqlx::query(&format!("SET LOCAL search_path = {path}"))
        .execute(&mut *tx)
        .await?;
    let r = sqlx::raw_sql(&m.sql).execute(&mut *tx).await;
    match r {
        Ok(_) => tx.commit().await,
        Err(e) => {
            let _ = tx.rollback().await;
            Err(e)
        }
    }
}

/// 隔离库：建 → 迁到 0069 → 返回（库名, 连接池）。
/// 跳过只有一种情形：**根本没设** `UTOPIA_DATABASE_URL`。设了地址连不上、
/// 建不了库、迁移链跑不动，全都以错误返回——给了地址却拿不到库，这次检查
/// 就是没有执行过，不该被记成绿色
async fn scratch(suffix: &str) -> anyhow::Result<Option<(String, PgPool)>> {
    let Some(admin) = admin_url() else {
        return Ok(None);
    };
    let admin_pool = PgPool::connect(&admin).await?;
    let name = format!("xkb70sp_{}_{}", suffix, Uuid::now_v7().simple());
    sqlx::query(&format!("CREATE DATABASE {name}"))
        .execute(&admin_pool)
        .await?;
    admin_pool.close().await;
    let Some(url) = utopia_store::test_db::url() else {
        drop_scratch(&name).await;
        return Ok(None);
    };
    let (head, _) = url.rsplit_once('/').expect("UTOPIA_DATABASE_URL 缺库名段");
    let pool = PgPool::connect(&format!("{head}/{name}")).await?;
    if let Err(e) = migrate_to(&pool, 69).await {
        pool.close().await;
        drop_scratch(&name).await;
        return Err(e);
    }
    Ok(Some((name, pool)))
}

async fn drop_scratch(name: &str) {
    if let Some(admin) = admin_url() {
        if let Ok(pool) = PgPool::connect(&admin).await {
            let _ = sqlx::query(&format!("DROP DATABASE IF EXISTS {name} WITH (FORCE)"))
                .execute(&pool)
                .await;
            pool.close().await;
        }
    }
}

/// 在隔离库上跑 `f`——不管成功、失败还是断言 panic，库都清掉再走：
/// 测失败的证据不许靠运维去捡烂尾库
async fn with_scratch<Fut>(suffix: &str, f: impl FnOnce(PgPool) -> Fut) -> anyhow::Result<()>
where
    Fut: std::future::Future<Output = anyhow::Result<()>>,
{
    use futures_util::FutureExt;
    use std::panic::AssertUnwindSafe;
    let Some((name, pool)) = scratch(suffix).await? else {
        return Ok(());
    };
    // catch_unwind：断言 panic 落在 Err 上，清库照常走到再重抛
    let r = AssertUnwindSafe(f(pool.clone())).catch_unwind().await;
    pool.close().await;
    drop_scratch(&name).await;
    match r {
        Ok(inner) => inner,
        Err(p) => std::panic::resume_unwind(p),
    }
}

/// 装完后要点名的几件东西：声明式边 26 条复合外键、父行作证边 10 个触发器、
/// 库不可过户 12 个触发器、支撑唯一约束 8 条，且所有新函数都在 public schema。
/// 同表自指边的提交边界检查由 DEFERRABLE INITIALLY DEFERRED 复合外键承担
/// （supersedes / from_statement_id / inverse_of / sub_property_of）
async fn assert_installed(pool: &PgPool) -> anyhow::Result<()> {
    let fns: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM pg_proc p JOIN pg_namespace n ON n.oid = p.pronamespace
          WHERE n.nspname = 'public' AND p.proname IN (
            'fact_evidence_stays_inside_its_kb',
            'derivation_premise_stays_inside_its_kb','qualifier_stays_inside_its_facts_kb',
            'type_parent_stays_inside_its_kb',
            'relation_scope_stays_inside_its_kb',
            'relation_qualifier_stays_inside_the_kb',
            'rule_condition_refs_stay_inside_the_kb','kb_ownership_is_not_reassigned',
            'typed_source_stays_inside_its_kb',
            'squalifier_stays_inside_its_facts_kb')",
    )
    .fetch_one(pool)
    .await?;
    assert_eq!(fns, 10, "全部触发器函数都必须落在 public schema");

    // 复合外键：每条源行自带 kb_id 的边一条，26 条
    let fks: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM pg_constraint WHERE contype = 'f' AND conname IN (
           'chunks_document_same_kb',
           'facts_subject_same_kb','facts_object_same_kb','facts_predicate_same_kb',
           'facts_supersedes_same_kb','facts_from_statement_same_kb',
           'derived_facts_subject_same_kb','derived_facts_object_same_kb',
           'derived_facts_predicate_same_kb','derived_facts_rule_same_kb',
           'derived_facts_attribute_rule_same_kb',
           'entities_type_same_kb',
           'entity_type_disjoint_a_same_kb','entity_type_disjoint_b_same_kb',
           'relation_types_inverse_same_kb','relation_types_sub_property_same_kb',
           'rules_predicate_same_kb',
           'attribute_rules_subject_type_same_kb','attribute_rules_conclude_type_same_kb',
           'attribute_rules_conclude_predicate_same_kb',
           'time_mentions_fact_same_kb','time_mentions_chunk_same_kb',
           'type_bindings_type_same_kb',
           'phrase_bindings_subject_type_same_kb','phrase_bindings_object_type_same_kb',
           'phrase_bindings_relation_type_same_kb')",
    )
    .fetch_one(pool)
    .await?;
    assert_eq!(fks, 26, "每条声明式边一个复合外键，一条都不能少");

    // 支撑唯一约束：被引表一家一个 (kb_id, id)
    let uniques: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM pg_constraint WHERE contype = 'u' AND conname IN (
           'documents_kb_id_key','chunks_kb_id_key','entities_kb_id_key',
           'entity_types_kb_id_key','relation_types_kb_id_key','facts_kb_id_key',
           'rules_kb_id_key','attribute_rules_kb_id_key')",
    )
    .fetch_one(pool)
    .await?;
    assert_eq!(uniques, 8, "每个被引表一条 (kb_id, id) 唯一约束");

    // 同表自指边真的递延：condeferrable 与 condeferred 两个位都立着
    let deferred: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM pg_constraint
          WHERE contype = 'f' AND condeferrable AND condeferred
            AND conname IN ('facts_supersedes_same_kb',
                            'facts_from_statement_same_kb',
                            'relation_types_inverse_same_kb',
                            'relation_types_sub_property_same_kb')",
    )
    .fetch_one(pool)
    .await?;
    assert_eq!(deferred, 4, "同表自指边的提交边界检查必须在场");

    // 父行作证的边与库不可过户：触发器计数（FK 自带的内部触发器不算）
    let trg: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM pg_trigger WHERE NOT tgisinternal AND tgname IN (
           'fact_evidence_same_kb','fact_derivations_same_kb',
           'fact_qualifiers_same_kb','entity_type_parents_same_kb',
           'relation_type_domains_same_kb','relation_type_ranges_same_kb',
           'relation_type_qualifiers_same_kb','attribute_rule_conditions_same_kb',
           'typed_fact_sources_same_kb','statement_qualifiers_same_kb',
           'facts_keep_their_kb','derived_facts_keep_their_kb','documents_keep_their_kb',
           'entities_keep_their_kb','entity_types_keep_their_kb','relation_types_keep_their_kb',
           'rules_keep_their_kb','attribute_rules_keep_their_kb',
           'entity_type_disjoint_keep_their_kb',
           'time_mentions_keep_their_kb','type_bindings_keep_their_kb',
           'phrase_bindings_keep_their_kb')",
    )
    .fetch_one(pool)
    .await?;
    assert_eq!(trg, 22, "父行作证的边与不可过户，一条都不能少");
    Ok(())
}

// =====================================================================
// 完备性守卫:0070 的覆盖面不靠「数过的约束名/触发器名」维持——那种数法
// 会在有人往账本里加了一条引用列、却没人回来改 0070 的时候保持全绿。
// 这里反过来:从 pg_catalog 现数责任面里的每一条列级引用边,逐条归类,
// 归不进任何一类就红;登记表里有而 catalog 里没有,也是红。
// =====================================================================

/// 0070 的责任面:语义账本的表——导出逐边解析引用的那些。
/// 这是范围声明而不是覆盖断言:一张新表要进账本,得先把表名登记进来,
/// 它的引用边才轮到逐条归类;面外表(审计、暂存、运维)不归这条不变量管。
const LEDGER_TABLES: &[&str] = &[
    // 自带 kb_id 的语义行
    "attribute_rules",
    "chunks",
    "derived_facts",
    "documents",
    "entities",
    "entity_type_disjoint",
    "entity_types",
    "facts",
    "phrase_bindings",
    "relation_types",
    "rules",
    "time_mentions",
    "type_bindings",
    // 行自己没有 kb_id、kb 权威由父行作证的连接行
    "attribute_rule_conditions",
    "entity_type_parents",
    "fact_derivations",
    "fact_evidence",
    "fact_qualifiers",
    "relation_type_domains",
    "relation_type_qualifiers",
    "relation_type_ranges",
    "statement_qualifiers",
    "typed_fact_sources",
];

/// 父行作证表:(表, owner 列, owner 表)。owner 边给出该行的 kb 权威,
/// 它本身不是「同库语义引用」,自动归 EXPLICIT_NON_SCOPE;其余指向带
/// kb_id 表的边都必须登记到 TRIGGER_EDGES。
const OWNER_ROWS: &[(&str, &str, &str)] = &[
    ("attribute_rule_conditions", "rule_id", "attribute_rules"),
    ("entity_type_parents", "child_id", "entity_types"),
    ("fact_derivations", "derived_fact_id", "derived_facts"),
    ("fact_evidence", "fact_id", "facts"),
    ("fact_qualifiers", "fact_id", "facts"),
    (
        "relation_type_domains",
        "relation_type_id",
        "relation_types",
    ),
    (
        "relation_type_qualifiers",
        "relation_type_id",
        "relation_types",
    ),
    ("relation_type_ranges", "relation_type_id", "relation_types"),
    ("statement_qualifiers", "fact_id", "facts"),
    ("typed_fact_sources", "fact_id", "facts"),
];

/// 触发器边登记:(表, 列, 触发器名)。登记是列级的——「这张表装着触发器」
/// 不蕴含「新加的列被它检查」:除登记外,还核对列落在该触发器的
/// UPDATE OF 清单里。表上新增一条指向带 kb_id 表的引用列而不登记,
/// 就归不进任何一类。
const TRIGGER_EDGES: &[(&str, &str, &str)] = &[
    (
        "attribute_rule_conditions",
        "predicate_id",
        "attribute_rule_conditions_same_kb",
    ),
    (
        "entity_type_parents",
        "parent_id",
        "entity_type_parents_same_kb",
    ),
    (
        "fact_derivations",
        "premise_derived_id",
        "fact_derivations_same_kb",
    ),
    (
        "fact_derivations",
        "premise_fact_id",
        "fact_derivations_same_kb",
    ),
    ("fact_evidence", "chunk_id", "fact_evidence_same_kb"),
    ("fact_evidence", "document_id", "fact_evidence_same_kb"),
    ("fact_qualifiers", "entity_id", "fact_qualifiers_same_kb"),
    (
        "fact_qualifiers",
        "qualifier_type_id",
        "fact_qualifiers_same_kb",
    ),
    (
        "relation_type_domains",
        "entity_type_id",
        "relation_type_domains_same_kb",
    ),
    (
        "relation_type_qualifiers",
        "qualifier_type_id",
        "relation_type_qualifiers_same_kb",
    ),
    (
        "relation_type_ranges",
        "entity_type_id",
        "relation_type_ranges_same_kb",
    ),
    (
        "statement_qualifiers",
        "entity_id",
        "statement_qualifiers_same_kb",
    ),
    (
        "typed_fact_sources",
        "statement_id",
        "typed_fact_sources_same_kb",
    ),
];

/// 面内但不属同库语义引用的边——一条一句话理由。owner 边不列在这里,
/// 由 OWNER_ROWS 自动豁免。
const NON_SCOPE: &[(&str, &str, &str)] = &[
    // 采集来源归属:导出从不把这条引用解析成 IRI
    (
        "documents",
        "source_id",
        "ingest attribution, not an export-resolved reference",
    ),
    // 合并簿记:merged_into 非空的行不进导出,这条指针永不进 IRI
    (
        "entities",
        "merged_into",
        "merge bookkeeping — merged rows never reach export",
    ),
];

/// pg_catalog 里数出来的一条列级引用边。`pairs` 是同一约束按
/// conkey/confkey 序数对齐出的全部列对——一条复合外键给出多行边,
/// 它们共享同一份 pairs。
struct RefEdge {
    src_table: String,
    src_col: String,
    tgt_table: String,
    tgt_col: String,
    conname: String,
    deferrable: bool,
    deferred: bool,
    pairs: Vec<(String, String)>,
}

impl RefEdge {
    fn label(&self) -> String {
        format!(
            "{}.{}\u{2192}{}.{}",
            self.src_table, self.src_col, self.tgt_table, self.tgt_col
        )
    }
}

/// 从 pg_catalog 现数 public schema 的全部外键列对——不读迁移源码,
/// conkey/confkey 按序数对齐,不靠数组位置猜。
async fn catalog_edges(pool: &PgPool) -> anyhow::Result<Vec<RefEdge>> {
    let rows = sqlx::query_as::<_, (i64, String, String, String, String, String, bool, bool)>(
        "SELECT c.oid::bigint,
                src.relname, sa.attname, tgt.relname, ta.attname,
                c.conname, c.condeferrable, c.condeferred
           FROM pg_constraint c
           JOIN pg_class src ON src.oid = c.conrelid
           JOIN pg_namespace sn
             ON sn.oid = src.relnamespace AND sn.nspname = 'public'
           JOIN pg_class tgt ON tgt.oid = c.confrelid
           JOIN pg_namespace tn
             ON tn.oid = tgt.relnamespace AND tn.nspname = 'public'
           JOIN unnest(c.conkey) WITH ORDINALITY AS ck(attnum, ord) ON true
           JOIN pg_attribute sa
             ON sa.attrelid = c.conrelid AND sa.attnum = ck.attnum
           JOIN pg_attribute ta
             ON ta.attrelid = c.confrelid AND ta.attnum = c.confkey[ck.ord]
          WHERE c.contype = 'f'
          ORDER BY c.oid, ck.ord",
    )
    .fetch_all(pool)
    .await?;
    let mut pairs: HashMap<i64, Vec<(String, String)>> = HashMap::new();
    for (oid, _, sc, _, tc, _, _, _) in &rows {
        pairs
            .entry(*oid)
            .or_default()
            .push((sc.clone(), tc.clone()));
    }
    Ok(rows
        .into_iter()
        .map(|(oid, st, sc, tt, tc, cn, dfr, dfd)| RefEdge {
            pairs: pairs.get(&oid).cloned().unwrap_or_default(),
            src_table: st,
            src_col: sc,
            tgt_table: tt,
            tgt_col: tc,
            conname: cn,
            deferrable: dfr,
            deferred: dfd,
        })
        .collect())
}

/// 带 kb_id 列的表 = 有库归属的行。
async fn kb_scoped_tables(pool: &PgPool) -> anyhow::Result<HashSet<String>> {
    Ok(sqlx::query_scalar::<_, String>(
        "SELECT c.relname FROM pg_class c
           JOIN pg_namespace n ON n.oid = c.relnamespace
          WHERE n.nspname = 'public' AND c.relkind = 'r'
            AND EXISTS (SELECT 1 FROM pg_attribute a
                         WHERE a.attrelid = c.oid AND a.attname = 'kb_id'
                           AND a.attnum > 0 AND NOT a.attisdropped)",
    )
    .fetch_all(pool)
    .await?
    .into_iter()
    .collect())
}

/// 用户触发器 -> 它 UPDATE OF 盯着的列名集合(tgattr)。
async fn trigger_watch_lists(
    pool: &PgPool,
) -> anyhow::Result<HashMap<(String, String), HashSet<String>>> {
    let rows = sqlx::query_as::<_, (String, String, Vec<String>)>(
        "SELECT cls.relname, t.tgname, COALESCE(w.cols, '{}'::text[])
           FROM pg_trigger t
           JOIN pg_class cls ON cls.oid = t.tgrelid
           JOIN pg_namespace n ON n.oid = cls.relnamespace AND n.nspname = 'public'
           LEFT JOIN LATERAL (
               SELECT array_agg(a.attname) AS cols
                 FROM pg_attribute a
                WHERE a.attrelid = t.tgrelid
                  AND a.attnum = ANY (string_to_array(t.tgattr::text, ' ')::smallint[])
           ) w ON true
          WHERE NOT t.tgisinternal",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(t, g, c)| ((t, g), c.into_iter().collect()))
        .collect())
}

/// 身上装着 0070 机制的表(同库约束/触发器、不可过户触发器、支撑唯一约束)——
/// 用来与 LEDGER_TABLES 互证:机制落在面外、或面内表什么机制都没有,都算漂移。
async fn mechanism_tables(pool: &PgPool) -> anyhow::Result<HashSet<String>> {
    Ok(sqlx::query_scalar::<_, String>(
        "SELECT cls.relname FROM pg_constraint c
           JOIN pg_class cls ON cls.oid = c.conrelid
           JOIN pg_namespace n ON n.oid = cls.relnamespace AND n.nspname = 'public'
          WHERE c.conname::text ~ '(_same_kb|_kb_id_key)$'
          UNION
         SELECT cls.relname FROM pg_trigger t
           JOIN pg_class cls ON cls.oid = t.tgrelid
           JOIN pg_namespace n ON n.oid = cls.relnamespace AND n.nspname = 'public'
          WHERE NOT t.tgisinternal
            AND t.tgname::text ~ '(_same_kb|_keep_their_kb)$'",
    )
    .fetch_all(pool)
    .await?
    .into_iter()
    .collect())
}

/// 逐条归类的结果。declarative / trigger_covered / non_scope 之外
/// 任何一桶非空,覆盖就是不完备。
#[derive(Debug, Default)]
struct Coverage {
    declarative: Vec<String>,
    trigger_covered: Vec<String>,
    non_scope: Vec<String>,
    /// owner-derived 表上 catalog 有、登记没有的边
    unknown: Vec<String>,
    /// 登记/豁免/owner 声明在 catalog 里对不上的边
    stale_registry: Vec<String>,
    /// kb 自持行上没有合法复合外键的边
    unprotected_direct: Vec<String>,
    /// 登记过、但触发器没装上或没盯该列的边
    unprotected_trigger: Vec<String>,
    /// 责任面与已装机制对不上
    surface_drift: Vec<String>,
}

impl Coverage {
    /// 完备 = 没有未归类的边、没有腐掉的登记、面与机制互证得上,
    /// 且两类已覆盖边的数量与迁移记录的 26/13 一致。
    fn assert_complete(&self) -> anyhow::Result<()> {
        let mut problems = String::new();
        let mut dump = |name: &str, xs: &[String]| {
            for x in xs {
                problems.push_str(&format!("  {name}: {x}\n"));
            }
        };
        dump("UNKNOWN", &self.unknown);
        dump("STALE_REGISTRY", &self.stale_registry);
        dump("UNPROTECTED_DIRECT", &self.unprotected_direct);
        dump("UNPROTECTED_TRIGGER", &self.unprotected_trigger);
        dump("SURFACE_DRIFT", &self.surface_drift);
        if self.declarative.len() != 26 {
            problems.push_str(&format!(
                "  DECLARATIVE_EDGES = {} (expected 26)\n",
                self.declarative.len()
            ));
        }
        if self.trigger_covered.len() != 13 {
            problems.push_str(&format!(
                "  TRIGGER_EDGES = {} (expected 13)\n",
                self.trigger_covered.len()
            ));
        }
        if problems.is_empty() {
            Ok(())
        } else {
            Err(anyhow::anyhow!(
                "catalog-derived coverage is incomplete:\n{problems}"
            ))
        }
    }
}

/// 每条相关边归到恰好一类。相关 = 源表在责任面、引用列不是 kb_id
/// 本身、目标表带 kb_id——kb_id→knowledge_bases 这类容器边被
/// `src_col != kb_id` 自然挡在面外,复合外键里的 kb_id→kb_id 那一腿
/// 也只是绑定机制、不是一条独立语义引用。
fn classify(
    edges: &[RefEdge],
    kb_scoped: &HashSet<String>,
    triggers: &HashMap<(String, String), HashSet<String>>,
    mechanism: &HashSet<String>,
) -> Coverage {
    let ledger: HashSet<&str> = LEDGER_TABLES.iter().copied().collect();
    let owner: HashMap<&str, (&str, &str)> =
        OWNER_ROWS.iter().map(|(t, c, p)| (*t, (*c, *p))).collect();
    let registry: HashMap<(&str, &str), &str> = TRIGGER_EDGES
        .iter()
        .map(|(t, c, g)| ((*t, *c), *g))
        .collect();
    let exemptions: HashMap<(&str, &str), &str> =
        NON_SCOPE.iter().map(|(t, c, r)| ((*t, *c), *r)).collect();
    let mut cov = Coverage::default();

    for t in mechanism.iter().filter(|t| !ledger.contains(t.as_str())) {
        cov.surface_drift.push(format!(
            "{t}: carries 0070 mechanism but is outside the declared surface"
        ));
    }
    for t in ledger.iter().filter(|t| !mechanism.contains(**t)) {
        cov.surface_drift.push(format!(
            "{t}: declared in the surface but carries no 0070 mechanism"
        ));
    }
    for (t, _, _) in OWNER_ROWS {
        if kb_scoped.contains(*t) {
            cov.surface_drift.push(format!(
                "{t}: owner-derived row grew its own kb_id — family changed"
            ));
        }
    }
    for t in ledger.iter().filter(|t| !owner.contains_key(**t)) {
        if !kb_scoped.contains(*t) {
            cov.surface_drift.push(format!(
                "{t}: direct-kb surface table lost its kb_id column"
            ));
        }
    }

    // 登记三个方向的腐化:owner 边、触发器边、豁免边在 catalog 里都得还在
    for (t, col, parent) in OWNER_ROWS {
        if !edges
            .iter()
            .any(|e| e.src_table == *t && e.src_col == *col && e.tgt_table == *parent)
        {
            cov.stale_registry.push(format!(
                "{t}.{col}\u{2192}{parent}: declared owner edge absent from catalog"
            ));
        }
    }
    for (t, c, g) in TRIGGER_EDGES {
        if !edges.iter().any(|e| e.src_table == *t && e.src_col == *c) {
            cov.stale_registry.push(format!(
                "{t}.{c} ({g}): registered trigger edge absent from catalog"
            ));
        }
    }
    for (t, c, _) in NON_SCOPE {
        if !edges.iter().any(|e| e.src_table == *t && e.src_col == *c) {
            cov.stale_registry
                .push(format!("{t}.{c}: exempted edge absent from catalog"));
        }
    }

    for e in edges.iter().filter(|e| {
        ledger.contains(e.src_table.as_str())
            && e.src_col != "kb_id"
            && kb_scoped.contains(e.tgt_table.as_str())
    }) {
        let label = e.label();
        if let Some((owner_col, owner_table)) = owner.get(e.src_table.as_str()) {
            if e.src_col == *owner_col {
                if e.tgt_table == *owner_table {
                    cov.non_scope.push(format!("{label} (owner edge)"));
                } else {
                    cov.stale_registry.push(format!(
                        "{label}: owner column now points at {}, not {owner_table}",
                        e.tgt_table
                    ));
                }
            } else if let Some(tg) = registry.get(&(e.src_table.as_str(), e.src_col.as_str())) {
                match triggers.get(&(e.src_table.clone(), (*tg).to_string())) {
                    Some(cols) if cols.contains(&e.src_col) => cov.trigger_covered.push(label),
                    Some(_) => cov
                        .unprotected_trigger
                        .push(format!("{label}: trigger {tg} does not watch this column")),
                    None => cov
                        .unprotected_trigger
                        .push(format!("{label}: trigger {tg} is not installed")),
                }
            } else {
                cov.unknown.push(label);
            }
        } else if let Some(reason) = exemptions.get(&(e.src_table.as_str(), e.src_col.as_str())) {
            cov.non_scope.push(format!("{label} ({reason})"));
        } else {
            // 直接 kb 边:同一条约束内必须同时绑住 (kb_id→kb_id) 与 (ref→id);
            // 单列 FOREIGN KEY (ref) REFERENCES t(id) 不算同库覆盖
            let composite = e.tgt_col == "id"
                && e.pairs.len() == 2
                && e.pairs.iter().any(|(s, t)| s == "kb_id" && t == "kb_id");
            if !composite {
                cov.unprotected_direct.push(format!(
                    "{label} via {} — no composite (kb_id, ref) \u{2192} (kb_id, id)",
                    e.conname
                ));
            } else if e.src_table == e.tgt_table && !(e.deferrable && e.deferred) {
                cov.unprotected_direct.push(format!(
                    "{label} via {} — same-table self reference must be \
                     DEFERRABLE INITIALLY DEFERRED",
                    e.conname
                ));
            } else {
                cov.declarative.push(label);
            }
        }
    }
    cov
}

async fn classify_reference_edges(pool: &PgPool) -> anyhow::Result<Coverage> {
    let (edges, kb_scoped, triggers, mechanism) = tokio::try_join!(
        catalog_edges(pool),
        kb_scoped_tables(pool),
        trigger_watch_lists(pool),
        mechanism_tables(pool),
    )?;
    Ok(classify(&edges, &kb_scoped, &triggers, &mechanism))
}

#[tokio::test]
async fn every_reference_edge_on_the_ledger_is_classified() -> anyhow::Result<()> {
    with_scratch("cover", |pool| async move {
        migration_70_under(&pool, "public").await?;
        classify_reference_edges(&pool).await?.assert_complete()
    })
    .await
}

/// 漂移探针 A:kb 自持行上新增一条单列外键(不配 kb_id)。
/// 「26 个名字还在」挡不住它——catalog 会把它数出来,归不进任何一类。
#[tokio::test]
async fn a_new_reference_on_a_kb_owned_row_fails_the_guard() -> anyhow::Result<()> {
    with_scratch("driftd", |pool| async move {
        migration_70_under(&pool, "public").await?;
        sqlx::query(
            "ALTER TABLE public.time_mentions
             ADD COLUMN probe_ref uuid REFERENCES public.relation_types(id)",
        )
        .execute(&pool)
        .await?;
        let cov = classify_reference_edges(&pool).await?;
        assert!(
            cov.unprotected_direct
                .iter()
                .any(|e| e.starts_with("time_mentions.probe_ref\u{2192}")),
            "新加的单列引用必须落进 UNPROTECTED_DIRECT: {cov:?}"
        );
        assert!(cov.assert_complete().is_err());
        Ok(())
    })
    .await
}

/// 漂移探针 B:父行作证表上新增一条指向 kb 表的引用列,但不去碰它的
/// 触发器。「表上有触发器」不许让这条新列显得已被覆盖——登记是列级的。
#[tokio::test]
async fn a_new_reference_on_an_owner_derived_row_fails_the_guard() -> anyhow::Result<()> {
    with_scratch("driftt", |pool| async move {
        migration_70_under(&pool, "public").await?;
        sqlx::query(
            "ALTER TABLE public.fact_evidence
             ADD COLUMN probe_rel uuid REFERENCES public.relation_types(id)",
        )
        .execute(&pool)
        .await?;
        let cov = classify_reference_edges(&pool).await?;
        assert!(
            cov.unknown
                .iter()
                .any(|e| e.starts_with("fact_evidence.probe_rel\u{2192}")),
            "未登记的新边必须落进 UNKNOWN: {cov:?}"
        );
        assert!(cov.assert_complete().is_err());
        Ok(())
    })
    .await
}

#[tokio::test]
async fn the_migration_installs_under_an_empty_search_path() -> anyhow::Result<()> {
    with_scratch("empty", |pool| async move {
        let r = migration_70_under(&pool, "''").await;
        assert!(r.is_ok(), "search_path 为空时装得上 0070: {r:?}");
        assert_installed(&pool).await
    })
    .await
}

#[tokio::test]
async fn the_migration_installs_under_a_hostile_search_path() -> anyhow::Result<()> {
    with_scratch("evil", |pool| async move {
        // 首位摆个干扰 schema：同名的 entities 表与同名函数——裸名解析会先看到它。
        // 限定到 public.* 的 DDL 不该理会它
        sqlx::query("CREATE SCHEMA decoy").execute(&pool).await?;
        sqlx::query("CREATE TABLE decoy.entities (id uuid, kb_id uuid, merged_into uuid)")
            .execute(&pool)
            .await?;
        sqlx::query(
            "CREATE FUNCTION decoy.kb_ownership_is_not_reassigned() RETURNS trigger
             LANGUAGE plpgsql AS $$ BEGIN RETURN NULL; END; $$",
        )
        .execute(&pool)
        .await?;

        let r = migration_70_under(&pool, "decoy, public").await;
        assert!(r.is_ok(), "首位被占的 search_path 下也装得上 0070: {r:?}");
        assert_installed(&pool).await?;
        // 干扰物原样留着：一次都没被选中
        let decoy_fn: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM pg_proc p JOIN pg_namespace n ON n.oid = p.pronamespace
              WHERE n.nspname = 'decoy' AND p.proname = 'kb_ownership_is_not_reassigned'",
        )
        .fetch_one(&pool)
        .await?;
        assert_eq!(decoy_fn, 1, "decoy 函数不该被覆盖也不该被删掉");
        Ok(())
    })
    .await
}

#[tokio::test]
async fn the_migration_installs_under_a_normal_search_path() -> anyhow::Result<()> {
    with_scratch("norm", |pool| async move {
        let r = migration_70_under(&pool, "public").await;
        assert!(r.is_ok(), "正常 search_path 下装得上 0070: {r:?}");
        assert_installed(&pool).await
    })
    .await
}
