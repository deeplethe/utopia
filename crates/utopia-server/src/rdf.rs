//! 账本 → RDF（0020）。
//!
//! 读的人不是另一个 Utopia，是一个手里有三元组库、要问「这个结论凭什么」的人。
//! 所以导出的内容是**区间与出处**，不只是当下的那些边——一份读起来干净自信、
//! 却没说其中一半在三月被撤回过的图，比没有导出更坏。
//!
//! 三件事定了这份文件的形状：
//!
//! 1. **导入来的类和关系留着原 IRI**（`entity_types.iri` / `relation_types.iri`）。
//!    schema.org 的库导出去还是 `schema:Organization`，对面手里的词汇表对得上
//! 2. **区间挂在具体化语句上**，不是挂在三元组上。RDF-star 更自然但多数消费者
//!    还读不了，命名图在 Turtle 里根本没有——一份打不开的文件不叫导出
//! 3. **有标准词就不自造**：`prov:invalidatedAtTime` 说的正是我们记录轴上那件事，
//!    另起一个私名只会把一个大家都认识的概念藏起来

use std::io::Write;
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};
use oxrdf::vocab::{rdf, rdfs, xsd};
use oxrdf::{Literal, NamedNode, NamedNodeRef, Term, TripleRef};
use utopia_store::export::{
    ExportClass, ExportDerived, ExportDocument, ExportEntity, ExportFact, ExportRelation,
};
use uuid::Uuid;

const OWL: &str = "http://www.w3.org/2002/07/owl#";
const PROV: &str = "http://www.w3.org/ns/prov#";
const SCHEMA: &str = "https://schema.org/";
/// 没有标准词的那几样才落在这里：置信度、派生标记、模型原话。
///
/// **URN 而不是 http://**：域名还没定，而一个词汇表 IRI 一旦发出去就不该再改。
/// 指向一个我们并不提供的地址，比指向一个不承诺解引用的 URN 更糟
const UTOPIA: &str = "urn:utopia:ns:";

fn nn(iri: impl Into<String>) -> NamedNode {
    // 拼出来的 IRI 全部来自 uuid / 本体 key / 配置里的 base，越界的字符在
    // `Names::new` 就挡掉了；这里再失败只能是 bug，不该把整份导出变成 500
    NamedNode::new(iri.into()).expect("导出 IRI 必须合法")
}

fn owl(term: &str) -> NamedNode {
    nn(format!("{OWL}{term}"))
}
fn prov(term: &str) -> NamedNode {
    nn(format!("{PROV}{term}"))
}
fn schema(term: &str) -> NamedNode {
    nn(format!("{SCHEMA}{term}"))
}
fn utopia(term: &str) -> NamedNode {
    nn(format!("{UTOPIA}{term}"))
}

/// 这份导出里的 IRI 怎么造。
///
/// 缺省是 URN：`urn:utopia:kb:{kb}:entity:{uuid}`。**稳定优先于可解引用**——
/// 同一个库隔一年再导一次，两份文件必须对得上；而一个自部署的实例并不知道
/// 自己对外的地址是什么。按请求的 Host 头去造，等于让身份取决于走了哪个反代。
/// 部署方知道自己发布在哪时，`?base=https://…/` 换成 http IRI。
pub struct Names {
    prefix: String,
    sep: char,
}

impl Names {
    pub fn new(kb_id: Uuid, base: Option<&str>) -> Result<Self, String> {
        match base.map(str::trim).filter(|b| !b.is_empty()) {
            Some(base) => {
                if !(base.starts_with("http://") || base.starts_with("https://")) {
                    return Err("`base` must be an http(s) IRI".into());
                }
                if base.contains(['<', '>', '"', '{', '}', '|', '\\', '^', '`', ' ']) {
                    return Err("`base` contains characters that cannot appear in an IRI".into());
                }
                let trimmed = base.trim_end_matches('/');
                Ok(Self {
                    prefix: format!("{trimmed}/kb/{kb_id}/"),
                    sep: '/',
                })
            }
            None => Ok(Self {
                prefix: format!("urn:utopia:kb:{kb_id}:"),
                sep: ':',
            }),
        }
    }

    fn mint(&self, kind: &str, id: &str) -> NamedNode {
        nn(format!("{}{kind}{}{id}", self.prefix, self.sep))
    }

    pub fn entity(&self, id: Uuid) -> NamedNode {
        self.mint("entity", &id.to_string())
    }
    pub fn fact(&self, id: Uuid) -> NamedNode {
        self.mint("fact", &id.to_string())
    }
    pub fn derived(&self, id: Uuid) -> NamedNode {
        self.mint("derived", &id.to_string())
    }
    pub fn document(&self, id: Uuid) -> NamedNode {
        self.mint("document", &id.to_string())
    }
    pub fn rule(&self, id: Uuid) -> NamedNode {
        self.mint("rule", &id.to_string())
    }
    /// 本体自己长出来的类/关系用 **key** 而不是 uuid：key 是这个库内部就在用的
    /// 标识（`UNIQUE (kb_id, key)`，抽取提示词和 API 用的都是它），文件因此读得懂。
    /// 导入来的一律用原 IRI
    pub fn class(&self, c: &ExportClass) -> NamedNode {
        match c.iri.as_deref() {
            Some(iri) => NamedNode::new(iri).unwrap_or_else(|_| self.mint("class", &c.key)),
            None => self.mint("class", &c.key),
        }
    }
    pub fn relation(&self, r: &ExportRelation) -> NamedNode {
        match r.iri.as_deref() {
            Some(iri) => NamedNode::new(iri).unwrap_or_else(|_| self.mint("relation", &r.key)),
            None => self.mint("relation", &r.key),
        }
    }
}

/// 序列化器写进来的地方。写完一页就把攒下的字节取走发给客户端——
/// 几十万条事实不能先在内存里拼成一整个 String
#[derive(Clone, Default)]
pub struct SharedBuf(Arc<Mutex<Vec<u8>>>);

impl SharedBuf {
    pub fn take(&self) -> Vec<u8> {
        let mut guard = self.0.lock().expect("导出缓冲区被毒化");
        std::mem::take(&mut *guard)
    }
}

impl Write for SharedBuf {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .expect("导出缓冲区被毒化")
            .extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// 两种格式共用同一套三元组，只是落笔不同——`oxrdfio` 把两个序列化器收在
/// 同一个类型后面，所以这里没有分支。
pub struct Sink(oxrdfio::WriterQuadSerializer<SharedBuf>);

/// 前缀表。Turtle 里它决定文件读起来是 `schema:Organization` 还是一长串尖括号
const PREFIXES: [(&str, &str); 6] = [
    ("rdf", "http://www.w3.org/1999/02/22-rdf-syntax-ns#"),
    ("rdfs", "http://www.w3.org/2000/01/rdf-schema#"),
    ("owl", OWL),
    ("xsd", "http://www.w3.org/2001/XMLSchema#"),
    ("prov", PROV),
    ("schema", SCHEMA),
];

impl Sink {
    pub fn new(format: Format, buf: SharedBuf) -> Self {
        let mut s = oxrdfio::RdfSerializer::from_format(format.rdf_format());
        for (p, iri) in PREFIXES {
            s = s.with_prefix(p, iri).expect("前缀表是常量，不该解析失败");
        }
        s = s
            .with_prefix("utopia", UTOPIA)
            .expect("前缀表是常量，不该解析失败");
        Sink(s.for_writer(buf))
    }

