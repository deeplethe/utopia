//! OWL / RDFS 文件的解析与**投影**。
//!
//! 分工按 0001 定的三层：本模块只做第二层（投影），第一层（原文保真进 blob）
//! 由调用方负责。**这里读不懂的东西不是错误，是"暂未投影"**——原文留着，
//! 将来补上消费者时重跑即可。
//!
//! 只解析，不推理：Oxigraph 那两个小解析器给出三元组，我们按名单挑走能用的。
//! 不引 horned-owl——推理机是 0002 的事，而且它读的是原文不是投影。

use std::collections::{BTreeMap, BTreeSet};

/// 投影出来的一个类。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct OwlClass {
    pub iri: String,
    /// 从 IRI 派生的短标签（模型读写的令牌）；调用方负责去重加后缀
    pub key: String,
    pub label: String,
    /// `rdfs:comment` —— 承重字段，逐字进抽取提示词
    pub description: String,
    /// `rdfs:subClassOf` 的全部父类 IRI（多继承在这里是常态）
    pub parents: Vec<String>,
}

/// 投影出来的一个属性（对象属性 → 关系，数据属性 → 属性）。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct OwlProperty {
    pub iri: String,
    pub key: String,
    pub label: String,
    pub description: String,
    /// true = owl:DatatypeProperty（字面值），false = 对象属性
    pub is_datatype: bool,
    pub functional: bool,
    pub inverse_functional: bool,
    pub domains: Vec<String>,
    pub ranges: Vec<String>,
}

/// 一次解析的结果。`unprojected` 是**报告**不是错误——见模块文档。
#[derive(Debug, Default)]
pub struct OwlProjection {
    pub classes: Vec<OwlClass>,
    pub properties: Vec<OwlProperty>,
    /// 出现过但我们今天不消费的谓词 → 次数。给预览页"暂未投影"那一栏
    pub unprojected: BTreeMap<String, usize>,
    /// 三元组总数，让人对"这个文件有多大"有个数
    pub triples: usize,
}

const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
const RDFS: &str = "http://www.w3.org/2000/01/rdf-schema#";
const OWL: &str = "http://www.w3.org/2002/07/owl#";

/// 支持的输入格式。v1 只做这两个——Protégé 导出的绝大多数是它们，
/// OWL/XML 与 Manchester 语法刻意砍掉（见 0001 P2 的 "v1 砍掉"）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RdfFormat {
    Turtle,
    RdfXml,
}

impl RdfFormat {
    /// 扩展名是强信号，内容只在**明确矛盾**时推翻它。
    ///
    /// 曾经反过来（`.rdf` 探不到 XML 标志就退回 Turtle），结果 FOAF 的官方文件
    /// 一开头是几十行 `<!--` 注释、`<rdf:` 在嗅探窗口之外，于是被当成 Turtle
    /// 送进解析器，第一行就报 "Invalid IRI code point"。
    pub fn detect(filename: &str, bytes: &[u8]) -> Self {
        let lower = filename.to_ascii_lowercase();
        // Turtle 的指纹很硬：只有它以 @prefix / @base / PREFIX 开头
        let looks_turtle = {
            let head = &bytes[..bytes.len().min(4096)];
            let s = String::from_utf8_lossy(head);
            s.lines()
                .map(str::trim_start)
                .find(|l| !l.is_empty() && !l.starts_with('#'))
                .is_some_and(|l| {
                    l.starts_with("@prefix")
                        || l.starts_with("@base")
                        || l.starts_with("PREFIX")
                        || l.starts_with("BASE")
                })
        };
        if lower.ends_with(".ttl") || lower.ends_with(".turtle") || lower.ends_with(".n3") {
            return Self::Turtle;
        }
        if lower.ends_with(".rdf") || lower.ends_with(".owl") || lower.ends_with(".xml") {
            // .owl 两种编码都常见，所以内容说了算——但只有"确实像 Turtle"才推翻
            return if looks_turtle {
                Self::Turtle
            } else {
                Self::RdfXml
            };
        }
        if looks_turtle {
            Self::Turtle
        } else {
            Self::RdfXml
        }
    }
}

