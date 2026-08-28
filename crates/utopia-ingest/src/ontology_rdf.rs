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
    // **并集去重，不是两个集合首尾相接**：词汇表常把同一个属性既声明为
    // rdf:Property 又声明为 owl:DatatypeProperty（FOAF 的 name、age、nick… 都这样），
    // 两个集合各收一次，chain 就会把它吐两遍。分类看 data_props 就够了。
    let all_props: BTreeSet<&String> = obj_props.iter().chain(data_props.iter()).collect();
    for iri in all_props {
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

const XSD: &str = "http://www.w3.org/2001/XMLSchema#";
const RDF_NS: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#";

/// `rdfs:range` 映射到我们的四种 datatype 的结果。**三路，不是两路**。
#[derive(Debug, Clone, PartialEq)]
pub enum RangeMapping {
    /// 能映射到 `text` / `number` / `date` / `bool`
    Datatype(&'static str),
    /// 压根没写 range：词汇表没做任何声明，只知道是字面量。
    /// `text` 是诚实的超集（它接受任何字符串，从不拦），所以建、并在预览里列出
    Absent,
    /// 写了 range，值是**短的可读字面量**，但我们的四种类型表达不了它
    ///（`time` 没有年、`duration` 是时长不是时点）。按 `text` 建**并报告**：
    /// 只丢了排序语义，值还在；跳过则是这条知识彻底不会被捕获，那更糟
    Degraded(String),
    /// 写了 range，而那个取值**本就不该进图谱**：二进制块、XML 片段、
    /// XML 内部标识。这些不是业务取值，建出来只会把大东西拖进图。跳过并报告
    Unusable(String),
}

/// 数据属性的 `rdfs:range` → datatype。
///
/// 按**完整 IRI** 匹配而不是局部名：自定义词汇表完全可能有个叫 `date` 的类，
/// 按尾巴匹配会把它当成 `xsd:date`。
///
/// 多条 range 一律 [`RangeMapping::Degraded`]——RDFS 里那是**交集**语义
///（"必须同时是两者"），几乎总是建模笔误，但规范如此，不猜。
pub fn map_range(ranges: &[String]) -> RangeMapping {
    match ranges {
        [] => RangeMapping::Absent,
        [one] => match datatype_of(one) {
            Some(dt) => RangeMapping::Datatype(dt),
            None if unusable(one) => RangeMapping::Unusable(one.clone()),
            None => RangeMapping::Degraded(one.clone()),
        },
        // 多条 range 是交集语义，不猜类型——但值仍是字面量，所以按 text 建
        many => RangeMapping::Degraded(many.join(" ∩ ")),
    }
}

/// 取值不该进图谱的那些：二进制块与 XML 内部管道。
/// **其余一律降级成 text**——一个存得下的值，宁可类型糙一点也别丢掉。
fn unusable(iri: &str) -> bool {
    if let Some(local) = iri.strip_prefix(XSD) {
        return matches!(
            local,
            "base64Binary"
                | "hexBinary"
                | "QName"
                | "NOTATION"
                | "ID"
                | "IDREF"
                | "IDREFS"
                | "ENTITY"
                | "ENTITIES"
        );
    }
    if let Some(local) = iri.strip_prefix(RDF_NS) {
        return local == "XMLLiteral";
    }
    false
}

fn datatype_of(iri: &str) -> Option<&'static str> {
    if let Some(local) = iri.strip_prefix(XSD) {
        return match local {
            // 有界与无符号变体全部收进 number：区别在取值范围，不在语义
            "decimal" | "integer" | "int" | "long" | "short" | "byte" | "nonNegativeInteger"
            | "positiveInteger" | "nonPositiveInteger" | "negativeInteger" | "unsignedLong"
            | "unsignedInt" | "unsignedShort" | "unsignedByte" | "double" | "float" => {
                Some("number")
            }
            // 我们的日期格式本就是 YYYY[-MM[-DD]]，逐级可省，所以 gYear / gYearMonth 装得下
            "date" | "dateTime" | "dateTimeStamp" | "gYear" | "gYearMonth" => Some("date"),
            "boolean" => Some("bool"),
            "string" | "normalizedString" | "token" | "language" | "Name" | "NCName"
            | "NMTOKEN" | "anyURI" => Some("text"),
            // time / gMonth / gDay / gMonthDay 缺年，duration 系列是时长不是时点 ——
            // 落到 None，再由 unusable() 分流：它们是可读字面量，降级成 text；
            // 二进制与 XML 内部标识才是真的不收
            _ => None,
        };
    }
    if let Some(local) = iri.strip_prefix(RDF_NS) {
        // XMLLiteral 是 XML 片段，不收
        return matches!(local, "PlainLiteral" | "langString").then_some("text");
    }
    if let Some(local) = iri.strip_prefix(RDFS) {
        return (local == "Literal").then_some("text");
    }
    if let Some(local) = iri.strip_prefix(OWL) {
        return matches!(local, "real" | "rational").then_some("number");
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn range_maps_every_numeric_variant() {
        for local in [
            "decimal",
            "integer",
            "int",
            "long",
            "short",
            "byte",
            "nonNegativeInteger",
            "positiveInteger",
            "nonPositiveInteger",
            "negativeInteger",
            "unsignedLong",
            "unsignedInt",
            "unsignedShort",
            "unsignedByte",
            "double",
            "float",
        ] {
            assert_eq!(
                map_range(&[format!("{XSD}{local}")]),
                RangeMapping::Datatype("number"),
                "{local}"
            );
        }
        // owl:real / owl:rational 也在 OWL 2 的 datatype map 里
        assert_eq!(
            map_range(&[format!("{OWL}rational")]),
            RangeMapping::Datatype("number")
        );
    }

    /// 我们的日期格式是 YYYY[-MM[-DD]]，逐级可省，所以只缺低位的 g 类型装得下
    #[test]
    fn partial_dates_fit_only_when_the_year_is_there() {
        for local in ["date", "dateTime", "dateTimeStamp", "gYear", "gYearMonth"] {
            assert_eq!(
                map_range(&[format!("{XSD}{local}")]),
                RangeMapping::Datatype("date"),
                "{local}"
            );
        }
        // 缺年的进不了 date —— 但它们是可读字面量，降级成 text 而不是丢掉
        for local in ["gMonth", "gDay", "gMonthDay", "time"] {
            assert!(
                matches!(
                    map_range(&[format!("{XSD}{local}")]),
                    RangeMapping::Degraded(_)
                ),
                "{local} 该降级成 text，不该丢"
            );
        }
    }

    /// 分界不是"能不能精确映射"，而是**"这个取值该不该进图谱"**。
    /// 存得下的一律留下来——类型糙一点，好过这条知识彻底不被捕获。
    #[test]
    fn a_value_we_can_store_is_kept_even_when_we_cannot_type_it() {
        // 没写 range：没有声明可丢，text 是诚实的超集
        assert_eq!(map_range(&[]), RangeMapping::Absent);
        // 写了但表达不了：时长是可读字面量，降级成 text 并报告
        assert!(matches!(
            map_range(&[format!("{XSD}duration")]),
            RangeMapping::Degraded(_)
        ));
        // 取值本就不该进图谱：二进制块与 XML 片段，这才是真的跳过
        assert!(matches!(
            map_range(&[format!("{XSD}base64Binary")]),
            RangeMapping::Unusable(_)
        ));
        assert!(matches!(
            map_range(&[format!("{RDF_NS}XMLLiteral")]),
            RangeMapping::Unusable(_)
        ));
    }

    /// 多条 range 在 RDFS 里是**交集**（"必须同时是两者"），不是并集。
    /// 几乎总是建模笔误，但规范如此 —— 不猜。
    #[test]
    fn several_ranges_are_an_intersection_we_refuse_to_guess() {
        let m = map_range(&[format!("{XSD}string"), format!("{XSD}integer")]);
        match m {
            // 交集不猜类型，但值仍是字面量，所以按 text 落下来
            RangeMapping::Degraded(s) => assert!(s.contains('∩')),
            other => panic!("多条 range 不该被精确映射: {other:?}"),
        }
    }

    /// 按完整 IRI 匹配：自定义词汇表里叫 date 的**类**不是 xsd:date
    #[test]
    fn a_class_that_happens_to_be_called_date_is_not_a_date() {
        assert!(matches!(
            map_range(&["http://acme.example/hr#date".into()]),
            RangeMapping::Degraded(_)
        ));
    }

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