    fn triple(&mut self, t: TripleRef<'_>) -> std::io::Result<()> {
        self.0
            .serialize_quad(t.in_graph(oxrdf::GraphNameRef::DefaultGraph))
    }

    /// `s p o`，o 是资源。
    fn r(&mut self, s: &NamedNode, p: &NamedNode, o: &NamedNode) -> std::io::Result<()> {
        self.triple(TripleRef::new(s.as_ref(), p.as_ref(), o.as_ref()))
    }

    /// `s p o`，o 是字面值。
    fn l(&mut self, s: &NamedNode, p: &NamedNode, o: &Literal) -> std::io::Result<()> {
        self.triple(TripleRef::new(s.as_ref(), p.as_ref(), o.as_ref()))
    }

    pub fn finish(self) -> std::io::Result<()> {
        self.0.finish().map(|_| ())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Turtle,
    JsonLd,
}

impl Format {
    fn rdf_format(self) -> oxrdfio::RdfFormat {
        match self {
            Format::Turtle => oxrdfio::RdfFormat::Turtle,
            // streaming profile：按主语一组一组往外写，不在内存里攒出整个文档。
            // 导出是这个仓库里唯一一处「一次输出可能比库还大」的地方
            Format::JsonLd => oxrdfio::RdfFormat::JsonLd {
                profile: oxrdfio::JsonLdProfile::Streaming.into(),
            },
        }
    }

    pub fn parse(raw: Option<&str>) -> Option<Self> {
        match raw.unwrap_or("turtle").trim().to_ascii_lowercase().as_str() {
            "turtle" | "ttl" => Some(Format::Turtle),
            "jsonld" | "json-ld" | "json" => Some(Format::JsonLd),
            _ => None,
        }
    }
    pub fn content_type(self) -> &'static str {
        match self {
            Format::Turtle => "text/turtle; charset=utf-8",
            Format::JsonLd => "application/ld+json",
        }
    }
    pub fn extension(self) -> &'static str {
        match self {
            Format::Turtle => "ttl",
            Format::JsonLd => "jsonld",
        }
    }
}

fn dt(at: DateTime<Utc>) -> Literal {
    Literal::new_typed_literal(
        at.to_rfc3339_opts(chrono::SecondsFormat::AutoSi, true),
        xsd::DATE_TIME,
    )
}

/// 世界时间按**当时量到的精度**落字面值：只知道年份就写 `xsd:gYear`。
/// 一律写成 `xsd:date` 等于替账本补上它从来没有过的确定性
fn world_time(at: DateTime<Utc>, precision: Option<&str>) -> Literal {
    let iso = at.format("%Y-%m-%d").to_string();
    match precision {
        Some("year") => Literal::new_typed_literal(iso[..4].to_string(), xsd::G_YEAR),
        Some("month") => Literal::new_typed_literal(iso[..7].to_string(), xsd::G_YEAR_MONTH),
        // 小时以下（0024）：xsd:dateTime 说不出「到分为止」，精度由 emit_validity 另写一条
        Some("hour" | "minute" | "second") => Literal::new_typed_literal(
            at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            xsd::DATE_TIME,
        ),
        _ => Literal::new_typed_literal(iso, xsd::DATE),
    }
}

/// 小时以下的精度：XSD 的类型说不出来，另写一条 `utopia:*Precision`。日期粒度不用——
/// gYear / gYearMonth / date 本身就是精度
fn sub_day(precision: Option<&str>) -> Option<&str> {
    precision.filter(|p| matches!(*p, "hour" | "minute" | "second"))
}

fn text(s: impl Into<String>) -> Literal {
    Literal::new_simple_literal(s.into())
}

fn confidence(c: f32) -> Literal {
    Literal::new_typed_literal(format!("{c:.2}"), xsd::DECIMAL)
}

fn flag(b: bool) -> Literal {
    Literal::new_typed_literal(if b { "true" } else { "false" }, xsd::BOOLEAN)
}

/// 一次导出里要反复查的东西：类与关系的 IRI、属性的值域。
pub struct Vocabulary {
    pub classes: Vec<(Uuid, NamedNode)>,
    pub relations: Vec<(Uuid, NamedNode, Option<String>, Option<String>)>,
}

impl Vocabulary {
    pub fn class(&self, id: Uuid) -> Option<&NamedNode> {
        self.classes.iter().find(|(i, _)| *i == id).map(|(_, n)| n)
    }
    pub fn relation(&self, id: Uuid) -> Option<&NamedNode> {
        self.relations
            .iter()
            .find(|(i, _, _, _)| *i == id)
            .map(|(_, n, _, _)| n)
    }
    /// (datatype, unit)
    fn literal_shape(&self, id: Uuid) -> (Option<&str>, Option<&str>) {
        self.relations
            .iter()
            .find(|(i, _, _, _)| *i == id)
            .map(|(_, _, d, u)| (d.as_deref(), u.as_deref()))
            .unwrap_or((None, None))
    }
}

pub fn vocabulary(
    names: &Names,
    classes: &[ExportClass],
    relations: &[ExportRelation],
) -> Vocabulary {
    Vocabulary {
        classes: classes.iter().map(|c| (c.id, names.class(c))).collect(),
        relations: relations
            .iter()
            .map(|r| (r.id, names.relation(r), r.datatype.clone(), r.unit.clone()))
            .collect(),
    }
}

pub fn emit_class(sink: &mut Sink, vocab: &Vocabulary, c: &ExportClass) -> std::io::Result<()> {
    let iri = match vocab.class(c.id) {
        Some(n) => n.clone(),
        None => return Ok(()),
    };
    sink.r(&iri, &nn(rdf::TYPE.as_str()), &owl("Class"))?;
    sink.l(&iri, &nn(rdfs::LABEL.as_str()), &text(c.label.clone()))?;
    if !c.description.is_empty() {
        sink.l(
            &iri,
            &nn(rdfs::COMMENT.as_str()),
            &text(c.description.clone()),
        )?;
    }
    for parent in &c.parents {
        if let Some(p) = vocab.class(*parent) {
            let p = p.clone();
            sink.r(&iri, &nn(rdfs::SUB_CLASS_OF.as_str()), &p)?;
        }
    }
    for other in &c.disjoint {
        if let Some(o) = vocab.class(*other) {
            let o = o.clone();
            sink.r(&iri, &owl("disjointWith"), &o)?;
        }
    }
    Ok(())
}

