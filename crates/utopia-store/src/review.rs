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

/// 「待确认」的判据，写成 SQL 片段，`counts` 与总览（`review_summary`）共用：
/// 有证据、但证据所在的分块全被新版本取代了，**而且这条事实至今仍成立**。别名固定用 `f`。
///
/// 上界那一句是「闭合」这个出路的前提：闭合是作废+改写，修正行把证据一起复制下来
/// （`temporal::close_superseded`），少了它，刚闭合的行照样满足判据——队列永远清不掉，
/// 再点一次只会撞上「区间已闭合」。判据与 0022 说同一句话：「结束不知哪天」不是开放。
pub const UNCONFIRMED_FACT: &str = "f.valid_to IS NULL AND f.valid_to_precision IS NULL
               AND EXISTS (SELECT 1 FROM fact_evidence fe WHERE fe.fact_id = f.id)
               AND NOT EXISTS (SELECT 1 FROM fact_evidence fe
                                 JOIN chunks c ON c.id = fe.chunk_id
                                WHERE fe.fact_id = f.id
                                  AND c.superseded_at IS NULL)";

pub async fn counts(pool: &PgPool, kb_id: Uuid) -> AppResult<ReviewCounts> {
    let sql = format!(
        "SELECT
           (SELECT count(*) FROM pending_facts WHERE kb_id = $1) AS pending,
           (SELECT count(*) FROM resolution_reviews
             WHERE kb_id = $1 AND status = 'pending') AS duplicates,
           (SELECT count(*) FROM resolution_reviews rr
             JOIN entities a ON a.id = rr.left_id JOIN entities b ON b.id = rr.right_id
             WHERE rr.kb_id = $1 AND rr.status = 'pending' AND {same}) AS duplicates_same_type,
           (SELECT count(*) FROM resolution_reviews rr
             JOIN entities a ON a.id = rr.left_id JOIN entities b ON b.id = rr.right_id
             WHERE rr.kb_id = $1 AND rr.status = 'pending' AND {conflict}) AS duplicates_type_conflict,
           (SELECT count(*) FROM fact_conflicts
             WHERE kb_id = $1 AND status = 'open') AS conflicts,
           (SELECT count(*) FROM facts f
             WHERE f.kb_id = $1 AND f.invalidated_at IS NULL AND {unconfirmed}) AS unconfirmed,
           (SELECT count(*) FROM facts
             WHERE kb_id = $1 AND invalidated_at IS NULL
               AND confidence < $2 AND derived_by_rule IS NULL) AS lowconf,
           (SELECT count(*) FROM concept_mappings
             WHERE kb_id = $1 AND status = 'proposed') AS mappings,
           (SELECT count(*) FROM axiom_violations
             WHERE kb_id = $1 AND status = 'open') AS violations,
           (SELECT count(*) FROM ontology_defects
             WHERE kb_id = $1 AND status = 'open') AS defects,
           (SELECT count(*) FROM entity_merges WHERE kb_id = $1) AS merges,
           (SELECT count(*) FROM agent_decisions
             WHERE kb_id = $1 AND status = 'proposed') AS agent,
           (SELECT count(*) FROM agent_decisions WHERE kb_id = $1) AS agent_rows,
           EXISTS (SELECT 1 FROM jobs j WHERE j.kind = 'govern' AND j.status = 'running'
                     AND j.payload->>'kb_id' = $1::text) AS agent_running,
           (SELECT count(*) FROM resolution_reviews rr
             WHERE rr.kb_id = $1 AND rr.status = 'pending'
               AND NOT EXISTS (SELECT 1 FROM agent_decisions d
                                WHERE d.target_kind = 'review' AND d.target_id = rr.id
                                  AND d.status = 'proposed')) AS agent_queue,
           (SELECT count(*) FROM (SELECT 1 FROM phrase_bindings WHERE kb_id = $1 AND status = 'undecided'
                                  UNION ALL SELECT 1 FROM type_bindings WHERE kb_id = $1 AND status = 'undecided') a) AS alignment",
        unconfirmed = UNCONFIRMED_FACT,
        same = crate::resolution::TypeFilter::Same.clause(),
        conflict = crate::resolution::TypeFilter::Conflict.clause(),
    );
    Ok(sqlx::query_as(&sql)
        .bind(kb_id)
        .bind(LOW_CONFIDENCE_BELOW)
        .fetch_one(pool)
        .await?)
}
