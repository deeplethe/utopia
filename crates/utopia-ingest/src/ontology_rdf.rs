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
    /// `owl:disjointWith` 的对端 IRI。**两个方向都收**——公理是对称的,
    /// 而词表通常只写一遍
    pub disjoint_with: Vec<String>,
}

/// 投影出来的一个属性（对象属性 → 关系，数据属性 → 属性）。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct OwlProperty {
    pub iri: String,
    pub key: String,
    pub label: String,
    pub description: String,
    /// true = 走属性通道（字面值），false = 走关系通道。
    /// 来源有二：显式的 `owl:DatatypeProperty`，或者 range 全是数据类型
    pub is_datatype: bool,
    pub functional: bool,
    pub inverse_functional: bool,
    /// OWL 属性公理。一致性检查(0002 R0)的判定依据:没有它们,
    /// `A part_of B` 与 `B part_of A` 同时存在到底是矛盾还是正常,无从判起
    pub transitive: bool,
    pub symmetric: bool,
    pub asymmetric: bool,
    pub irreflexive: bool,
    pub domains: Vec<String>,
    pub ranges: Vec<String>,
    /// **多条 range 是并集还是交集**。`rdfs:range` 写多条是交集
    ///（"必须同时是两者"），`schema:rangeIncludes` 写多条是并集
    ///（"哪个都行"）。倒进同一个 Vec 而不记这一位，
    /// `author rangeIncludes Organization, Person` 就会被读成
    /// "必须既是组织又是人"，然后降级成 text——一条边就这么没了
    pub ranges_union: bool,
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
    /// **这份文件自己声明为数据类型的那些 IRI** → 我们的四种之一。
    /// schema.org 把 Text/Number/Date… 声明成 `a rdfs:Class, schema:DataType`，
    /// 只看 `rdfs:Class` 会把它们当成实体类建出来（`text`、`boolean` 成了实体类型）
    pub vocab_datatypes: VocabDatatypes,
}

/// 词汇表自己声明的数据类型 IRI → 我们的四种（`text`/`number`/`date`/`bool`）。
pub type VocabDatatypes = BTreeMap<String, &'static str>;

const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
const RDFS: &str = "http://www.w3.org/2000/01/rdf-schema#";
const OWL: &str = "http://www.w3.org/2002/07/owl#";

/// schema.org 自己那套 domain/range。**不是标准词汇**，所以写死在这里，
/// 但值得认：schema.org 及派生词汇表一个 `rdfs:domain` 都没有，
/// 不认这两个谓词就等于把它整个类型系统当成没看见——1600 多个属性
/// 会全变成无约束的关系。
/// 两种 scheme 都收：同一份词汇表的新旧版本分别用 https 与 http 发布。
const SCHEMA_NS: [&str; 2] = ["https://schema.org/", "http://schema.org/"];

