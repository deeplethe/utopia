//! 场景包（0073）：按业务场景预置的一小套词汇，连同它要回答的问题。
//!
//! 标准本体包（`ontology_packs`，0008）给的是通用类型，不给语料要的谓词；场景包反过来：
//! 几个类、十来条属性和关系，定义是写给对齐器读的，单位和 datatype 已经选对，再加上这类
//! 语料要回答的能力问题（0061）。文件内嵌进二进制，离线部署也有。
//!
//! **走编辑器的路，不走 OWL 导入。** 类和属性用本体页同样的调用建出来，没有 IRI，是这个库
//! 自己的元素（0012）：能改、能删、导出时是库自己的词汇。问题按人提的记（`origin = person`，
//! 建库的人是作者）：勾上场景包就是这个人对库的要求。只在建库时装：新库没有事实，不必对账。
//!
//! 键撞上已有元素（先装的标准包、另一个场景包）的跳过并报告，不合并不覆盖；定义域、值域
//! 只认包自己的类键或库里已有的键，认不出的丢掉那条声明、照样装。

use std::collections::HashMap;
use std::sync::LazyLock;

use serde::Deserialize;
use utopia_core::models::RelationAxioms;
use utopia_core::{AppError, AppResult};
use uuid::Uuid;

use crate::state::AppState;

/// 一段双语文字：标签是数据，按库的 `ontology_lang` 取（0004）
#[derive(Debug, Clone, Deserialize)]
pub struct Text {
    pub zh: String,
    pub en: String,
}

