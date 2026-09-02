//! 审核队列的**真实条数**。
//!
//! 从前左栏的徽标读的是接口返回的数组长度，而接口固定只回 100 条——于是一个
//! 有 164 条低置信事实的库，界面写着 100。清完那 100 条，剩下的 64 条会再冒
//! 出来，看起来像凭空长的。
//!
//! **数数和取数是两件事，得分开做。** 取数有上限（一页十条，翻页拿下一页），
//! 数数没有：`count(*)` 走的是与列表同一套 WHERE，索引也是同一条。
//!
//! 八个 COUNT 合成一条查询而不是发八次：它们都在同一个 kb 上，一次往返把
//! 左栏一次性填满，而分开发会让切换知识库时左栏一档一档地跳出来。

use sqlx::PgPool;
use utopia_core::models::ReviewCounts;
use utopia_core::AppResult;
use uuid::Uuid;

/// 低置信的阈值。**与 `review_routes` 共用一个常量**——两处各写一个数，
/// 迟早分叉成「徽标说 12 条，点进去 9 条」。
pub const LOW_CONFIDENCE_BELOW: f32 = 0.75;

pub async fn counts(pool: &PgPool, kb_id: Uuid) -> AppResult<ReviewCounts> {
    Ok(sqlx::query_as(
        "SELECT
           (SELECT count(*) FROM pending_facts WHERE kb_id = $1) AS pending,
           (SELECT count(*) FROM resolution_reviews
             WHERE kb_id = $1 AND status = 'pending') AS duplicates,
           (SELECT count(*) FROM fact_conflicts
             WHERE kb_id = $1 AND status = 'open') AS conflicts,
           -- 待确认 = 有证据、但证据所在的分块全被新版本取代了
           (SELECT count(*) FROM facts f
             WHERE f.kb_id = $1 AND f.invalidated_at IS NULL
               AND EXISTS (SELECT 1 FROM fact_evidence fe WHERE fe.fact_id = f.id)
               AND NOT EXISTS (SELECT 1 FROM fact_evidence fe
                                 JOIN chunks c ON c.id = fe.chunk_id
                                WHERE fe.fact_id = f.id
                                  AND c.superseded_at IS NULL)) AS unconfirmed,
           (SELECT count(*) FROM facts
             WHERE kb_id = $1 AND invalidated_at IS NULL
               AND confidence < $2 AND derived_by_rule IS NULL) AS lowconf,
           (SELECT count(*) FROM concept_mappings
             WHERE kb_id = $1 AND status = 'proposed') AS mappings,
           (SELECT count(*) FROM axiom_violations
             WHERE kb_id = $1 AND status = 'open') AS violations,
           (SELECT count(*) FROM ontology_defects
             WHERE kb_id = $1 AND status = 'open') AS defects,
           (SELECT count(*) FROM entity_merges WHERE kb_id = $1) AS merges",
    )
    .bind(kb_id)
    .bind(LOW_CONFIDENCE_BELOW)
    .fetch_one(pool)
    .await?)
}