/// 谓词/类是不是 schema.org 那个名字下的某一个。
fn is_schema(iri: &str, local: &str) -> bool {
    SCHEMA_NS.iter().any(|ns| {
        iri.len() == ns.len() + local.len() && iri.starts_with(ns) && iri.ends_with(local)
    })
}

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
    // **相对 IRI 需要 base 才能解析。** 从字节读没有文档 URL，而 Turtle 规范说
    // base 默认取文档自身的地址。给一个占位符：本体文件里的相对 IRI 几乎都是
    // 文档级元数据（PROV-O 的 `<#> a owl:Ontology`），我们不消费 owl:Ontology 节点，
    // 解析得过去就行。不给的话整个文件在第一个 `<#>` 上报
    // "No scheme found in an absolute IRI" 而全军覆没
    const BASE: &str = "urn:utopia:import";
    match format {
        RdfFormat::Turtle => {
            let p = oxttl::TurtleParser::new()
                .with_base_iri(BASE)
                .map_err(|e| anyhow::anyhow!("base IRI 无效：{e}"))?;
            for r in p.for_reader(bytes) {
                push(r?);
            }
        }
        RdfFormat::RdfXml => {
            let p = oxrdfxml::RdfXmlParser::new()
                .with_base_iri(BASE)
                .map_err(|e| anyhow::anyhow!("base IRI 无效：{e}"))?;
            for r in p.for_reader(bytes) {
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
    let mut transitive: BTreeSet<String> = BTreeSet::new();
    let mut symmetric: BTreeSet<String> = BTreeSet::new();
    let mut asymmetric: BTreeSet<String> = BTreeSet::new();
    let mut irreflexive: BTreeSet<String> = BTreeSet::new();
    let mut plain_props: BTreeSet<String> = BTreeSet::new();
    let mut datatype_roots: BTreeSet<String> = BTreeSet::new();
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
            // rdf:Property **没说**是对象还是数据。跟显式 owl:ObjectProperty
            // 分开存：那个是"说了"，不该被 range 改判；这个是"没说"，
            // 下面按 range 定通道，range 也没有才兜底当关系
            "http://www.w3.org/1999/02/22-rdf-syntax-ns#Property" => {
                plain_props.insert(t.subject.clone());
            }
            x if x == format!("{OWL}FunctionalProperty") => {
                functional.insert(t.subject.clone());
            }
            x if x == format!("{OWL}InverseFunctionalProperty") => {
                inverse_functional.insert(t.subject.clone());
            }
            // 属性公理:R0 的一致性检查靠它们才谈得上判定。
            //
            // **五个随包词表只覆盖了其中一半**(TransitiveProperty 14 条、
            // SymmetricProperty 1 条,而 Asymmetric 与 Irreflexive 一条没有),
            // 但那五个只是冷启动的底座。这条线的终点是把**企业自己的本体**
            // 导进来(0001 开篇:FIBO、行业标准、Protégé 自建),那些里
            // 反对称与非自反是常见声明。按 OWL 收全,而不是按手上这几个包收。
            x if x == format!("{OWL}TransitiveProperty") => {
                transitive.insert(t.subject.clone());
            }
            x if x == format!("{OWL}SymmetricProperty") => {
                symmetric.insert(t.subject.clone());
            }
            x if x == format!("{OWL}AsymmetricProperty") => {
                asymmetric.insert(t.subject.clone());
            }
            x if x == format!("{OWL}IrreflexiveProperty") => {
                irreflexive.insert(t.subject.clone());
            }
            // 词汇表自报的数据类型。这里只收显式声明的那几个根，
            // 子类（Integer ⊂ Number、URL ⊂ Text）等父类图收完再往下传。
            // **标记类自己也收**：schema:DataType 的声明是 `a rdfs:Class`，
            // 不收就会剩下一个叫 data_type 的实体类型；顺带让
            // `rdfs:subClassOf schema:DataType` 这种写法也落进闭包
            x if is_schema(x, "DataType") => {
                datatype_roots.insert(t.subject.clone());
                datatype_roots.insert(x.to_string());
            }
            _ => {}
        }
    }

    // 再收集标签、注释、父类、domain/range
    let mut labels: BTreeMap<String, Vec<(String, Option<String>)>> = BTreeMap::new();
    let mut comments: BTreeMap<String, Vec<(String, Option<String>)>> = BTreeMap::new();
    let mut parents: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut disjoint: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut domains: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut ranges: BTreeMap<String, Vec<String>> = BTreeMap::new();
    // rangeIncludes 用过的属性，它的 range 按并集读
    let mut union_ranged: BTreeSet<String> = BTreeSet::new();
    let known = |p: &str| {
        p == RDF_TYPE
            || p == format!("{RDFS}label")
            || p == format!("{RDFS}comment")
            || p == format!("{RDFS}subClassOf")
            || p == format!("{RDFS}subPropertyOf")
            || p == format!("{RDFS}domain")
            || p == format!("{RDFS}range")
            || is_schema(p, "domainIncludes")
            || is_schema(p, "rangeIncludes")
            || p == format!("{OWL}disjointWith")
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
        } else if p == format!("{OWL}disjointWith") {
            // **两个方向都记。** `owl:disjointWith` 是对称的,而词表通常只写一遍
            // (W3C Org 的 Role/Membership/Site/ChangeEvent 四者两两互斥,只写了六行)。
            // 只按写的方向存,查"A 与 B 互斥吗"就得看调用方碰巧从哪一头问
            if let Some(o) = &t.object_iri {
                disjoint
                    .entry(t.subject.clone())
                    .or_default()
                    .push(o.clone());
                disjoint
                    .entry(o.clone())
                    .or_default()
                    .push(t.subject.clone());
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
        } else if is_schema(p, "domainIncludes") {
            // domainIncludes 是并集（"可以用在这些类型上"），而我们的 domain
            // 列表本来就是并集语义（签名 `person|organization`），直接进
            if let Some(o) = &t.object_iri {
                domains
                    .entry(t.subject.clone())
                    .or_default()
                    .push(o.clone());
            }
        } else if is_schema(p, "rangeIncludes") {
            if let Some(o) = &t.object_iri {
                ranges.entry(t.subject.clone()).or_default().push(o.clone());
                union_ranged.insert(t.subject.clone());
            }
        } else if !known(p) {
            // 报告而不是丢弃：预览页要能说清"这个文件里还有什么我们没消费"
            *proj.unprojected.entry(p.to_string()).or_insert(0) += 1;
        }
    }

    // 数据类型闭包：根是显式 `a schema:DataType` 的那几个，子类沿 subClassOf
    // 往下传（Integer ⊂ Number、URL ⊂ Text）。
    // **哪些 IRI 是数据类型由文件自己说**，写死的只有"这个数据类型算我们四种里的
    // 哪一种"——那一半是命名约定，RDF 里推不出来
    let mut datatype_classes = datatype_roots.clone();
    loop {
        let grown: Vec<String> = parents
            .iter()
            .filter(|(child, ps)| {
                !datatype_classes.contains(*child)
                    && ps.iter().any(|p| datatype_classes.contains(p))
            })
            .map(|(child, _)| child.clone())
            .collect();
        if grown.is_empty() {
            break;
        }
        datatype_classes.extend(grown);
    }
    for iri in &datatype_classes {
        if let Some(dt) = name_datatype(iri, &parents, &datatype_classes) {
            proj.vocab_datatypes.insert(iri.clone(), dt);
        }
    }

    for iri in &classes {
        // **数据类型不是实体类型。** schema:Text 声明的是
        // `a rdfs:Class, schema:DataType`，只看前半截就会建出叫
        // `text`、`number`、`boolean` 的实体类型来
        if datatype_classes.contains(iri) {
            continue;
        }
        proj.classes.push(OwlClass {
            key: key_from_iri(iri),
            label: pick_lang(labels.get(iri)).unwrap_or_else(|| local_name(iri).to_string()),
            description: pick_lang(comments.get(iri)).unwrap_or_default(),
            parents: parents.get(iri).cloned().unwrap_or_default(),
            disjoint_with: {
                // 去重:两个方向都收之后,词表若两边都写过就会重复
                let mut d = disjoint.get(iri).cloned().unwrap_or_default();
                d.sort();
                d.dedup();
                d
            },
            iri: iri.clone(),
        });
    }
    // **并集去重，不是两个集合首尾相接**：词汇表常把同一个属性既声明为
    // rdf:Property 又声明为 owl:DatatypeProperty（FOAF 的 name、age、nick… 都这样），
    // 两个集合各收一次，chain 就会把它吐两遍。分类看 data_props 就够了。
    let all_props: BTreeSet<&String> = obj_props
        .iter()
        .chain(data_props.iter())
        .chain(plain_props.iter())
        .collect();
    for iri in all_props {
        let rs = ranges.get(iri).cloned().unwrap_or_default();
        // 走属性通道还是关系通道。显式声明说了算；没说的看 range：
        // **全是数据类型才算属性**。并集里只要有一个类就走关系——
        // `address` 的 range 是 `PostalAddress|Text`，判成属性就把那条边
        // 永久丢了，而关系总能给一个新实体起名字。富的那一侧可回退，
        // 穷的那一侧回不去
        let is_datatype = if data_props.contains(iri) {
            true
        } else if obj_props.contains(iri) {
            false
        } else {
            !rs.is_empty() && rs.iter().all(|r| datatype_classes.contains(r))
        };
        proj.properties.push(OwlProperty {
            key: key_from_iri(iri),
            label: pick_lang(labels.get(iri))
                .unwrap_or_else(|| local_name(iri).replace('_', " ").to_string()),
            description: pick_lang(comments.get(iri)).unwrap_or_default(),
            is_datatype,
            functional: functional.contains(iri),
            inverse_functional: inverse_functional.contains(iri),
            transitive: transitive.contains(iri),
            symmetric: symmetric.contains(iri),
            asymmetric: asymmetric.contains(iri),
            irreflexive: irreflexive.contains(iri),
            domains: domains.get(iri).cloned().unwrap_or_default(),
            ranges: rs,
            ranges_union: union_ranged.contains(iri),
            iri: iri.clone(),
        });
    }
    order_by_home_namespace(&mut proj);
    Ok(proj)
}