/// 三元组的中间形态：只留我们看得懂的部分。
struct Triple {
    subject: String,
    predicate: String,
    /// 宾语是 IRI 时为 Some
    object_iri: Option<String>,
    /// 宾语是字面量时为 Some(值, 语言标记)
    object_lit: Option<(String, Option<String>)>,
}

fn read_triples(bytes: &[u8], format: RdfFormat) -> anyhow::Result<Vec<Triple>> {
    use oxrdf::Term;
    let mut out = Vec::new();
    let mut push = |t: oxrdf::Triple| {
        let subject = match &t.subject {
            oxrdf::NamedOrBlankNode::NamedNode(n) => n.as_str().to_string(),
            // 空节点是匿名类表达式（owl:Restriction 之类）——投影不碰它们，
            // 原文里留着等推理机
            oxrdf::NamedOrBlankNode::BlankNode(_) => return,
        };
        let (object_iri, object_lit) = match &t.object {
            Term::NamedNode(n) => (Some(n.as_str().to_string()), None),
            Term::Literal(l) => (
                None,
                Some((
                    l.value().to_string(),
                    l.language().map(|s| s.to_ascii_lowercase()),
                )),
            ),
            _ => (None, None),
        };
        out.push(Triple {
            subject,
            predicate: t.predicate.as_str().to_string(),
            object_iri,
            object_lit,
        });
    };
    match format {
        RdfFormat::Turtle => {
            for r in oxttl::TurtleParser::new().for_reader(bytes) {
                push(r?);
            }
        }
        RdfFormat::RdfXml => {
            for r in oxrdfxml::RdfXmlParser::new().for_reader(bytes) {
                push(r?);
            }
        }
    }
    Ok(out)
}

