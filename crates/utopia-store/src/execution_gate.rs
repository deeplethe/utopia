//! 执行闸门（0027，#357）：一次自动合并能不能不经人就落地，看它**撤得回什么**，
//! 不只看把握。
//!
//! 合并在图里撤得回：`revert_merge` 把事实搬回去，0019 之后连合并前的图都读得出。
//! 撤不回的是它活着那几天离开图的东西——推理据它推出的派生、据它开出的违规与告警、
//! 进了导出的三元组、带引用给出去的答案。撤销还原的是图，不是图被读进去的那个世界。
//!
//! 这里问的只有一件事：**这一次合并会不会立刻把什么送出图外**。会，就留给人，
//! 把握再高也不动手。只管合并：分开撤起来不费什么，下一次合并就是撤销。
//!
//! `hold` 是纯函数，`impact_of` 只取数。攒批裁决器与治理 agent 走同一个闸门；
//! 路线图上那道「执行闸门」（检查 agent 的调用）以后也接这一个，不长第二个。

use sqlx::PgPool;
use utopia_core::AppResult;
use uuid::Uuid;

/// 一次合并会立刻牵动的东西
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Impact {
    /// 两边各持一条同一个 functional 谓词的事实、指向不同的实体，合了之后按时间也排不开：
    /// `functional` 违规，一致性检查下一次跑就会开出来——那是离开图的第一站。
    /// 记谓词标签；`inverse_functional` 对称地算
    pub contradictions: Vec<String>,
    /// 任一边参与的、还成立的派生事实数：合并改了前提，这些派生会被重写
    pub derived: i64,
    /// 任一边在对话里被认过的回答数：这个实体是人在问的，错合并就进下一个答案
    pub answered: i64,
}

/// 留给人的理由。`Display` 写成 `kind value`，直接接在 `escalate_impact|` 后面，
/// 界面按 kind 措辞
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Hold {
    Contradiction(String),
    Derived(i64),
    Answered(i64),
}

impl std::fmt::Display for Hold {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Hold::Contradiction(p) => write!(f, "contradiction {p}"),
            Hold::Derived(n) => write!(f, "derived {n}"),
            Hold::Answered(n) => write!(f, "answered {n}"),
        }
    }
}

impl Hold {
    /// 写给 `agent_decisions.reason` 的一句：人在 Agent 队列里看到 agent 为什么没动手
    pub fn explain(&self) -> String {
        match self {
            Hold::Contradiction(p) => {
                format!("merging would put two \"{p}\" facts on one entity")
            }
            Hold::Derived(n) => format!("{n} derived facts rest on one side"),
            Hold::Answered(n) => format!("one side was named in {n} answers"),
        }
    }
}

impl Impact {
    /// 给第二层的工具看的一段（0028）：合并会牵动什么，一行一件；什么都不牵动也说出来
    pub fn describe(&self) -> String {
        let mut lines = Vec::new();
        for p in &self.contradictions {
            lines.push(format!(
                "- merging would put two \"{p}\" facts on one entity; \"{p}\" allows one value"
            ));
        }
        if self.derived > 0 {
            lines.push(format!(
                "- {} derived fact(s) rest on one side and would be rewritten",
                self.derived
            ));
        }
        if self.answered > 0 {
            lines.push(format!(
                "- one side was named in {} chat answer(s)",
                self.answered
            ));
        }
        if lines.is_empty() {
            "(merging would touch nothing outside the graph: no one-value relation clashes, no derived facts, no answers named either side)".to_string()
        } else {
            lines.join(
                "
",
            )
        }
    }
}

/// 这一次合并该不该留给人。矛盾最先说——它会立刻开出违规；其次派生，其次答案。
/// 什么都不牵动就是 None：把握够就照旧自动
pub fn hold(impact: &Impact) -> Option<Hold> {
    if let Some(p) = impact.contradictions.first() {
        return Some(Hold::Contradiction(p.clone()));
    }
    if impact.derived > 0 {
        return Some(Hold::Derived(impact.derived));
    }
    if impact.answered > 0 {
        return Some(Hold::Answered(impact.answered));
    }
    None
}

/// 取一对实体合并会牵动什么。三条查询，只在裁决器有把握要合的时候才跑。
///
/// 写路径的守卫看的是「当前那一行」（`invalidated_at IS NULL`），与 `resolve_mention`
/// 数度数一个写法；不走记录轴谓词——修正永远发生在现在（0019）
pub async fn impact_of(pool: &PgPool, kb_id: Uuid, a: Uuid, b: Uuid) -> AppResult<Impact> {
    // 与一致性检查同一个口径：只看实体宾语的边，**按时间看**——两边各有一个不同的值，
    // 合了之后若能排成先后接替（房东先 HPBB1、后 BBHQ1），一致性检查不会报，这里也
    // 不拦。从前这里不看时间，一份换过房东的租约有两个名字就永远合不起来
    let contradictions = crate::temporal::merge_would_overlap(pool, kb_id, a, b).await?;

    let derived: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM derived_facts d
         WHERE d.kb_id = $1 AND d.invalidated_at IS NULL
           AND (d.subject_id IN ($2, $3) OR d.object_id IN ($2, $3))",
    )
    .bind(kb_id)
    .bind(a)
    .bind(b)
    .fetch_one(pool)
    .await?;

    // `resolved` 是每一轮回答认下的实体（id、名字、类型）；被认过就是被问过
    let answered: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM conversation_messages m
         JOIN conversations c ON c.id = m.conversation_id
         WHERE c.kb_id = $1 AND m.role = 'assistant'
           AND EXISTS (SELECT 1 FROM jsonb_array_elements(m.resolved) e
                       WHERE e->>'id' IN ($2, $3))",
    )
    .bind(kb_id)
    .bind(a.to_string())
    .bind(b.to_string())
    .fetch_one(pool)
    .await?;

    Ok(Impact {
        contradictions,
        derived,
        answered,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_touched_means_no_hold() {
        assert_eq!(hold(&Impact::default()), None);
    }

    #[test]
    fn a_contradiction_speaks_before_anything_else() {
        let i = Impact {
            contradictions: vec!["CEO of".into(), "headquartered in".into()],
            derived: 4,
            answered: 2,
        };
        assert_eq!(hold(&i), Some(Hold::Contradiction("CEO of".into())));
        assert_eq!(hold(&i).unwrap().to_string(), "contradiction CEO of");
    }

    #[test]
    fn describe_says_what_would_move_or_that_nothing_would() {
        assert!(Impact::default()
            .describe()
            .contains("nothing outside the graph"));
        let i = Impact {
            contradictions: vec!["CEO of".into()],
            derived: 2,
            answered: 0,
        };
        let text = i.describe();
        assert!(text.contains("two \"CEO of\" facts"));
        assert!(text.contains("2 derived fact"));
        assert!(!text.contains("chat answer"));
    }

    #[test]
    fn derived_then_answered() {
        let i = Impact {
            contradictions: vec![],
            derived: 3,
            answered: 1,
        };
        assert_eq!(hold(&i), Some(Hold::Derived(3)));
        let i = Impact { derived: 0, ..i };
        assert_eq!(hold(&i), Some(Hold::Answered(1)));
        assert_eq!(Hold::Answered(1).to_string(), "answered 1");
        assert_eq!(
            Hold::Contradiction("CEO of".into()).explain(),
            "merging would put two \"CEO of\" facts on one entity"
        );
    }
}