/// 把**这份文件自己的**词汇表排到前面。
///
/// 撞 key 时调用方是先到先得（`owl_import::plan` 里那个 `claimed`），而"先"
/// 此前来自 IRI 的字典序——于是 `http://` 排在 `https://` 前面，被引用的
/// 小词汇表系统性地压过主词汇表。schema.org 那份文件里合了 50 个命名空间，
/// 主词汇表声明了其中 94% 的词，却输掉了 141 场撞车里的 114 场：
/// `location` 输给 OMG Commons、`country` 输给 unece.org、
/// `organization` 输给 purl.org。丢的正是最该用的那批词。
///
/// 判据是**声明得最多的那个命名空间就是文件的主人**——不认 schema.org
/// 这个名字，任何词汇表都适用。同数时取字典序小的，保证可重复。
fn order_by_home_namespace(proj: &mut OwlProjection) {
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for iri in proj
        .classes
        .iter()
        .map(|c| c.iri.as_str())
        .chain(proj.properties.iter().map(|p| p.iri.as_str()))
    {
        *counts.entry(namespace_of(iri)).or_insert(0) += 1;
    }
    // max_by_key 取的是最后一个最大值，而 BTreeMap 按 key 升序——
    // 于是同数时拿到字典序最大的那个。要的是最小的，所以自己比
    let Some(home) = counts
        .into_iter()
        .fold(None::<(&str, usize)>, |best, (ns, n)| match best {
            Some((_, bn)) if bn >= n => best,
            _ => Some((ns, n)),
        })
        .map(|(ns, _)| ns.to_string())
    else {
        return;
    };
    // 稳定排序：只把主词汇表提到前面，其余保持原有的字典序
    proj.classes.sort_by_key(|c| namespace_of(&c.iri) != home);
    proj.properties
        .sort_by_key(|p| namespace_of(&p.iri) != home);
}

