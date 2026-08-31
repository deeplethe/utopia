//! 抽取丢弃信号：哪些事实抽出来了却没能落地，以及为什么。
//!
//! 与 `ontology_misses` 分开是刻意的——那张表说的是"你的本体缺这些"，读者是
//! 本体维护者，动作是加类型；这张说的是"这些事实没落地"，读者是上传文档的人，
//! 动作是改文档或改本体。混在一个面板里两边都讲不清。
//!
//! 记录失败不影响抽取（调用方一律 `let _ =`）——信号缺一条，远好过因为记信号
//! 失败而中断整篇文档的抽取。

use sqlx::PgPool;
use utopia_core::models::ExtractionDrop;
use utopia_core::AppResult;
use uuid::Uuid;

/// 原因码。前端按这个查文案，所以是稳定契约，不要改字面量。
pub mod reason {
    /// 主语没在 entities 里声明 → 类型不明，属性无法校验 domain
    pub const SUBJECT_NOT_DECLARED: &str = "subject_not_declared";
    /// 属性挂在了不该挂的类上（salary 挂到 Organization）
    pub const ATTR_DOMAIN_MISMATCH: &str = "attr_domain_mismatch";
    /// 属性事实既没给 value 也没给 object
    pub const ATTR_NO_VALUE: &str = "attr_no_value";
    /// 值不合 datatype，归一化失败
    pub const ATTR_DATATYPE: &str = "attr_datatype";
    /// 模型自报置信度低于阈值
    pub const LOW_CONFIDENCE: &str = "low_confidence";
    /// 关系事实缺宾语
    pub const OBJECT_MISSING: &str = "object_missing";
    /// 模型给的这一条不合结构（缺 predicate 之类）→ 只跳这一条，不牵连整块
    pub const MALFORMED_ITEM: &str = "malformed_item";
    /// 主语的类型对不上关系声明的 domain，**且对调也不合法**——那是选错了关系
    /// 或类型判错，不是方向问题。照原样落库 + 记信号，交给人看，不猜
    pub const DOMAIN_MISMATCH: &str = "domain_mismatch";
    /// 模型给的"实体名"其实是一整句话或从句——不是一个东西的名字。
    /// 这类东西永远匹配不到别处的提及，在图上是孤点，还会拖累消解
    pub const NOT_AN_ENTITY_NAME: &str = "not_an_entity_name";
    /// 主语违反 domain 而宾语符合，已按本体声明的方向把主宾掰正。
    /// **动作必须留痕**：自动的、看不见的改写才是 0001 反对的那种
    pub const DIRECTION_CORRECTED: &str = "direction_corrected";
    /// 模型输出被截断（撞上 max_tokens）→ 已完整的那些留下，尾巴丢掉
    pub const TRUNCATED_REPLY: &str = "truncated_reply";
}

pub async fn record(
    pool: &PgPool,
    kb_id: Uuid,
    document_id: Uuid,
    reason: &str,
    detail: &str,
    example: Option<&str>,
) -> AppResult<()> {
    sqlx::query(
        "INSERT INTO extraction_drops (kb_id, document_id, reason, detail, example)
         VALUES ($1, $2, $3, left($4, 120), left($5, 200))
         ON CONFLICT (kb_id, document_id, reason, detail)
         DO UPDATE SET count = extraction_drops.count + 1,
                       example = COALESCE(EXCLUDED.example, extraction_drops.example),
                       updated_at = now()",
    )
    .bind(kb_id)
    .bind(document_id)
    .bind(reason)
    .bind(detail)
    .bind(example)
    .execute(pool)
    .await?;
    Ok(())
}

/// 重抽开始时清掉这篇文档的旧信号——本轮要从头讲一遍这篇文档的故事。
pub async fn clear_for_document(pool: &PgPool, document_id: Uuid) -> AppResult<()> {
    sqlx::query("DELETE FROM extraction_drops WHERE document_id = $1")
        .bind(document_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// 一个 KB 的全部丢弃信号。行数按 (文档 × 原因 × 具体对象) 聚合后很小，
/// 一次取回让 Library 既能算每篇的总数、又能直接展开详情，不必逐行发请求。
pub async fn for_kb(pool: &PgPool, kb_id: Uuid) -> AppResult<Vec<ExtractionDrop>> {
    Ok(sqlx::query_as(
        "SELECT document_id, reason, detail, count, example FROM extraction_drops
         WHERE kb_id = $1 ORDER BY count DESC, reason LIMIT 2000",
    )
    .bind(kb_id)
    .fetch_all(pool)
    .await?)
}