impl Text {
    pub fn get(&self, lang: &str) -> &str {
        if lang == "zh" {
            &self.zh
        } else {
            &self.en
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Class {
    pub key: String,
    pub label: Text,
    pub description: Text,
    /// 父类的键，必须在本包里排在前面或已在库里
    #[serde(default)]
    pub parents: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Attribute {
    pub key: String,
    /// 挂在哪个类上（定义域）
    pub host: String,
    pub label: Text,
    pub description: Text,
    /// text / number / date / bool
    pub datatype: String,
    /// 数的单位，按这个语料写的那种；读不出的值记成 `attr_datatype` 丢弃
    #[serde(default)]
    pub unit: Option<String>,
    #[serde(default = "yes")]
    pub functional: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Relation {
    pub key: String,
    pub label: Text,
    pub description: Text,
    #[serde(default)]
    pub domains: Vec<String>,
    #[serde(default)]
    pub ranges: Vec<String>,
    /// state / event / eternal（0031）
    #[serde(default = "state")]
    pub temporal: String,
    #[serde(default)]
    pub functional: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Question {
    pub question: Text,
    #[serde(default)]
    pub expected_answer: Option<Text>,
    /// 答案要经过的类和属性（0061 决定 1 的「形状」），照原样存
    #[serde(default)]
    pub needs: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ScenarioPack {
    pub id: String,
    pub name: Text,
    pub summary: Text,
    #[serde(default)]
    pub classes: Vec<Class>,
    #[serde(default)]
    pub attributes: Vec<Attribute>,
    #[serde(default)]
    pub relations: Vec<Relation>,
    #[serde(default)]
    pub questions: Vec<Question>,
}

fn yes() -> bool {
    true
}
fn state() -> String {
    "state".into()
}

const FILES: &[(&str, &str)] = &[
    (
        "enterprise-profile",
        include_str!("../packs/scenarios/enterprise-profile.json"),
    ),
    (
        "contract-chain",
        include_str!("../packs/scenarios/contract-chain.json"),
    ),
    (
        "personnel-records",
        include_str!("../packs/scenarios/personnel-records.json"),
    ),
];

/// 内嵌的全部场景包。文件写错是构建的问题，不是运行时的：这里直接 panic，测试
/// [`tests::every_shipped_pack_parses_and_resolves`] 在 CI 里先于任何部署撞上它
pub static PACKS: LazyLock<Vec<ScenarioPack>> = LazyLock::new(|| {
    FILES
        .iter()
        .map(|(id, text)| {
            let pack: ScenarioPack = serde_json::from_str(text)
                .unwrap_or_else(|e| panic!("scenario pack {id} does not parse: {e}"));
            assert_eq!(&pack.id, id, "scenario pack file {id} carries another id");
            pack
        })
        .collect()
});

pub fn get(id: &str) -> Option<&'static ScenarioPack> {
    PACKS.iter().find(|p| p.id == id)
}

/// 装了什么、跳过了什么。`skipped` 是人读的句子：键撞上了谁、哪条声明认不出
#[derive(Debug, Default, serde::Serialize)]
pub struct Report {
    pub classes: usize,
    pub properties: usize,
    pub questions: usize,
    pub skipped: Vec<String>,
}

/// 把几个场景包装进一个新库。`lang` 是库的 `ontology_lang`：标签与定义按它取。
/// 建了东西就排一份 `embed_ontology`，第一次对齐之前元素得有向量（同编辑器与导入）
pub async fn install(
    state: &AppState,
    kb_id: Uuid,
    actor: Uuid,
    lang: &str,
    ids: &[String],
) -> AppResult<Report> {
    let mut report = Report::default();
    if ids.is_empty() {
        return Ok(report);
    }
    let packs: Vec<&ScenarioPack> = ids
        .iter()
        .map(|id| {
            get(id).ok_or_else(|| AppError::invalid("unknown_pack", format!("未知的场景包：{id}")))
        })
        .collect::<AppResult<_>>()?;
    let pool = &state.pool;
    // 库里已有的键（先装的标准包、前一个场景包）：撞上的跳过，声明可以指向它们
    let mut classes: HashMap<String, Uuid> =
        sqlx::query_as::<_, (String, Uuid)>("SELECT key, id FROM entity_types WHERE kb_id = $1")
            .bind(kb_id)
            .fetch_all(pool)
            .await?
            .into_iter()
            .collect();
    let mut properties: HashMap<String, Uuid> =
        sqlx::query_as::<_, (String, Uuid)>("SELECT key, id FROM relation_types WHERE kb_id = $1")
            .bind(kb_id)
            .fetch_all(pool)
            .await?
            .into_iter()
            .collect();
    for pack in &packs {
        for c in &pack.classes {
            if classes.contains_key(&c.key) {
                report
                    .skipped
                    .push(format!("{}: class {} already exists", pack.id, c.key));
                continue;
            }
            let mut parents = Vec::new();
            for p in &c.parents {
                match classes.get(p) {
                    Some(id) => parents.push(*id),
                    None => report
                        .skipped
                        .push(format!("{}: parent {p} of {} not found", pack.id, c.key)),
                }
            }
            let id = utopia_store::ontology::create_entity_type(
                pool,
                kb_id,
                &c.key,
                c.label.get(lang),
                "#64748b",
                "circle",
                &parents,
                c.description.get(lang),
            )
            .await?;
            classes.insert(c.key.clone(), id);
            report.classes += 1;
        }
        for a in &pack.attributes {
            if properties.contains_key(&a.key) {
                report
                    .skipped
                    .push(format!("{}: property {} already exists", pack.id, a.key));
                continue;
            }
            let Some(host) = classes.get(&a.host) else {
                report.skipped.push(format!(
                    "{}: host {} of {} not found",
                    pack.id, a.host, a.key
                ));
                continue;
            };
            let id = utopia_store::ontology::create_relation_type(
                pool,
                kb_id,
                &a.key,
                a.label.get(lang),
                "state",
                RelationAxioms {
                    functional: a.functional,
                    ..RelationAxioms::default()
                },
                a.description.get(lang),
                "attribute",
                &[*host],
                &[],
                Some(a.datatype.as_str()),
                a.unit.as_deref(),
            )
            .await?;
            properties.insert(a.key.clone(), id);
            report.properties += 1;
        }
        for r in &pack.relations {
            if properties.contains_key(&r.key) {
                report
                    .skipped
                    .push(format!("{}: property {} already exists", pack.id, r.key));
                continue;
            }
            let mut resolve = |keys: &[String], side: &str| -> Vec<Uuid> {
                keys.iter()
                    .filter_map(|k| {
                        let id = classes.get(k).copied();
                        if id.is_none() {
                            report
                                .skipped
                                .push(format!("{}: {side} {k} of {} not found", pack.id, r.key));
                        }
                        id
                    })
                    .collect()
            };
            let domains = resolve(&r.domains, "domain");
            let ranges = resolve(&r.ranges, "range");
            let id = utopia_store::ontology::create_relation_type(
                pool,
                kb_id,
                &r.key,
                r.label.get(lang),
                &r.temporal,
                RelationAxioms {
                    functional: r.functional,
                    ..RelationAxioms::default()
                },
                r.description.get(lang),
                "relation",
                &domains,
                &ranges,
                None,
                None,
            )
            .await?;
            properties.insert(r.key.clone(), id);
            report.properties += 1;
        }
        for q in &pack.questions {
            let text = q.question.get(lang);
            if utopia_store::competency_questions::exists_text(pool, kb_id, text).await? {
                report
                    .skipped
                    .push(format!("{}: question already exists: {text}", pack.id));
                continue;
            }
            utopia_store::competency_questions::create(
                pool,
                kb_id,
                utopia_store::competency_questions::NewQuestion {
                    question: text,
                    expected_answer: q.expected_answer.as_ref().map(|t| t.get(lang)),
                    needs: q.needs.as_ref(),
                    origin: "person",
                    status: "accepted",
                    created_by: Some(actor),
                },
            )
            .await?;
            report.questions += 1;
        }
        let _ = utopia_store::audit::record(
            pool,
            Some(kb_id),
            actor,
            "scenario_pack.installed",
            "knowledge_base",
            Some(kb_id),
            serde_json::json!({ "pack": pack.id, "lang": lang }),
        )
        .await;
    }
    if report.classes + report.properties > 0 {
        // 新元素要有向量，对齐的短名单才看得见它（同 `reindex_then_align`）
        utopia_store::jobs::enqueue_unless_queued_after(
            pool,
            "embed_ontology",
            serde_json::json!({ "kb_id": kb_id }),
            std::time::Duration::from_secs(2),
        )
        .await?;
    }
    if !report.skipped.is_empty() {
        tracing::info!(%kb_id, skipped = ?report.skipped, "场景包有跳过的元素");
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::{Text, PACKS};
    use std::collections::HashSet;

    /// 文件是手写的 JSON：解析得了、键不重复、每条声明都指向本包排在前面的类、datatype 合法
    #[test]
    fn every_shipped_pack_parses_and_resolves() {
        assert_eq!(PACKS.len(), 3);
        for pack in PACKS.iter() {
            let mut classes: HashSet<&str> = HashSet::new();
            for c in &pack.classes {
                for p in &c.parents {
                    assert!(
                        classes.contains(p.as_str()),
                        "{}: parent {p} of {} comes later",
                        pack.id,
                        c.key
                    );
                }
                assert!(classes.insert(&c.key), "{}: class {} twice", pack.id, c.key);
            }
            let mut properties: HashSet<&str> = HashSet::new();
            for a in &pack.attributes {
                assert!(
                    classes.contains(a.host.as_str()),
                    "{}: host {} of {}",
                    pack.id,
                    a.host,
                    a.key
                );
                assert!(
                    matches!(a.datatype.as_str(), "text" | "number" | "date" | "bool"),
                    "{}: {}",
                    pack.id,
                    a.key
                );
                assert!(
                    properties.insert(&a.key),
                    "{}: property {} twice",
                    pack.id,
                    a.key
                );
            }
            for r in &pack.relations {
                for k in r.domains.iter().chain(r.ranges.iter()) {
                    assert!(
                        classes.contains(k.as_str()),
                        "{}: {k} of {}",
                        pack.id,
                        r.key
                    );
                }
                assert!(
                    matches!(r.temporal.as_str(), "state" | "event" | "eternal"),
                    "{}: {}",
                    pack.id,
                    r.key
                );
                assert!(
                    properties.insert(&r.key),
                    "{}: property {} twice",
                    pack.id,
                    r.key
                );
            }
            assert!(
                !pack.questions.is_empty(),
                "{}: a pack ships with its questions",
                pack.id
            );
            for t in pack
                .classes
                .iter()
                .map(|c| &c.description)
                .chain(pack.attributes.iter().map(|a| &a.description))
                .chain(pack.relations.iter().map(|r| &r.description))
            {
                assert!(
                    !t.zh.trim().is_empty() && !t.en.trim().is_empty(),
                    "{}: a definition in both languages",
                    pack.id
                );
            }
        }
    }

    #[test]
    fn a_label_follows_the_base_language() {
        let t = Text {
            zh: "企业".into(),
            en: "Company".into(),
        };
        assert_eq!(t.get("zh"), "企业");
        assert_eq!(t.get("en"), "Company");
        assert_eq!(t.get("fr"), "Company");
    }
}