/// IRI 去掉局部名剩下的那段（含结尾的 `#` 或 `/`）。
fn namespace_of(iri: &str) -> &str {
    match iri.rfind(['#', '/']) {
        Some(i) => &iri[..=i],
        None => iri,
    }
}

/// 数据类型 IRI → 我们的四种。自身叫得上名就用自身，否则沿 `rdfs:subClassOf`
/// 上溯（Integer → Number、URL → Text）。
///
/// 叫不上名的返回 `None`——比如 `schema:Time`，它只有时刻没有日期，
/// 跟 `xsd:time` 同样待遇：由 [`map_range`] 走 [`RangeMapping::Degraded`]，
/// 按 text 建**并报告**。
fn name_datatype(
    iri: &str,
    parents: &BTreeMap<String, Vec<String>>,
    datatypes: &BTreeSet<String>,
) -> Option<&'static str> {
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    let mut stack: Vec<&str> = vec![iri];
    while let Some(cur) = stack.pop() {
        if !seen.insert(cur) {
            continue;
        }
        if let Some(dt) = well_known_datatype(cur) {
            return Some(dt);
        }
        // 只沿数据类型往上走：数据类型的父类可能是普通的 rdfs:Class
        //（schema:DataType ⊂ rdfs:Class），越过去就走进整个类层次了
        for p in parents.get(cur).into_iter().flatten() {
            if datatypes.contains(p) {
                stack.push(p);
            }
        }
    }
    None
}

/// 写死的那一半：数据类型的**名字**对应我们哪一种。
///
/// 这从 RDF 里推不出来——文件说得出"Integer 是个数据类型"，
/// 说不出"它是数字不是日期"。表里只放根，子类靠 subClassOf 上溯够到。
///
/// 这里按局部名匹配是安全的，跟 [`datatype_of`] 那条"必须按完整 IRI 匹配"
/// 不冲突：能走到这儿的 IRI，是**这份文件自己**声明成数据类型的，
/// 一个文件说 "Date 是数据类型" 就不会同时拿 Date 当实体类。
fn well_known_datatype(iri: &str) -> Option<&'static str> {
    [
        ("Text", "text"),
        ("Number", "number"),
        ("Date", "date"),
        ("DateTime", "date"),
        ("Boolean", "bool"),
    ]
    .into_iter()
    .find(|(local, _)| is_schema(iri, local))
    .map(|(_, dt)| dt)
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
    /// 写了 range，而**抽取器永远不可能从散文里读出这个值**：二进制块、
    /// XML 片段、XML 内部标识。跳过它保护不了任何数据（本来就不会有值），
    /// 省掉的是提示词——每个属性都是抽取提示词里的一行，每个文本块付一遍
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
    resolve_range(ranges, false, &VocabDatatypes::new())
}