pub fn emit_relation(
    sink: &mut Sink,
    vocab: &Vocabulary,
    r: &ExportRelation,
) -> std::io::Result<()> {
    let iri = match vocab.relation(r.id) {
        Some(n) => n.clone(),
        None => return Ok(()),
    };
    let kind = if r.kind == "attribute" {
        owl("DatatypeProperty")
    } else {
        owl("ObjectProperty")
    };
    sink.r(&iri, &nn(rdf::TYPE.as_str()), &kind)?;
    sink.l(&iri, &nn(rdfs::LABEL.as_str()), &text(r.label.clone()))?;
    if !r.description.is_empty() {
        sink.l(
            &iri,
            &nn(rdfs::COMMENT.as_str()),
            &text(r.description.clone()),
        )?;
    }
    // 公理照抄，一条不落：一致性检查就是按它们跑的，读的人要能自己复算
    for (on, term) in [
        (r.functional, "FunctionalProperty"),
        (r.inverse_functional, "InverseFunctionalProperty"),
        (r.is_transitive, "TransitiveProperty"),
        (r.is_symmetric, "SymmetricProperty"),
        (r.is_asymmetric, "AsymmetricProperty"),
        (r.is_irreflexive, "IrreflexiveProperty"),
    ] {
        if on {
            sink.r(&iri, &nn(rdf::TYPE.as_str()), &owl(term))?;
        }
    }
    // 只复制已声明的边；不补反向声明或传递闭包，目标也只在本库词汇表中找。
    for (target, predicate) in [
        (r.inverse_of, owl("inverseOf")),
        (r.sub_property_of, nn(rdfs::SUB_PROPERTY_OF.as_str())),
    ] {
        if let Some(target) = target.and_then(|id| vocab.relation(id)) {
            sink.r(&iri, &predicate, target)?;
        }
    }
    // 时间语义也照抄（0031）：一个 event 谓词的事实两端是同一刻，一个 eternal 谓词的
    // 事实没有日期——读的人不看这一条，会把前者读成一天的状态、后者读成从不知何时起。
    // 状态是默认，不写
    if r.temporal != "state" {
        sink.l(&iri, &utopia("temporal"), &text(r.temporal.clone()))?;
    }
    for d in &r.domains {
        if let Some(c) = vocab.class(*d) {
            let c = c.clone();
            sink.r(&iri, &nn(rdfs::DOMAIN.as_str()), &c)?;
        }
    }
    for g in &r.ranges {
        if let Some(c) = vocab.class(*g) {
            let c = c.clone();
            sink.r(&iri, &nn(rdfs::RANGE.as_str()), &c)?;
        }
    }
    if let Some(unit) = &r.unit {
        sink.l(&iri, &utopia("unit"), &text(unit.clone()))?;
    }
    Ok(())
}

pub fn emit_entity(
    sink: &mut Sink,
    names: &Names,
    vocab: &Vocabulary,
    e: &ExportEntity,
) -> std::io::Result<()> {
    let iri = names.entity(e.id);
    sink.l(
        &iri,
        &nn(rdfs::LABEL.as_str()),
        &text(e.canonical_name.clone()),
    )?;
    // 类型可以没有（0009）：没判出来就不写，而不是补一个 owl:Thing 充数
    if let Some(t) = e.type_id.and_then(|t| vocab.class(t)) {
        let t = t.clone();
        sink.r(&iri, &nn(rdf::TYPE.as_str()), &t)?;
    }
    Ok(())
}

pub fn emit_document(sink: &mut Sink, names: &Names, d: &ExportDocument) -> std::io::Result<()> {
    let iri = names.document(d.id);
    sink.r(&iri, &nn(rdf::TYPE.as_str()), &prov("Entity"))?;
    sink.l(&iri, &nn(rdfs::LABEL.as_str()), &text(d.filename.clone()))?;
    if let Some(key) = &d.external_key {
        sink.l(&iri, &utopia("externalKey"), &text(key.clone()))?;
    }
    if let Some(t) = d.doc_time {
        sink.l(&iri, &schema("datePublished"), &world_time(t, Some("day")))?;
    }
    sink.l(&iri, &prov("generatedAtTime"), &dt(d.created_at))?;
    // 删掉的文档仍在文件里：它的事实还挂着它当出处，抹掉出处等于抹掉证据链
    if let Some(t) = d.deleted_at {
        sink.l(&iri, &prov("invalidatedAtTime"), &dt(t))?;
    }
    Ok(())
}

/// 一条事实：具体化语句（必出）+ 现行三元组（只在现在仍持有且仍有效时出）。
pub fn emit_fact(
    sink: &mut Sink,
    names: &Names,
    vocab: &Vocabulary,
    f: &ExportFact,
    now: DateTime<Utc>,
) -> std::io::Result<()> {
    let stmt = names.fact(f.id);
    let subject = names.entity(f.subject_id);
    let predicate = f.predicate_id.and_then(|p| vocab.relation(p)).cloned();
    let object: Option<Term> = match (f.object_id, &f.object_value) {
        (Some(o), _) => Some(names.entity(o).into()),
        (None, Some(v)) => {
            // An unbound statement still has an object; only its datatype is unknown.
            let datatype = f.predicate_id.and_then(|p| vocab.literal_shape(p).0);
            Some(literal_value(v, datatype).into())
        }
        _ => None,
    };

    sink.r(&stmt, &nn(rdf::TYPE.as_str()), &nn(rdf::STATEMENT.as_str()))?;
    sink.r(&stmt, &nn(rdf::SUBJECT.as_str()), &subject)?;
    match (&predicate, &f.surface_predicate) {
        (Some(p), _) => sink.r(&stmt, &nn(rdf::PREDICATE.as_str()), p)?,
        // 本体没接住这条关系（0010）：**不造一个谓词**。原话作为字面值留在这里，
        // 读的人看得见「系统听见的是这个词，而词汇表里没有它」
        (None, Some(word)) => sink.l(&stmt, &utopia("proposedPredicate"), &text(word.clone()))?,
        (None, None) => {}
    }
    if let Some(o) = &object {
        sink.triple(TripleRef::new(stmt.as_ref(), rdf::OBJECT, o.as_ref()))?;
    }
    // 只相对一件事给出的值（「触发日后 45 天」，#681 §4）：字面量是原文，这一行说它不是日期
    if f.object_value.as_ref().is_some_and(is_relative) {
        sink.l(&stmt, &utopia("relativeValue"), &flag(true))?;
    }
    emit_validity(
        sink,
        &stmt,
        f.valid_from,
        f.valid_from_precision.as_deref(),
        f.valid_to,
        f.valid_to_precision.as_deref(),
    )?;
    sink.l(&stmt, &prov("generatedAtTime"), &dt(f.recorded_at))?;
    if let Some(t) = f.invalidated_at {
        sink.l(&stmt, &prov("invalidatedAtTime"), &dt(t))?;
    }
    sink.l(&stmt, &utopia("confidence"), &confidence(f.confidence))?;
    if let Some(old) = f.supersedes {
        let old = names.fact(old);
        sink.r(&stmt, &utopia("supersedes"), &old)?;
    }
    for doc in &f.documents {
        let d = names.document(*doc);
        sink.r(&stmt, &prov("wasDerivedFrom"), &d)?;
    }
    for quote in &f.quotes {
        sink.l(&stmt, &utopia("quote"), &text(quote.clone()))?;
    }
    // 引文从哪来（0040）：审计的人要不问我们就分得清一句原文和一段看图描述（0020）
    for origin in &f.quote_origins {
        sink.l(&stmt, &utopia("evidenceOrigin"), &text(origin.clone()))?;
    }
    // 边上的属性（0037）：陈述节点上各多一行，谓词是属性的 IRI，字面量按它的 datatype
    for q in &f.qualifiers {
        let Some(p) = vocab.relation(q.qualifier_type_id) else {
            continue;
        };
        if let Some(v) = &q.value {
            let (datatype, _) = vocab.literal_shape(q.qualifier_type_id);
            sink.l(&stmt, p, &literal_value(v, datatype))?;
        } else if let Some(e) = q.entity_id {
            sink.r(&stmt, p, &names.entity(e))?;
        }
    }

    // 现行三元组：**仍被持有，且现在仍成立**。区间已闭合或已撤回的不写这一条,
    // 否则一个忽略具体化的消费者会读到「张三现在还管着那个项目」。
    // 「现在成立」按读出来的区间判（0022）：没起点的从最早证据起，结束了不知哪天
    // 的到说出它的那份文档为止。规则在 world_axis 里，这里只看投影出来的两端
    let held = f.invalidated_at.is_none();
    let holds_now = f.holds_from.is_some_and(|t| t <= now) && f.holds_to.is_none_or(|t| t > now);
    if held && holds_now {
        if let (Some(p), Some(o)) = (&predicate, &object) {
            sink.triple(TripleRef::new(subject.as_ref(), p.as_ref(), o.as_ref()))?;
        }
    }
    Ok(())
}