/// 解析并投影。语言标记优先 `@en`/`@zh`，其次无标记，最后随便一个。
pub fn project(bytes: &[u8], format: RdfFormat) -> anyhow::Result<OwlProjection> {
    let triples = read_triples(bytes, format)?;
    let mut proj = OwlProjection {
        triples: triples.len(),
        ..Default::default()
    };

    // 先分类：谁是类、谁是对象属性、谁是数据属性、谁带函数性标记
    let mut classes: BTreeSet<String> = BTreeSet::new();
    let mut obj_props: BTreeSet<String> = BTreeSet::new();
    let mut data_props: BTreeSet<String> = BTreeSet::new();
    let mut functional: BTreeSet<String> = BTreeSet::new();
    let mut inverse_functional: BTreeSet<String> = BTreeSet::new();
    for t in &triples {
        if t.predicate != RDF_TYPE {
            continue;
        }
        let Some(o) = t.object_iri.as_deref() else {
            continue;
        };
        match o {
            x if x == format!("{OWL}Class") || x == format!("{RDFS}Class") => {
                classes.insert(t.subject.clone());
            }
            x if x == format!("{OWL}ObjectProperty") => {
                obj_props.insert(t.subject.clone());
            }
            x if x == format!("{OWL}DatatypeProperty") => {
                data_props.insert(t.subject.clone());
            }
            // rdf:Property 没说是对象还是数据 —— 按对象属性处理，
            // 因为宾语是 IRI 的三元组远多于字面值，猜错的代价也只是分错通道
            "http://www.w3.org/1999/02/22-rdf-syntax-ns#Property" => {
                obj_props.insert(t.subject.clone());
            }
            x if x == format!("{OWL}FunctionalProperty") => {
                functional.insert(t.subject.clone());
            }
            x if x == format!("{OWL}InverseFunctionalProperty") => {
                inverse_functional.insert(t.subject.clone());
            }
            _ => {}
        }
    }

    // 再收集标签、注释、父类、domain/range
    let mut labels: BTreeMap<String, Vec<(String, Option<String>)>> = BTreeMap::new();
    let mut comments: BTreeMap<String, Vec<(String, Option<String>)>> = BTreeMap::new();
    let mut parents: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut domains: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut ranges: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let known = |p: &str| {
        p == RDF_TYPE
            || p == format!("{RDFS}label")
            || p == format!("{RDFS}comment")
            || p == format!("{RDFS}subClassOf")
            || p == format!("{RDFS}subPropertyOf")
            || p == format!("{RDFS}domain")
            || p == format!("{RDFS}range")
    };
    for t in &triples {
        let p = t.predicate.as_str();
        if p == format!("{RDFS}label") {
            if let Some(l) = &t.object_lit {
                labels.entry(t.subject.clone()).or_default().push(l.clone());
            }
        } else if p == format!("{RDFS}comment") {
            if let Some(l) = &t.object_lit {
                comments
                    .entry(t.subject.clone())
                    .or_default()
                    .push(l.clone());
            }
        } else if p == format!("{RDFS}subClassOf") {
            if let Some(o) = &t.object_iri {
                parents
                    .entry(t.subject.clone())
                    .or_default()
                    .push(o.clone());
            }
        } else if p == format!("{RDFS}domain") {
            if let Some(o) = &t.object_iri {
                domains
                    .entry(t.subject.clone())
                    .or_default()
                    .push(o.clone());
            }
        } else if p == format!("{RDFS}range") {
            if let Some(o) = &t.object_iri {
                ranges.entry(t.subject.clone()).or_default().push(o.clone());
            }
        } else if !known(p) {
            // 报告而不是丢弃：预览页要能说清"这个文件里还有什么我们没消费"
            *proj.unprojected.entry(p.to_string()).or_insert(0) += 1;
        }
    }

    for iri in &classes {
        proj.classes.push(OwlClass {
            key: key_from_iri(iri),
            label: pick_lang(labels.get(iri)).unwrap_or_else(|| local_name(iri).to_string()),
            description: pick_lang(comments.get(iri)).unwrap_or_default(),
            parents: parents.get(iri).cloned().unwrap_or_default(),
            iri: iri.clone(),
        });
    }
    for iri in obj_props.iter().chain(data_props.iter()) {
        proj.properties.push(OwlProperty {
            key: key_from_iri(iri),
            label: pick_lang(labels.get(iri))
                .unwrap_or_else(|| local_name(iri).replace('_', " ").to_string()),
            description: pick_lang(comments.get(iri)).unwrap_or_default(),
            is_datatype: data_props.contains(iri),
            functional: functional.contains(iri),
            inverse_functional: inverse_functional.contains(iri),
            domains: domains.get(iri).cloned().unwrap_or_default(),
            ranges: ranges.get(iri).cloned().unwrap_or_default(),
            iri: iri.clone(),
        });
    }
    Ok(proj)
}

/// 多语言标签里挑一个：优先 en / zh，其次无语言标记，最后第一个。
fn pick_lang(vals: Option<&Vec<(String, Option<String>)>>) -> Option<String> {
    let vals = vals?;
    for want in ["en", "zh"] {
        if let Some((v, _)) = vals
            .iter()
            .find(|(_, l)| l.as_deref().is_some_and(|l| l.starts_with(want)))
        {
            return Some(v.clone());
        }
    }
    vals.iter()
        .find(|(_, l)| l.is_none())
        .or_else(|| vals.first())
        .map(|(v, _)| v.clone())
}

/// IRI 的局部名：最后一个 `#` 或 `/` 之后的部分。
pub fn local_name(iri: &str) -> &str {
    iri.rsplit(['#', '/']).next().unwrap_or(iri)
}