/// 按属性**自己的** range 语义来解。
///
/// `rdfs:range` 写多条是交集，`schema:rangeIncludes` 写多条是并集，两者都落在
/// 同一个 `ranges` 里——不看 [`OwlProperty::ranges_union`] 这一位就会把
/// `author rangeIncludes Organization, Person` 读成"必须既是组织又是人"。
pub fn map_range_of(p: &OwlProperty, vocab: &VocabDatatypes) -> RangeMapping {
    resolve_range(&p.ranges, p.ranges_union, vocab)
}

fn resolve_range(ranges: &[String], union: bool, vocab: &VocabDatatypes) -> RangeMapping {
    // 词汇表自报的数据类型先查（schema:Text → text），查不到再走 xsd/owl 那套标准的
    let named = |iri: &String| vocab.get(iri).copied().or_else(|| datatype_of(iri));
    match ranges {
        [] => RangeMapping::Absent,
        [one] => match named(one) {
            Some(dt) => RangeMapping::Datatype(dt),
            None if unusable(one) => RangeMapping::Unusable(one.clone()),
            None => RangeMapping::Degraded(one.clone()),
        },
        many if union => {
            // 并集：全指向同一种就是那一种（Text ∪ URL 都是 text）。
            // 不一致则降级成 text 并报告——text 是任意并集的诚实上界
            let dts: Vec<Option<&'static str>> = many.iter().map(named).collect();
            match dts[0] {
                Some(dt) if dts.iter().all(|d| *d == Some(dt)) => RangeMapping::Datatype(dt),
                _ if many.iter().all(|r| unusable(r)) => RangeMapping::Unusable(many.join(" ∪ ")),
                _ => RangeMapping::Degraded(many.join(" ∪ ")),
            }
        }
        // rdfs:range 写多条是交集语义，不猜类型——但值仍是字面量，所以按 text 建
        many => RangeMapping::Degraded(many.join(" ∩ ")),
    }
}

/// 抽取器不可能从散文里读出来的那些：二进制块与 XML 内部管道。
///
/// 判据不是"这个值该不该存"——属性值本来就存在图谱里（走 `facts.object_value`，
/// 证据、时态、审阅全套）。判据是**会不会有值**：「门店每天 9:00 开门」里有
/// `09:00`，而一张 base64 平面图不会出现在散文里；就算文档里真有一段 base64，
/// 把它当成事实抽出来也是错的。
///
/// **其余一律降级成 text**——一个抽得出来的值，宁可类型糙一点也别让它没处可去。
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
        ex:Employee owl:equivalentClass ex:Staff .
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

        // 消费不了的公理进报告，不是丢弃也不是报错。
        //
        // `disjointWith` 曾经在这份名单上,现在被消费了(它是一致性检查的判定
        // 依据)——所以这里改用一个仍然消费不了的公理来守这条性质本身
        assert!(p
            .unprojected
            .contains_key("http://www.w3.org/2002/07/owl#equivalentClass"));
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

    /// schema.org 那一套的最小复刻：数据类型自报家门，属性用
    /// domainIncludes / rangeIncludes，全部只声明成 rdf:Property。
    const SCHEMA_ISH: &str = r#"
@prefix rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix schema: <https://schema.org/> .

schema:DataType a rdfs:Class .
schema:Text a rdfs:Class, schema:DataType .
schema:Number a rdfs:Class, schema:DataType .
schema:Date a rdfs:Class, schema:DataType .
schema:Time a rdfs:Class, schema:DataType .
schema:URL a rdfs:Class ; rdfs:subClassOf schema:Text .
schema:Integer a rdfs:Class ; rdfs:subClassOf schema:Number .

schema:Organization a rdfs:Class ; rdfs:label "Organization" .
schema:Person a rdfs:Class ; rdfs:label "Person" .
schema:PostalAddress a rdfs:Class ; rdfs:label "PostalAddress" .

