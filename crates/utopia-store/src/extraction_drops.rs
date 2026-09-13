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
    /// 模型在边上写了一个这条关系没声明过的属性 key（0037）
    pub const QUALIFIER_UNKNOWN: &str = "qualifier_unknown";
    /// 边上属性的值换不成它声明的 datatype
    pub const QUALIFIER_DATATYPE: &str = "qualifier_datatype";
    /// 同一条边再听到一次，属性值与已记的不一致——先记下，不覆盖
    pub const QUALIFIER_CONFLICT: &str = "qualifier_conflict";
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
    /// 守卫放行了、结构却像从句（限定词起头的长串、句中的关系词）。**只记不挡**：
    /// 实体照常落库，例句留下来——#193 要的是一份跨语料的标注集，再决定哪条升成硬规则
    pub const CLAUSE_SUSPECT: &str = "clause_suspect";
    /// 主语是「跟 X 有关的一群人」而写成了 X（#578）："former OpenAI personnel" 不是
    /// OpenAI。事实不落，例句是那个短语；这一类该由提示词的规则改写到有名字的一侧，
    /// 这里只量它还错多少
    pub const SUBJECT_SHORTENED: &str = "subject_shortened";
    /// 主宾片段不在引文里（#582）：模型没照抄。事实照旧处理，只记下来
    pub const SPAN_NOT_IN_QUOTE: &str = "span_not_in_quote";
    /// 主语片段是个描述，不是任何声明过的实体的名字：事实不落（#582，取代 #578 的词表）
    pub const SUBJECT_DESCRIBED: &str = "subject_described";
    /// 宾语片段是个描述：事实照落，宾语落成字面值（#582）
    pub const OBJECT_DESCRIBED: &str = "object_described";
    /// 片段点的是另一个声明过的实体：改绑到它（#582）
    pub const SPAN_REBOUND: &str = "span_rebound";
    /// 片段是所绑名字前面带了别的词（"entrepreneur Tasha McCauley" / "companies using
    /// OpenAI"）：头衔还是另一件东西，机器分不开，绑定照旧，只记（#582）
    pub const SPAN_PREFIXED: &str = "span_prefixed";
    /// 片段里没有任何声明过的名字（"him" / "the company"）：模型消解了指代，无从核对，
    /// 绑定照旧，只记（#582）
    pub const SPAN_COREFERENCE: &str = "span_coreference";
    /// 片段抄的是事实**另一侧**的名字（宾语片段写成了主语）：抄错了位置，不是绑错了
    /// 实体。绑定照旧，只记（#582）
    pub const SPAN_MISPLACED: &str = "span_misplaced";
    /// 宾语既没在 entities 里声明、库里也没有叫这个名字的东西：事实照落，
    /// 宾语落成字面值而不是节点（#559）。记下来是为了量：这一类里有多少
    /// 本该是实体（模型漏报），有多少本来就是描述
    pub const OBJECT_UNDECLARED: &str = "object_undeclared";
    /// 以下都来自 `utopia_extract::normalize`：只看结构、不看词的形状检查
    /// 值只有破折号（`—`）：表里的「无」，不落
    pub const NO_VALUE: &str = "no_value";
    /// 值后面有一截引文里没有的字：只留引文里有的那段。只记
    pub const VALUE_TRIMMED: &str = "value_trimmed";
    /// 没有宾语也没有值、只带边属性：属性落成主语上的值事实。只记
    pub const QUALIFIERS_WITHOUT_OBJECT: &str = "qualifiers_without_object";
    /// 宾语是契约格式的日期：数落成值、日期进有效期；没带数的把写出来的那段落成值。只记
    pub const TIME_AS_OBJECT: &str = "time_as_object";
    /// 主语是契约格式的日期：数是谁的回复里没说，不落
    pub const TIME_AS_SUBJECT: &str = "time_as_subject";
    /// 宾语名字包住另一个声明实体、同句已有指向本尊的边：可能是描述，也可能是另一个
    /// 东西（每股收益包住了净利润）。只记，不删
    pub const OBJECT_DESCRIBES_DECLARED: &str = "object_describes_declared";
    /// 上面几条去掉事实后没人引用的声明：不建
    pub const ORPHAN_DECLARATION: &str = "orphan_declaration";
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