/// 从 IRI 派生 key。**IRI 是身份，key 是给模型读的标签**（见 0001 P2）：
/// 只允许 `[a-z0-9_]`、最长 40，所以 IRI 本身进不去。
/// 驼峰拆成下划线：`hasEmployee` → `has_employee`。
pub fn key_from_iri(iri: &str) -> String {
    let local = local_name(iri);
    let mut out = String::with_capacity(local.len() + 4);
    let mut prev_lower = false;
    for c in local.chars() {
        if c.is_ascii_uppercase() {
            if prev_lower && !out.is_empty() {
                out.push('_');
            }
            out.push(c.to_ascii_lowercase());
            prev_lower = false;
        } else if c.is_ascii_alphanumeric() {
            out.push(c);
            prev_lower = true;
        } else if !out.ends_with('_') && !out.is_empty() {
            out.push('_');
            prev_lower = false;
        }
    }
    while out.ends_with('_') {
        out.pop();
    }
    out.chars().take(40).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const TTL: &str = r#"
        @prefix owl: <http://www.w3.org/2002/07/owl#> .
        @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
        @prefix ex: <http://acme.example/hr#> .

        ex:Employee a owl:Class ;
            rdfs:label "Employee"@en ;
            rdfs:label "员工"@zh ;
            rdfs:comment "A person on the payroll."@en ;
            rdfs:subClassOf ex:Person .
        ex:Person a owl:Class ; rdfs:label "Person" .
        ex:hasManager a owl:ObjectProperty, owl:FunctionalProperty ;
            rdfs:label "has manager" ;
            rdfs:domain ex:Employee ;
            rdfs:range ex:Person .
        ex:salary a owl:DatatypeProperty ; rdfs:domain ex:Employee .
        ex:Employee owl:disjointWith ex:Contractor .
    "#;

    #[test]
    fn projects_classes_properties_and_reports_the_rest() {
        let p = project(TTL.as_bytes(), RdfFormat::Turtle).unwrap();
        let emp = p.classes.iter().find(|c| c.key == "employee").unwrap();
        // 多语言标签取 en 优先
        assert_eq!(emp.label, "Employee");
        assert_eq!(emp.description, "A person on the payroll.");
        assert_eq!(emp.parents, vec!["http://acme.example/hr#Person"]);

        let mgr = p
            .properties
            .iter()
            .find(|x| x.key == "has_manager")
            .unwrap();
        assert!(mgr.functional && !mgr.is_datatype);
        assert_eq!(mgr.ranges, vec!["http://acme.example/hr#Person"]);

        let sal = p.properties.iter().find(|x| x.key == "salary").unwrap();
        assert!(sal.is_datatype);

        // 消费不了的公理进报告，不是丢弃也不是报错
        assert!(p
            .unprojected
            .contains_key("http://www.w3.org/2002/07/owl#disjointWith"));
    }

    #[test]
    fn detects_rdfxml_that_opens_with_comments() {
        // FOAF 的官方文件就长这样：几十行 <!-- --> 之后才见 <rdf:RDF>。
        // 早先版本嗅不到 XML 标志就退回 Turtle，结果第一行就解析失败
        let head = b"<!-- This is the FOAF vocabulary, expressed using RDFS and OWL. -->\n\
                     <!-- padding padding padding padding padding padding padding -->\n";
        assert_eq!(RdfFormat::detect("index.rdf", head), RdfFormat::RdfXml);
        // 反过来：.owl 里放 Turtle 也常见，内容说了算
        assert_eq!(
            RdfFormat::detect("x.owl", b"@prefix owl: <http://x#> .\n"),
            RdfFormat::Turtle
        );
        // 注释开头的 Turtle 同样认得出（# 行先跳过）
        assert_eq!(
            RdfFormat::detect("x", b"# a note\n\n@base <http://x> .\n"),
            RdfFormat::Turtle
        );
    }

    #[test]
    fn key_derives_from_the_iri_local_name() {
        assert_eq!(key_from_iri("http://x/hr#hasEmployee"), "has_employee");
        assert_eq!(key_from_iri("http://x/ns/Person"), "person");
        assert_eq!(key_from_iri("http://x#HTTP_Server"), "http_server");
    }
}