schema:foundingDate a rdf:Property ;
    schema:domainIncludes schema:Organization ;
    schema:rangeIncludes schema:Date .
schema:author a rdf:Property ;
    schema:domainIncludes schema:Organization ;
    schema:rangeIncludes schema:Organization, schema:Person .
schema:address a rdf:Property ;
    schema:domainIncludes schema:Organization ;
    schema:rangeIncludes schema:PostalAddress, schema:Text .
schema:homepage a rdf:Property ;
    schema:domainIncludes schema:Person ;
    schema:rangeIncludes schema:Text, schema:URL .
schema:opens a rdf:Property ;
    schema:domainIncludes schema:Organization ;
    schema:rangeIncludes schema:Time .
schema:knows a rdf:Property ;
    schema:domainIncludes schema:Person .
"#;

    fn schema_ish() -> OwlProjection {
        project(SCHEMA_ISH.as_bytes(), RdfFormat::Turtle).unwrap()
    }

    fn prop<'a>(p: &'a OwlProjection, key: &str) -> &'a OwlProperty {
        p.properties.iter().find(|x| x.key == key).unwrap()
    }

    #[test]
    fn schema_org_datatypes_are_not_entity_types() {
        let p = schema_ish();
        let keys: Vec<&str> = p.classes.iter().map(|c| c.key.as_str()).collect();
        // Text / Number / Date / Time 声明的是 `a rdfs:Class, schema:DataType`，
        // 只看前半截就会建出叫 text、number 的实体类型来
        for gone in [
            "text",
            "number",
            "date",
            "time",
            "url",
            "integer",
            "data_type",
        ] {
            assert!(!keys.contains(&gone), "{gone} 不该是实体类型：{keys:?}");
        }
        assert!(keys.contains(&"organization") && keys.contains(&"person"));
    }

    #[test]
    fn domain_includes_feeds_the_signature() {
        let p = schema_ish();
        // 不认 schema:domainIncludes 的话这里是空的，签名就成了 (* → *)
        assert_eq!(
            prop(&p, "founding_date").domains,
            vec!["https://schema.org/Organization".to_string()]
        );
    }

    #[test]
    fn a_union_range_of_classes_stays_a_relation() {
        let p = schema_ish();
        // rangeIncludes Organization, Person —— 并集，两个都是类
        assert!(!prop(&p, "author").is_datatype);
        // rangeIncludes PostalAddress, Text —— schema.org 的常见写法，
        // Text 是"懒得建实体就写个字符串"。判成属性就把这条边永久丢了
        assert!(!prop(&p, "address").is_datatype);
    }

    #[test]
    fn a_union_range_of_datatypes_becomes_an_attribute() {
        let p = schema_ish();
        let fd = prop(&p, "founding_date");
        assert!(fd.is_datatype);
        assert_eq!(
            map_range_of(fd, &p.vocab_datatypes),
            RangeMapping::Datatype("date")
        );
        // Text ∪ URL：URL ⊂ Text，两个都解到 text，所以不用降级也不用报告
        let hp = prop(&p, "homepage");
        assert!(hp.is_datatype);
        assert_eq!(
            map_range_of(hp, &p.vocab_datatypes),
            RangeMapping::Datatype("text")
        );
    }

    #[test]
    fn a_union_is_not_an_intersection() {
        let p = schema_ish();
        // 这是整件事最容易错的一步：两个 range 倒进同一个 Vec 之后，
        // 不看 ranges_union 就会当成 rdfs:range 的交集语义
        assert!(prop(&p, "author").ranges_union);
        assert_eq!(prop(&p, "author").ranges.len(), 2);
        // 而 rdfs:range 写两条仍然是交集
        assert!(matches!(
            map_range(&[format!("{XSD}string"), format!("{XSD}integer")]),
            RangeMapping::Degraded(ref s) if s.contains('∩')
        ));
    }

    #[test]
    fn an_unnamed_datatype_degrades_and_is_reported() {
        let p = schema_ish();
        // schema:Time 只有时刻没有日期，跟 xsd:time 一个待遇：是数据类型
        //（所以走属性通道、不当实体类型），但叫不上名 → 按 text 建**并报告**
        let o = prop(&p, "opens");
        assert!(o.is_datatype);
        assert!(matches!(
            map_range_of(o, &p.vocab_datatypes),
            RangeMapping::Degraded(ref s) if s.ends_with("Time")
        ));
    }

    #[test]
    fn a_bare_rdf_property_without_range_is_still_a_relation() {
        let p = schema_ish();
        // 没有 range 就没有判据，兜底仍是关系——宾语是 IRI 的远多于字面值
        let k = prop(&p, "knows");
        assert!(!k.is_datatype);
        assert!(k.ranges.is_empty());
    }

    #[test]
    fn schema_predicates_are_consumed_not_reported_as_unprojected() {
        let p = schema_ish();
        // 认了就不该再出现在"暂未投影"里，否则预览页会说
        // "还有 2312 条 domainIncludes 没消费"，而其实消费了
        for consumed in ["domainIncludes", "rangeIncludes"] {
            assert!(
                !p.unprojected.keys().any(|k| k.ends_with(consumed)),
                "{consumed} 应已消费：{:?}",
                p.unprojected
            );
        }
    }

    /// 一份文件里合了两个词汇表，主词汇表的 IRI 用 https、被引用的用 http——
    /// 字典序下 http 在前，主词汇表就输了。这是 schema.org 那份文件的形状。
    const TWO_VOCABS: &str = r#"