/// 派生事实（0002）。**不写现行三元组**：它是推出来的，不是谁断言的；
/// 一个忽略具体化的消费者不该把引擎的结论当成文档里的话
pub fn emit_derived(
    sink: &mut Sink,
    names: &Names,
    vocab: &Vocabulary,
    d: &ExportDerived,
) -> std::io::Result<()> {
    let stmt = names.derived(d.id);
    // 推理活动的身份：公理规则有自己的 id，业务规则用它的。两者都要有一个
    // 稳定的 IRI，否则审计读到一条结论却指不出「凭什么」
    let rule = match (d.rule_id, d.attribute_rule_id) {
        (Some(r), _) => names.rule(r),
        (None, Some(r)) => names.rule(r),
        // 库里的 CHECK 保证不会两个都空；真到了这一步宁可跳过整条
        (None, None) => return Ok(()),
    };
    sink.r(&stmt, &nn(rdf::TYPE.as_str()), &nn(rdf::STATEMENT.as_str()))?;
    sink.r(
        &stmt,
        &nn(rdf::SUBJECT.as_str()),
        &names.entity(d.subject_id),
    )?;
    if let Some(p) = vocab.relation(d.predicate_id).cloned() {
        sink.r(&stmt, &nn(rdf::PREDICATE.as_str()), &p)?;
    }
    // 宾语两条通道，与断言事实同一套：实体走 IRI，字面值结论走字面量（0021）
    match (d.object_id, &d.object_value) {
        (Some(o), _) => sink.r(&stmt, &nn(rdf::OBJECT.as_str()), &names.entity(o))?,
        (None, Some(v)) => {
            let (datatype, _) = vocab.literal_shape(d.predicate_id);
            sink.l(
                &stmt,
                &nn(rdf::OBJECT.as_str()),
                &literal_value(v, datatype),
            )?;
        }
        (None, None) => {}
    }
    sink.l(&stmt, &utopia("derived"), &flag(true))?;
    emit_validity(
        sink,
        &stmt,
        d.valid_from,
        d.valid_from_precision.as_deref(),
        d.valid_to,
        d.valid_to_precision.as_deref(),
    )?;
    sink.l(&stmt, &prov("generatedAtTime"), &dt(d.derived_at))?;
    if let Some(t) = d.invalidated_at {
        sink.l(&stmt, &prov("invalidatedAtTime"), &dt(t))?;
    }
    sink.l(&stmt, &utopia("confidence"), &confidence(d.confidence))?;
    sink.r(&stmt, &prov("wasGeneratedBy"), &rule)?;
    sink.r(&rule, &nn(rdf::TYPE.as_str()), &prov("Activity"))?;
    // 标签用规则自己的名字（业务规则），公理退回它的种类名——审计读到的是
    // 「Gas-bearing well」而不是「business」
    sink.l(
        &rule,
        &nn(rdfs::LABEL.as_str()),
        &text(d.rule_name.clone().unwrap_or_else(|| d.rule.clone())),
    )?;
    // 前提。审计顺着 prov:used 往下走一步就到断言，再走一步就到句子
    for premise in &d.premises {
        let p = names.fact(*premise);
        sink.r(&stmt, &prov("used"), &p)?;
    }
    for premise in &d.premises_derived {
        let p = names.derived(*premise);
        sink.r(&stmt, &prov("used"), &p)?;
    }
    Ok(())
}

fn emit_validity(
    sink: &mut Sink,
    stmt: &NamedNode,
    from: Option<DateTime<Utc>>,
    from_precision: Option<&str>,
    to: Option<DateTime<Utc>>,
    to_precision: Option<&str>,
) -> std::io::Result<()> {
    if let Some(t) = from {
        sink.l(stmt, &schema("validFrom"), &world_time(t, from_precision))?;
        if let Some(p) = sub_day(from_precision) {
            sink.l(stmt, &utopia("validFromPrecision"), &text(p))?;
        }
    }
    match (to, to_precision) {
        (Some(t), p) => {
            sink.l(stmt, &schema("validThrough"), &world_time(t, p))?;
            if let Some(p) = sub_day(p) {
                sink.l(stmt, &utopia("validThroughPrecision"), &text(p))?;
            }
        }
        // 「结束了，但不知道哪天」——账本专门为它留了一个状态，导出不能把它
        // 压成「至今仍成立」（那正是 valid_to 一列承载两个意思时的老毛病）
        (None, Some("unknown")) => sink.l(stmt, &utopia("endedUnknown"), &flag(true))?,
        (None, _) => {}
    }
    Ok(())
}

/// 值上带着 `"relative": true`：原文只相对一件事给出它，没有日历上的日期
fn is_relative(v: &serde_json::Value) -> bool {
    v.get("relative").and_then(|r| r.as_bool()) == Some(true)
}

