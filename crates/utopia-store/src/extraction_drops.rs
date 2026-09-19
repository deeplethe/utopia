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
/// 都由开放抽取那条路（0044）发出；类型化那条路的原因码随它一起退场了
pub mod reason {
    /// 这一块整个没抽成：调用失败（端点不可用、请求没送到、等太久）或回复解析不了。
    ///
    /// **与别的原因码不同，它丢的是一整块原文，不是一条陈述。** 任务的 `last_error`
    /// 里也有这句话，可那是给排查的人看的；文档抽完之后还能看见「这一块没进去」的，
    /// 只有这张表（0001：丢弃是一行，不是沉默）
    pub const CHUNK_UNEXTRACTED: &str = "chunk_unextracted";
    /// 陈述缺宾语也缺值
    pub const OBJECT_MISSING: &str = "object_missing";
    /// 模型给的这一条不合结构（缺短语之类）→ 只跳这一条，不牵连整块
    pub const MALFORMED_ITEM: &str = "malformed_item";
    /// 模型输出被截断（撞上 max_tokens）→ 已完整的那些留下，尾巴丢掉
    pub const TRUNCATED_REPLY: &str = "truncated_reply";
    /// 这条陈述的值不在它自己的引文里（#729）：证据没写着这个数。
    ///
    /// 表格是这个错的产地——模型把一行五列压成五条只有值不同的陈述，引文却指向表上面
    /// 那句导语。五条里至多一条对，而图上没有任何东西分得出是哪一条。**引文是这条陈述
    /// 的全部依据**：值不在里面，它就不是文档说的
    pub const VALUE_NOT_IN_QUOTE: &str = "value_not_in_quote";
    /// 模型报的别名（或它的引文）不在这一块原文里（0041 决定 2）：不记这个名字。
    /// 名字是召回的桥，一座凭空的桥会把两个不相干的实体接到一起
    pub const NAME_NOT_IN_TEXT: &str = "name_not_in_text";
    /// 模型报的别名，这次回复（或本文档前面几块）里已经是另一个实体的名字（0041 决定 2）：
    /// 一个名字不会同时是两样东西的名字。「海探1项目」声明成了一个机构，就不是探测器的别名
    pub const NAME_CLAIMED_BY_ANOTHER: &str = "name_claimed_by_another";
    /// 宾语既没在回复里列出、库里也没有叫这个名字的东西：陈述照落，
    /// 宾语落成字面值而不是节点（#559）。记下来是为了量：这一类里有多少
    /// 本该是实体（模型漏报），有多少本来就是描述
    pub const OBJECT_UNDECLARED: &str = "object_undeclared";
    /// 模型的引文在这一块里找不到原样的一句：陈述照落、引文照记，只是没有偏移
    /// （`quote_start` / `quote_end` 留空）。这个信号数的是有多少条没定位到
    pub const QUOTE_NOT_IN_CHUNK: &str = "quote_not_in_chunk";
    /// 时间词不在它的引文里、也不在这一块里：不记这条时间提及。时间词是照抄的字，
    /// 抄不出来的字就不是文档说的
    pub const TIME_NOT_IN_QUOTE: &str = "time_not_in_quote";
    /// 陈述、属性或名字指着一个回复里不存在的实体编号或已知句柄：跳过这一条
    pub const UNKNOWN_REF: &str = "unknown_ref";
    /// 短语就是那个值（「director nominees —ten (10)→ ten (10)」）：表格丢了列头时模型
    /// 的写法之一。陈述照落（主语和值都在，信息没丢），记一笔让它可量。
    /// 纯结构判断：两串字去掉首尾空白后相同
    pub const PHRASE_IS_VALUE: &str = "phrase_is_value";
    /// 短语就是主语的名字（「实物商品网上零售额 —实物商品网上零售额→ 127878亿元」）：
    /// 同上，另一种写法，同样照落
    pub const PHRASE_IS_SUBJECT: &str = "phrase_is_subject";
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