@prefix rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix home: <https://home.example/> .
@prefix cited: <http://cited.example/> .

home:Location a rdfs:Class ; rdfs:label "Location" .
home:Country a rdfs:Class ; rdfs:label "Country" .
home:Person a rdfs:Class ; rdfs:label "Person" .
home:Organization a rdfs:Class ; rdfs:label "Organization" .
cited:Location a rdfs:Class ; rdfs:label "Location (cited)" .

home:worksAt a rdf:Property ; rdfs:label "worksAt" .
home:knows a rdf:Property ; rdfs:label "knows" .
cited:worksAt a rdf:Property ; rdfs:label "worksAt (cited)" .
"#;

    #[test]
    fn the_files_own_vocabulary_wins_a_key_collision() {
        let p = project(TWO_VOCABS.as_bytes(), RdfFormat::Turtle).unwrap();
        // 撞车由调用方先到先得地裁决，所以顺序就是裁决。
        // 主词汇表声明得最多，它必须排在前面——否则 `http://` < `https://`
        // 这条字典序会让被引用的词汇表赢走 location、country、organization
        let first_location = p.classes.iter().find(|c| c.key == "location").unwrap();
        assert!(
            first_location.iri.starts_with("https://home.example/"),
            "输给了 {}",
            first_location.iri
        );
        let first_works = p.properties.iter().find(|x| x.key == "works_at").unwrap();
        assert!(first_works.iri.starts_with("https://home.example/"));
    }
}

#[cfg(test)]
mod axioms {
    use super::*;

    /// OWL 的属性公理与类互斥都要投影出来——它们是一致性检查（0002 R0）的**判定
    /// 依据**。没有它们，`A part_of B` 与 `B part_of A` 同时成立到底是矛盾还是
    /// 正常，无从判起：`alias_of` 双向是对的，`produces` 双向几乎肯定是错的，
    /// 而区分这两者的东西只能来自本体。
    const AX: &str = r#"
        @prefix owl: <http://www.w3.org/2002/07/owl#> .
        @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
        @prefix ex: <http://acme.example/ax#> .

        ex:Person a owl:Class ; owl:disjointWith ex:Organization .
        ex:Organization a owl:Class .
        ex:Document a owl:Class .

        ex:partOf   a owl:ObjectProperty, owl:TransitiveProperty, owl:AsymmetricProperty .
        ex:aliasOf  a owl:ObjectProperty, owl:SymmetricProperty .
        ex:reportsTo a owl:ObjectProperty, owl:IrreflexiveProperty .
        ex:plain    a owl:ObjectProperty .
    "#;

    fn proj() -> OwlProjection {
        project(AX.as_bytes(), RdfFormat::Turtle).unwrap()
    }