/// 属性事实的字面值。`{"value": …, "unit": …}` 或 `{"summary": …}`。
/// 相对的值写成普通字符串：`"45 days after the Trigger Date"^^xsd:date` 是个不合法的字面量
fn literal_value(v: &serde_json::Value, datatype: Option<&str>) -> Literal {
    let raw = v.get("value").unwrap_or(v);
    let as_text = match raw {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Null => v
            .get("summary")
            .and_then(|s| s.as_str())
            .unwrap_or_default()
            .to_string(),
        other => other.to_string(),
    };
    let ty: NamedNodeRef<'_> = match datatype {
        _ if is_relative(v) => xsd::STRING,
        Some("number") => xsd::DECIMAL,
        Some("date") => xsd::DATE,
        Some("bool") => xsd::BOOLEAN,
        _ => xsd::STRING,
    };
    Literal::new_typed_literal(as_text, ty)
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxrdf::Quad;

    fn kb() -> Uuid {
        Uuid::parse_str("01a06dc4-f40a-7013-b09f-1b499e2e7441").unwrap()
    }

    fn id(n: u8) -> Uuid {
        Uuid::from_bytes([n; 16])
    }

    fn at(s: &str) -> DateTime<Utc> {
        s.parse().unwrap()
    }

    fn class(n: u8, key: &str, iri: Option<&str>) -> ExportClass {
        ExportClass {
            id: id(n),
            key: key.into(),
            label: key.into(),
            description: String::new(),
            iri: iri.map(str::to_string),
            parents: vec![],
            disjoint: vec![],
        }
    }

    fn relation(n: u8, key: &str, iri: Option<&str>, kind: &str) -> ExportRelation {
        ExportRelation {
            id: id(n),
            key: key.into(),
            label: key.into(),
            description: String::new(),
            iri: iri.map(str::to_string),
            kind: kind.into(),
            datatype: (kind == "attribute").then(|| "number".to_string()),
            unit: None,
            temporal: "state".into(),
            functional: true,
            inverse_functional: false,
            is_transitive: false,
            is_symmetric: false,
            is_asymmetric: false,
            is_irreflexive: false,
            inverse_of: None,
            sub_property_of: None,
            domains: vec![],
            ranges: vec![],
        }
    }

    fn fact(n: u8) -> ExportFact {
        ExportFact {
            qualifiers: Vec::new(),
            id: id(n),
            subject_id: id(10),
            predicate_id: Some(id(2)),
            surface_predicate: None,
            object_id: Some(id(11)),
            object_value: None,
            valid_from: None,
            valid_from_precision: None,
            valid_to: None,
            valid_to_precision: None,
            // 读出来的区间（0022）：没起点就从锚点起，这里让它与记下的时刻同一天
            holds_from: Some(at("2026-01-01T00:00:00Z")),
            holds_to: None,
            recorded_at: at("2026-01-01T00:00:00Z"),
            invalidated_at: None,
            confidence: 0.9,
            supersedes: None,
            documents: vec![],
            quotes: vec![],
            quote_origins: vec![],
            subject_kb: Some(kb()),
            object_kb: Some(kb()),
            predicate_kb: Some(kb()),
            supersedes_kb: None,
            foreign_document: false,
            foreign_chunk: false,
            subject_merged: false,
            object_merged: false,
        }
    }

    /// 导出一遍再解析回来。**必须解析回来**：断言字符串里有没有某一段，
    /// 证明不了这份文件是不是合法的 Turtle，而那正是导出唯一要保证的事
    fn export(format: Format, emit: impl FnOnce(&mut Sink, &Names, &Vocabulary)) -> Vec<Quad> {
        let names = Names::new(kb(), None).unwrap();
        let classes = vec![
            class(1, "person", Some("https://schema.org/Person")),
            class(3, "team", None),
        ];
        let relations = vec![
            relation(
                2,
                "works_for",
                Some("https://schema.org/worksFor"),
                "relation",
            ),
            relation(4, "headcount", None, "attribute"),
        ];
        let vocab = vocabulary(&names, &classes, &relations);
        let buf = SharedBuf::default();
        let mut sink = Sink::new(format, buf.clone());
        for c in &classes {
            emit_class(&mut sink, &vocab, c).unwrap();
        }
        for r in &relations {
            emit_relation(&mut sink, &vocab, r).unwrap();
        }
        emit(&mut sink, &names, &vocab);
        sink.finish().unwrap();
        let bytes = buf.take();
        oxrdfio::RdfParser::from_format(match format {
            Format::Turtle => oxrdfio::RdfFormat::Turtle,
            Format::JsonLd => oxrdfio::RdfFormat::JsonLd {
                profile: oxrdfio::JsonLdProfileSet::empty(),
            },
        })
        .for_slice(&bytes)
        .map(|q| q.expect("导出的文件必须解析得回来"))
        .collect()
    }

    fn has(quads: &[Quad], s: &str, p: &str, o: &str) -> bool {
        quads.iter().any(|q| {
            q.subject.to_string() == s && q.predicate.to_string() == format!("<{p}>") && {
                let obj = q.object.to_string();
                obj == o || obj == format!("<{o}>")
            }
        })
    }

    fn objects(quads: &[Quad], s: &str, p: &str) -> Vec<String> {
        quads
            .iter()
            .filter(|q| q.subject.to_string() == s && q.predicate.to_string() == format!("<{p}>"))
            .map(|q| q.object.to_string())
            .collect()
    }

    const STMT: &str = "<urn:utopia:kb:01a06dc4-f40a-7013-b09f-1b499e2e7441:fact:05050505-0505-0505-0505-050505050505>";
    const SUBJ: &str = "<urn:utopia:kb:01a06dc4-f40a-7013-b09f-1b499e2e7441:entity:0a0a0a0a-0a0a-0a0a-0a0a-0a0a0a0a0a0a>";
    const OBJ: &str = "<urn:utopia:kb:01a06dc4-f40a-7013-b09f-1b499e2e7441:entity:0b0b0b0b-0b0b-0b0b-0b0b-0b0b0b0b0b0b>";
    const WORKS_FOR: &str = "https://schema.org/worksFor";

    #[test]
    fn unbound_literal_objects_survive_both_formats() {
        for value in [
            serde_json::json!({"value": "待复检"}),
            serde_json::json!({"value": "quote: \" and slash: \\"}),
            serde_json::json!({"value": ""}),
            serde_json::json!({"value": 0}),
            serde_json::json!({"value": false}),
            serde_json::json!({"value": null, "summary": "not specified"}),
            serde_json::Value::Null,
        ] {
            let mut f = fact(5);
            f.predicate_id = None;
            f.surface_predicate = Some("状态".into());
            f.object_id = None;
            f.object_value = Some(value.clone());
            f.documents = vec![id(20)];
            f.quotes = vec!["设备 A 待复检".into()];
            f.supersedes = Some(id(6));
            for retracted in [false, true] {
                f.invalidated_at = retracted.then(|| at("2026-02-01T00:00:00Z"));
                let mut sets = Vec::new();
                for format in [Format::Turtle, Format::JsonLd] {
                    let quads = export(format, |sink, names, vocab| {
                        emit_fact(sink, names, vocab, &f, at("2026-06-01T00:00:00Z")).unwrap();
                    });
                    assert_eq!(
                        objects(&quads, STMT, rdf::OBJECT.as_str()),
                        vec![literal_value(&value, None).to_string()],
                        "unbound statement lost its literal object: {value}"
                    );
                    assert!(objects(&quads, STMT, rdf::PREDICATE.as_str()).is_empty());
                    assert!(!quads.iter().any(|q| q.subject.to_string() == SUBJ));
                    sets.push(quads.into_iter().collect::<std::collections::HashSet<_>>());
                }
                assert_eq!(sets[0], sets[1]);
            }
        }
    }

    #[test]
    fn bound_literal_datatypes_survive_both_formats() {
        for (datatype, value, expected) in [
            (
                "number",
                serde_json::json!(0),
                Literal::new_typed_literal("0", xsd::DECIMAL),
            ),
            (
                "text",
                serde_json::json!("待复检"),
                Literal::new_simple_literal("待复检"),
            ),
            (
                "bool",
                serde_json::json!(false),
                Literal::new_typed_literal("false", xsd::BOOLEAN),
            ),
        ] {
            let mut f = fact(5);
            f.predicate_id = Some(id(4));
            f.object_id = None;
            f.object_value = Some(serde_json::json!({"value": value}));
            for format in [Format::Turtle, Format::JsonLd] {
                let quads = export(format, |sink, names, _| {
                    let mut property = relation(4, "value", None, "attribute");
                    property.datatype = Some(datatype.into());
                    let vocab = vocabulary(names, &[], &[property]);
                    emit_fact(sink, names, &vocab, &f, at("2026-06-01T00:00:00Z")).unwrap();
                });
                assert_eq!(
                    objects(&quads, STMT, rdf::OBJECT.as_str()),
                    vec![expected.to_string()]
                );
                assert!(quads.iter().any(|q| q.subject.to_string() == SUBJ
                    && q.object == Term::Literal(expected.clone())));
            }
        }
    }

    #[test]
    fn an_absent_object_is_not_an_empty_literal() {
        let mut f = fact(5);
        f.predicate_id = None;
        f.object_id = None;
        f.object_value = None;
        for format in [Format::Turtle, Format::JsonLd] {
            let quads = export(format, |sink, names, vocab| {
                emit_fact(sink, names, vocab, &f, at("2026-06-01T00:00:00Z")).unwrap();
            });
            assert!(objects(&quads, STMT, rdf::OBJECT.as_str()).is_empty());
        }
    }

    #[test]
    fn an_imported_class_keeps_its_own_iri() {
        let quads = export(Format::Turtle, |_, _, _| {});
        // 导入来的 schema.org 类导出去还是 schema:Person
        assert!(has(
            &quads,
            "<https://schema.org/Person>",
            "http://www.w3.org/1999/02/22-rdf-syntax-ns#type",
            "http://www.w3.org/2002/07/owl#Class"
        ));
        // 本体自己长的用 key 铸 IRI，读得懂
        assert!(has(
            &quads,
            "<urn:utopia:kb:01a06dc4-f40a-7013-b09f-1b499e2e7441:class:team>",
            "http://www.w3.org/1999/02/22-rdf-syntax-ns#type",
            "http://www.w3.org/2002/07/owl#Class"
        ));
        // 公理照抄：一致性检查按它跑，读的人要能自己复算
        assert!(has(
            &quads,
            &format!("<{WORKS_FOR}>"),
            "http://www.w3.org/1999/02/22-rdf-syntax-ns#type",
            "http://www.w3.org/2002/07/owl#FunctionalProperty"
        ));
    }

    #[test]
    fn a_live_fact_is_both_a_statement_and_a_triple() {
        let quads = export(Format::Turtle, |sink, names, vocab| {
            emit_fact(sink, names, vocab, &fact(5), at("2026-06-01T00:00:00Z")).unwrap();
        });
        assert!(has(
            &quads,
            STMT,
            "http://www.w3.org/1999/02/22-rdf-syntax-ns#type",
            "http://www.w3.org/1999/02/22-rdf-syntax-ns#Statement"
        ));
        assert!(has(
            &quads,
            STMT,
            "http://www.w3.org/1999/02/22-rdf-syntax-ns#subject",
            SUBJ
        ));
        assert!(
            has(&quads, SUBJ, WORKS_FOR, OBJ),
            "仍然成立的事实要有一条平铺三元组——不看具体化的消费者靠它拿到现状"
        );
        assert_eq!(
            objects(&quads, STMT, "urn:utopia:ns:confidence"),
            vec!["\"0.90\"^^<http://www.w3.org/2001/XMLSchema#decimal>"]
        );
    }

    #[test]
    fn a_closed_or_retracted_fact_is_a_statement_only() {
        // 区间已闭合：世界轴上它已经结束
        let mut closed = fact(5);
        closed.valid_from = Some(at("2023-01-01T00:00:00Z"));
        closed.valid_from_precision = Some("day".into());
        closed.valid_to = Some(at("2024-07-01T00:00:00Z"));
        closed.valid_to_precision = Some("day".into());
        // 读出来的区间随原文（0022）：两端都给了，就是它们
        closed.holds_from = Some(at("2023-01-01T00:00:00Z"));
        closed.holds_to = Some(at("2024-07-01T00:00:00Z"));
        let quads = export(Format::Turtle, |sink, names, vocab| {
            emit_fact(sink, names, vocab, &closed, at("2026-06-01T00:00:00Z")).unwrap();
        });
        assert!(
            !has(&quads, SUBJ, WORKS_FOR, OBJ),
            "结束了的关系不该以现在时写出去——那正是导出会骗人的地方"
        );
        assert_eq!(
            objects(&quads, STMT, "https://schema.org/validThrough"),
            vec!["\"2024-07-01\"^^<http://www.w3.org/2001/XMLSchema#date>"]
        );

        // 记录轴上被撤回：世界轴上它甚至还"开着"，但我们已经不这么认为了
        let mut retracted = fact(5);
        retracted.invalidated_at = Some(at("2026-03-01T00:00:00Z"));
        let quads = export(Format::Turtle, |sink, names, vocab| {
            emit_fact(sink, names, vocab, &retracted, at("2026-06-01T00:00:00Z")).unwrap();
        });
        assert!(!has(&quads, SUBJ, WORKS_FOR, OBJ), "撤回的不出平铺三元组");
        assert_eq!(
            objects(&quads, STMT, "http://www.w3.org/ns/prov#invalidatedAtTime"),
            vec!["\"2026-03-01T00:00:00Z\"^^<http://www.w3.org/2001/XMLSchema#dateTime>"]
        );
    }

    #[test]
    fn record_axis_subseconds_round_trip_without_changing_world_precision() {
        for timestamp in [
            "2026-09-20T00:00:00Z",
            "2026-09-20T00:00:00.100Z",
            "2026-09-20T00:00:00.100001Z",
            "2026-09-20T00:00:00.100002Z",
            "2026-09-20T00:00:00.123456789Z",
        ] {
            let original = at(timestamp);
            let literal = dt(original);
            assert_eq!(literal.datatype(), xsd::DATE_TIME);
            assert_eq!(literal.value().parse::<DateTime<Utc>>().unwrap(), original);
        }
        assert_eq!(
            dt(at("2026-09-20T00:00:00Z")).value(),
            "2026-09-20T00:00:00Z"
        );
        let instant = at("2026-09-20T12:34:56.123456Z");
        for (precision, lexical, datatype) in [
            ("year", "2026", xsd::G_YEAR),
            ("month", "2026-09", xsd::G_YEAR_MONTH),
            ("day", "2026-09-20", xsd::DATE),
            ("hour", "2026-09-20T12:34:56Z", xsd::DATE_TIME),
            ("minute", "2026-09-20T12:34:56Z", xsd::DATE_TIME),
            ("second", "2026-09-20T12:34:56Z", xsd::DATE_TIME),
        ] {
            assert_eq!(
                world_time(instant, Some(precision)),
                Literal::new_typed_literal(lexical, datatype)
            );
        }
    }

    #[test]
    fn a_year_stays_a_year() {
        let mut coarse = fact(5);
        coarse.valid_from = Some(at("2023-01-01T00:00:00Z"));
        coarse.valid_from_precision = Some("year".into());
        let quads = export(Format::Turtle, |sink, names, vocab| {
            emit_fact(sink, names, vocab, &coarse, at("2026-06-01T00:00:00Z")).unwrap();
        });
        // 只量到年份就写 gYear。一律写成 xsd:date 等于替账本补上它没有过的确定性
        assert_eq!(
            objects(&quads, STMT, "https://schema.org/validFrom"),
            vec!["\"2023\"^^<http://www.w3.org/2001/XMLSchema#gYear>"]
        );
    }

    #[test]
    fn an_ended_but_undated_relation_says_so() {
        let mut ended = fact(5);
        ended.valid_to = None;
        ended.valid_to_precision = Some("unknown".into());
        // 结束了不知哪天：读出来的终点是锚点（说出它的那份文档的日期），SQL 就这么投影
        ended.holds_to = ended.holds_from;
        let quads = export(Format::Turtle, |sink, names, vocab| {
            emit_fact(sink, names, vocab, &ended, at("2026-06-01T00:00:00Z")).unwrap();
        });
        // 「结束了，但不知道哪天」不能压成「至今仍成立」
        assert!(has(
            &quads,
            STMT,
            "urn:utopia:ns:endedUnknown",
            "\"true\"^^<http://www.w3.org/2001/XMLSchema#boolean>"
        ));
        assert!(!has(&quads, SUBJ, WORKS_FOR, OBJ));
    }

    #[test]
    fn a_predicate_the_ontology_never_accepted_is_not_invented() {
        let mut bare = fact(5);
        bare.predicate_id = None;
        bare.surface_predicate = Some("acquired".into());
        let quads = export(Format::Turtle, |sink, names, vocab| {
            emit_fact(sink, names, vocab, &bare, at("2026-06-01T00:00:00Z")).unwrap();
        });
        assert!(
            objects(
                &quads,
                STMT,
                "http://www.w3.org/1999/02/22-rdf-syntax-ns#predicate"
            )
            .is_empty(),
            "本体没有这条关系就不铸一个谓词出来（0010）"
        );
        assert_eq!(
            objects(&quads, STMT, "urn:utopia:ns:proposedPredicate"),
            vec!["\"acquired\""]
        );
    }

    #[test]
    fn an_attribute_carries_its_datatype() {
        let mut attr = fact(5);
        attr.predicate_id = Some(id(4));
        attr.object_id = None;
        attr.object_value = Some(serde_json::json!({ "value": 42, "unit": "people" }));
        let quads = export(Format::Turtle, |sink, names, vocab| {
            emit_fact(sink, names, vocab, &attr, at("2026-06-01T00:00:00Z")).unwrap();
        });
        assert_eq!(
            objects(
                &quads,
                STMT,
                "http://www.w3.org/1999/02/22-rdf-syntax-ns#object"
            ),
            vec!["\"42\"^^<http://www.w3.org/2001/XMLSchema#decimal>"]
        );
    }

    /// 日期属性上相对的值（#681 §4）导出成普通字符串，陈述上另有一行说它是相对的——
    /// 写成 xsd:date 的字面量不合法，严格的解析器会整份拒收
    #[test]
    fn a_relative_deadline_is_a_string_that_says_it_is_relative() {
        let dated = literal_value(&serde_json::json!({ "value": "2020-06-23" }), Some("date"));
        assert_eq!(dated.datatype(), xsd::DATE);
        let relative = literal_value(
            &serde_json::json!({ "value": "45 days after the Trigger Date", "relative": true }),
            Some("date"),
        );
        assert_eq!(relative.datatype(), xsd::STRING);
        assert_eq!(relative.value(), "45 days after the Trigger Date");

        let mut attr = fact(5);
        attr.predicate_id = Some(id(4));
        attr.object_id = None;
        attr.object_value = Some(
            serde_json::json!({ "value": "45 days after the Trigger Date", "relative": true }),
        );
        let quads = export(Format::Turtle, |sink, names, vocab| {
            emit_fact(sink, names, vocab, &attr, at("2026-06-01T00:00:00Z")).unwrap();
        });
        assert!(has(
            &quads,
            STMT,
            "urn:utopia:ns:relativeValue",
            "\"true\"^^<http://www.w3.org/2001/XMLSchema#boolean>"
        ));
    }

    /// 业务规则的结论也要出现在导出里，而且宾语是**字面值**。
    ///
    /// 这一条挡的是一次静默丢失：取数那边原本 `JOIN rules`，而业务规则的
    /// `rule_id` 是 NULL——整条结论会被内连接挡在文件之外，而 0020 承诺的正是
    /// 「审计员不靠我们也能读全」。活动的标签也要是规则自己的名字，
    /// 「business」对读的人没有意义
    #[test]
    fn a_rule_conclusion_reaches_the_export_as_a_literal() {
        let derived = ExportDerived {
            id: id(7),
            subject_id: id(10),
            predicate_id: id(2),
            object_id: None,
            object_value: Some(serde_json::json!({ "class": "gas_well" })),
            rule_id: None,
            attribute_rule_id: Some(id(9)),
            valid_from: None,
            valid_from_precision: None,
            valid_to: None,
            valid_to_precision: None,
            derived_at: at("2026-02-01T00:00:00Z"),
            invalidated_at: None,
            confidence: 0.9,
            rule: "business".into(),
            rule_name: Some("Gas-bearing well".into()),
            premises: vec![id(5)],
            premises_derived: Vec::new(),
            subject_kb: Some(kb()),
            object_kb: None,
            predicate_kb: Some(kb()),
            rule_kb: None,
            attribute_rule_kb: Some(kb()),
            foreign_fact_premise: false,
            foreign_derived_premise: false,
            subject_merged: false,
            object_merged: false,
        };
        let quads = export(Format::Turtle, |sink, names, vocab| {
            emit_derived(sink, names, vocab, &derived).unwrap();
        });
        let stmt = "<urn:utopia:kb:01a06dc4-f40a-7013-b09f-1b499e2e7441:derived:07070707-0707-0707-0707-070707070707>";
        // 派生标记还在——它仍旧是推出来的，不是谁断言的
        assert!(has(
            &quads,
            stmt,
            "urn:utopia:ns:derived",
            "\"true\"^^<http://www.w3.org/2001/XMLSchema#boolean>"
        ));
        // 宾语是字面值而不是一个实体 IRI
        let obj = objects(
            &quads,
            stmt,
            "http://www.w3.org/1999/02/22-rdf-syntax-ns#object",
        );
        assert_eq!(obj.len(), 1, "结论要有宾语");
        assert!(
            obj[0].starts_with('"'),
            "字面值结论的宾语该是字面量，拿到的是 {}",
            obj[0]
        );
        // 前提照常挂着：审计顺着 prov:used 走得到那两条读数
        assert_eq!(
            objects(&quads, stmt, "http://www.w3.org/ns/prov#used").len(),
            1
        );
        // 活动的标签是规则的名字
        let rule_iri =
            "<urn:utopia:kb:01a06dc4-f40a-7013-b09f-1b499e2e7441:rule:09090909-0909-0909-0909-090909090909>";
        assert_eq!(
            objects(
                &quads,
                rule_iri,
                "http://www.w3.org/2000/01/rdf-schema#label"
            ),
            vec!["\"Gas-bearing well\""],
            "推理活动要以规则名示人"
        );
    }

    #[test]
    fn declared_property_links_are_local_explicit_and_order_independent() {
        let root = relation(21, "root", Some("https://example.test/root"), "relation");
        let mut inverse = relation(22, "inverse", None, "relation");
        inverse.inverse_of = Some(root.id);
        let mut child = relation(23, "child", None, "relation");
        child.sub_property_of = Some(root.id);
        let mut leaf = relation(24, "leaf", None, "relation");
        leaf.sub_property_of = Some(child.id);
        let mut missing = relation(25, "unresolved", None, "relation");
        missing.inverse_of = Some(id(98));
        missing.sub_property_of = Some(id(99));
        let mut relations = vec![root, inverse, child, leaf, missing];
        let mut sets = Vec::new();
        for reverse in [false, true] {
            if reverse {
                relations.reverse();
            }
            for format in [Format::Turtle, Format::JsonLd] {
                let quads = export(format, |sink, names, _| {
                    let vocab = vocabulary(names, &[], &relations);
                    for r in &relations {
                        emit_relation(sink, &vocab, r).unwrap();
                    }
                });
                let names = Names::new(kb(), None).unwrap();
                let iri = |n| names.relation(relations.iter().find(|r| r.id == id(n)).unwrap());
                let expected: std::collections::HashSet<_> = [
                    (iri(22).into(), owl("inverseOf"), Term::from(iri(21))),
                    (
                        iri(23).into(),
                        nn(rdfs::SUB_PROPERTY_OF.as_str()),
                        Term::from(iri(21)),
                    ),
                    (
                        iri(24).into(),
                        nn(rdfs::SUB_PROPERTY_OF.as_str()),
                        Term::from(iri(23)),
                    ),
                ]
                .into_iter()
                .collect();
                let links: std::collections::HashSet<_> = quads
                    .iter()
                    .filter(|q| {
                        q.predicate == owl("inverseOf") || q.predicate == rdfs::SUB_PROPERTY_OF
                    })
                    .map(|q| (q.subject.clone(), q.predicate.clone(), q.object.clone()))
                    .collect();
                assert_eq!(
                    links, expected,
                    "only stored, local links should be emitted"
                );
                sets.push(quads.into_iter().collect::<std::collections::HashSet<_>>());
            }
        }
        for set in &sets[1..] {
            assert_eq!(&sets[0], set);
        }
    }

    #[test]
    fn a_derivation_says_it_is_one_and_names_its_premises() {
        let derived = ExportDerived {
            id: id(7),
            subject_id: id(10),
            predicate_id: id(2),
            object_id: Some(id(11)),
            object_value: None,
            rule_id: Some(id(8)),
            attribute_rule_id: None,
            valid_from: None,
            valid_from_precision: None,
            valid_to: None,
            valid_to_precision: None,
            derived_at: at("2026-02-01T00:00:00Z"),
            invalidated_at: None,
            confidence: 0.8,
            rule: "transitive".into(),
            rule_name: None,
            premises: vec![id(5)],
            premises_derived: vec![id(6)],
            subject_kb: Some(kb()),
            object_kb: Some(kb()),
            predicate_kb: Some(kb()),
            rule_kb: Some(kb()),
            attribute_rule_kb: None,
            foreign_fact_premise: false,
            foreign_derived_premise: false,
            subject_merged: false,
            object_merged: false,
        };
        for format in [Format::Turtle, Format::JsonLd] {
            let quads = export(format, |sink, names, vocab| {
                emit_derived(sink, names, vocab, &derived).unwrap();
            });
            let stmt = "<urn:utopia:kb:01a06dc4-f40a-7013-b09f-1b499e2e7441:derived:07070707-0707-0707-0707-070707070707>";
            assert!(has(
                &quads,
                stmt,
                "urn:utopia:ns:derived",
                "\"true\"^^<http://www.w3.org/2001/XMLSchema#boolean>"
            ));
            assert_eq!(
                objects(&quads, stmt, "http://www.w3.org/ns/prov#used"),
                vec![
                    STMT.to_string(),
                    Names::new(kb(), None).unwrap().derived(id(6)).to_string()
                ]
            );
            assert!(
                !has(&quads, SUBJ, WORKS_FOR, OBJ),
                "推出来的边不写成平铺三元组：那会让人把引擎的结论当成文档里的话"
            );
        }
    }

    #[test]
    fn evidence_points_back_at_the_document_and_the_sentence() {
        let mut cited = fact(5);
        cited.documents = vec![id(12)];
        cited.quotes = vec!["Lin Zhao joined Acme in 2023.".into()];
        let quads = export(Format::Turtle, |sink, names, vocab| {
            emit_fact(sink, names, vocab, &cited, at("2026-06-01T00:00:00Z")).unwrap();
        });
        assert_eq!(
            objects(&quads, STMT, "http://www.w3.org/ns/prov#wasDerivedFrom"),
            vec!["<urn:utopia:kb:01a06dc4-f40a-7013-b09f-1b499e2e7441:document:0c0c0c0c-0c0c-0c0c-0c0c-0c0c0c0c0c0c>"]
        );
        assert_eq!(
            objects(&quads, STMT, "urn:utopia:ns:quote"),
            vec!["\"Lin Zhao joined Acme in 2023.\""]
        );
    }

    #[test]
    fn json_ld_carries_the_same_triples() {
        let emit = |sink: &mut Sink, names: &Names, vocab: &Vocabulary| {
            emit_fact(sink, names, vocab, &fact(5), at("2026-06-01T00:00:00Z")).unwrap();
        };
        let ttl = export(Format::Turtle, emit);
        let jsonld = export(Format::JsonLd, emit);
        assert_eq!(
            ttl.len(),
            jsonld.len(),
            "两种格式是同一张图的两种写法，三元组数目必须一样"
        );
        assert!(has(&jsonld, SUBJ, WORKS_FOR, OBJ));
    }

    #[test]
    fn a_published_deployment_can_mint_http_iris() {
        let names = Names::new(kb(), Some("https://acme.example/utopia/")).unwrap();
        assert_eq!(
            names.entity(id(10)).as_str(),
            "https://acme.example/utopia/kb/01a06dc4-f40a-7013-b09f-1b499e2e7441/entity/0a0a0a0a-0a0a-0a0a-0a0a-0a0a0a0a0a0a"
        );
        // 非 http 的 base 拒掉：拼出来的会是一份谁也解析不了的文件
        assert!(Names::new(kb(), Some("javascript:alert(1)")).is_err());
        assert!(Names::new(kb(), Some("https://acme.example/a b")).is_err());
    }
}