    #[test]
    fn property_axioms_survive_the_projection() {
        let p = proj();
        let by = |k: &str| p.properties.iter().find(|x| x.key == k).unwrap().clone();

        let part_of = by("part_of");
        assert!(part_of.transitive, "TransitiveProperty 要落到属性上");
        assert!(part_of.asymmetric, "AsymmetricProperty 同上");
        assert!(!part_of.symmetric);

        assert!(
            by("alias_of").symmetric,
            "对称属性双向出现是正确的，不是矛盾"
        );
        assert!(by("reports_to").irreflexive, "非自反：自己汇报给自己是矛盾");

        // 没声明的一律为假——**默认不是"未知"而是"没这条公理"**。
        // OWL 是开放世界，但一致性检查只能按写下来的判：没写就是没有依据，
        // 而没有依据时不报矛盾，比猜一个公理出来安全
        let plain = by("plain");
        assert!(!plain.transitive && !plain.symmetric);
        assert!(!plain.asymmetric && !plain.irreflexive);
    }

    /// **两个方向都要有。** 词表通常只写一遍（W3C Org 把四个类两两互斥写成六行），
    /// 只按书写方向存，查"A 与 B 互斥吗"就取决于调用方碰巧从哪一头问。
    #[test]
    fn disjointness_is_recorded_from_both_ends() {
        let p = proj();
        let d = |k: &str| {
            p.classes
                .iter()
                .find(|c| c.key == k)
                .unwrap()
                .disjoint_with
                .clone()
        };
        assert_eq!(d("person"), vec!["http://acme.example/ax#Organization"]);
        assert_eq!(d("organization"), vec!["http://acme.example/ax#Person"]);
        assert!(d("document").is_empty(), "没声明互斥的类不该凭空多出来");
    }
}

/// 拿**真包**验一遍,而不只是夹具。
///
/// 先例在 `pack_alignment::against_real_packs`:那一版靠真包抓出了四个凭记忆
/// 写错的 IRI。夹具只能证明"我写的 Turtle 我自己解析得了",真包能证明
/// "官方文件里那些写法我们接得住"——两者不是一回事。
#[cfg(test)]
mod against_real_packs {
    use super::*;
    use std::io::Read;

    fn load(name: &str) -> Vec<u8> {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../utopia-server/packs/");
        let f = std::fs::File::open(format!("{path}{name}")).expect("包文件在");
        let mut out = Vec::new();
        flate2::read::GzDecoder::new(f)
            .read_to_end(&mut out)
            .expect("解压");
        out
    }

    /// W3C Org 把 Organization / Role / Membership / Site / ChangeEvent 五者两两
    /// 互斥,官方文件里每对只写一次。**五个类每个都该看到四个对端**——只按书写
    /// 方向存的话,先被写到的那几个会缺对端,而缺哪个取决于文件里的行序。
    ///
    /// (写这条断言时我按 grep 目测数成了"四者",真包当场纠正:`Organization`
    /// 也在互斥集里。夹具证明不了这种事。)
    #[test]
    fn w3c_org_declares_four_mutually_disjoint_classes() {
        let p = project(&load("w3c-org.ttl.gz"), RdfFormat::Turtle).unwrap();
        for key in ["organization", "role", "membership", "site", "change_event"] {
            let c = p
                .classes
                .iter()
                .find(|c| c.key == key)
                .unwrap_or_else(|| panic!("{key} 该被投影出来"));
            assert_eq!(
                c.disjoint_with.len(),
                4,
                "{key} 该与另外四个互斥,实得 {:?}",
                c.disjoint_with
            );
        }
    }

    /// IOF Core 声明了一批传递属性(before/after/occursDuring…)。
    /// 这是 R0 环检测唯一有真实依据的地方
    #[test]
    fn iof_core_declares_transitive_properties() {
        let p = project(&load("iof-core.rdf.gz"), RdfFormat::RdfXml).unwrap();
        let n = p.properties.iter().filter(|x| x.transitive).count();
        assert!(n >= 8, "IOF 该有一批传递属性,实得 {n}");
    }

    /// FOAF 的 Person ⊥ Organization / Document —— **最常撞的那一对**。
    /// `classify_type_drift` 里那张手写表(person vs organization 判 Disjoint)
    /// 想表达的就是它,注释里也写着"公理落库后这张表应改从本体读"
    #[test]
    fn foaf_says_a_person_is_not_an_organization() {
        let p = project(&load("foaf.rdf.gz"), RdfFormat::RdfXml).unwrap();
        let person = p
            .classes
            .iter()
            .find(|c| c.key == "person")
            .expect("foaf:Person");
        assert!(
            person
                .disjoint_with
                .iter()
                .any(|d| d.ends_with("Organization")),
            "foaf:Person 该与 Organization 互斥,实得 {:?}",
            person.disjoint_with
        );
    }
}
