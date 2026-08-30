//! 包与包之间的同名词处置。
//!
//! 导入撞 key 时，`owl_import` 的默认判断是 [`Disposition::KeyTaken`]——跳过并报告，
//! 不自动加后缀。那条理由（"重导入认不出自己上次建的是哪个"）针对的是**猜**出来的
//! 后缀；本模块给的是**声明**的处置，重导入结果一样，所以不受那条约束。
//!
//! 为什么需要它：预制包两两撞名约 20 处，而且分两类——
//!
//! - `org:Organization` 与 `schema:Organization` 是**同一个东西**，跳过是对的，
//!   但不该报成"冲突"让用户去裁一件没得裁的事
//! - `org:role`（组织里的职位）与 `schema:role`（演员饰演的角色）**只是同名**，
//!   跳过等于丢掉 W3C Org 存在的理由
//!
//! 只覆盖预制包。用户手动导入的词汇表不在这里——那是明确的意图，撞名该报给他看。

/// 同名词的处置。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Alignment {
    /// 同义：已有的那个就是它，不必再建。跳过，但记作"已对齐"而非"冲突"
    SameAs,
    /// 同名不同义：改用这个 key 建出来
    Rename(&'static str),
}

/// (进来的 IRI, 已占位的 IRI) → 处置。
///
/// 上游自己声明过的对齐优先抄：W3C Org 的文档声明了与 FOAF 的对应，
/// PROV-O 声明了与 FOAF、Dublin Core 的对应。这里只记我们的包之间实际会撞的那些。
const TABLE: &[(&str, &str, Alignment)] = &[
    // ── W3C Org × schema.org ──────────────────────────────────────────
    (
        "http://www.w3.org/ns/org#Organization",
        "https://schema.org/Organization",
        Alignment::SameAs,
    ),
    (
        "http://www.w3.org/ns/org#identifier",
        "https://schema.org/identifier",
        Alignment::SameAs,
    ),
    (
        "http://www.w3.org/ns/org#location",
        "https://schema.org/location",
        Alignment::SameAs,
    ),
    // org:Role 是"职位所承载的角色"（与 Post、Membership 配套），
    // schema:role 是创作作品里的饰演关系。同名，无关
    (
        "http://www.w3.org/ns/org#Role",
        "https://schema.org/Role",
        Alignment::Rename("org_role"),
    ),
    // org:member 是带任期的成员关系（经由 Membership 具体化），
    // schema:member 是泛指的从属。粒度不同，两个都要
    (
        "http://www.w3.org/ns/org#member",
        "https://schema.org/member",
        Alignment::Rename("org_member"),
    ),
    (
        "http://www.w3.org/ns/org#memberOf",
        "https://schema.org/memberOf",
        Alignment::Rename("org_member_of"),
    ),
    // ── PROV-O × schema.org ───────────────────────────────────────────
    // prov:Agent 是 Person 与 Organization 的**超类**，不是同义词。
    // 判成 SameAs 会把"施事者"这个抽象层整个抹掉
    (
        "http://www.w3.org/ns/prov#Agent",
        "https://schema.org/agent",
        Alignment::Rename("prov_agent"),
    ),
    // ── FOAF × schema.org ─────────────────────────────────────────────
    (
        "http://xmlns.com/foaf/0.1/Person",
        "https://schema.org/Person",
        Alignment::SameAs,
    ),
    (
        "http://xmlns.com/foaf/0.1/Organization",
        "https://schema.org/Organization",
        Alignment::SameAs,
    ),
    (
        "http://xmlns.com/foaf/0.1/Project",
        "https://schema.org/Project",
        Alignment::SameAs,
    ),
    (
        "http://xmlns.com/foaf/0.1/name",
        "https://schema.org/name",
        Alignment::SameAs,
    ),
    (
        "http://xmlns.com/foaf/0.1/knows",
        "https://schema.org/knows",
        Alignment::SameAs,
    ),
    (
        "http://xmlns.com/foaf/0.1/givenName",
        "https://schema.org/givenName",
        Alignment::SameAs,
    ),
    (
        "http://xmlns.com/foaf/0.1/familyName",
        "https://schema.org/familyName",
        Alignment::SameAs,
    ),
    (
        "http://xmlns.com/foaf/0.1/gender",
        "https://schema.org/gender",
        Alignment::SameAs,
    ),
    (
        "http://xmlns.com/foaf/0.1/logo",
        "https://schema.org/logo",
        Alignment::SameAs,
    ),
    (
        "http://xmlns.com/foaf/0.1/thumbnail",
        "https://schema.org/thumbnail",
        Alignment::SameAs,
    ),
    (
        "http://xmlns.com/foaf/0.1/title",
        "https://schema.org/title",
        Alignment::SameAs,
    ),
    (
        "http://xmlns.com/foaf/0.1/member",
        "https://schema.org/member",
        Alignment::SameAs,
    ),
    // foaf:Agent 与 prov:Agent 同义（PROV-O 官方就是这么对齐的），
    // 但都不等于 schema:agent
    (
        "http://xmlns.com/foaf/0.1/Agent",
        "https://schema.org/agent",
        Alignment::Rename("foaf_agent"),
    ),
    // foaf:status 是即时通讯时代的在线状态，schema:status 是订单/动作状态
    (
        "http://xmlns.com/foaf/0.1/status",
        "https://schema.org/status",
        Alignment::Rename("foaf_status"),
    ),
];

/// 撞名时查这张表。两个 IRI 都不在预制包里就返回 `None`，走原来的 `KeyTaken`。
pub fn lookup(incoming_iri: &str, existing_iri: &str) -> Option<Alignment> {
    TABLE
        .iter()
        .find(|(a, b, _)| *a == incoming_iri && *b == existing_iri)
        .map(|(_, _, al)| *al)
}

#[cfg(test)]
mod tests {
    use super::*;

    // 命名空间前缀只在测试里拼 IRI 用；表里写的是完整 IRI，
    // 因为那是要跟投影结果逐字符比对的东西，拼接会掩盖笔误
    const SCHEMA: &str = "https://schema.org/";
    const ORG: &str = "http://www.w3.org/ns/org#";
    const PROV: &str = "http://www.w3.org/ns/prov#";
    const FOAF: &str = "http://xmlns.com/foaf/0.1/";

    #[test]
    fn same_as_and_rename_are_both_reachable() {
        assert_eq!(
            lookup(
                &format!("{ORG}Organization"),
                &format!("{SCHEMA}Organization")
            ),
            Some(Alignment::SameAs)
        );
        assert_eq!(
            lookup(&format!("{ORG}Role"), &format!("{SCHEMA}Role")),
            Some(Alignment::Rename("org_role"))
        );
    }

    /// 不在表里的组合必须落回 `KeyTaken`——**默认是报告冲突，不是猜**
    #[test]
    fn unknown_pairs_fall_through() {
        assert_eq!(
            lookup("http://example.com/a#Foo", &format!("{SCHEMA}Foo")),
            None
        );
        assert_eq!(
            lookup(&format!("{PROV}Entity"), "http://example.com/b#Entity"),
            None
        );
    }

    /// 方向敏感：表是 (进来的, 已占的)，反过来查不到。
    /// 装包顺序不同时命中的是不同的条目，不该靠对称性蒙混
    #[test]
    fn lookup_is_directional() {
        assert!(lookup(
            &format!("{SCHEMA}Organization"),
            &format!("{ORG}Organization")
        )
        .is_none());
    }

    /// Rename 出来的 key 必须过 `validate_key` 那套约束：小写字母数字下划线、不超 40
    #[test]
    fn renamed_keys_are_valid() {
        for (_, _, al) in TABLE {
            if let Alignment::Rename(k) = al {
                assert!(k.len() <= 40, "{k} 超过 40 字符");
                assert!(
                    k.chars()
                        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'),
                    "{k} 含非法字符"
                );
            }
        }
    }

    #[test]
    fn no_duplicate_pairs() {
        let mut seen: Vec<(&str, &str)> = TABLE.iter().map(|(a, b, _)| (*a, *b)).collect();
        let n = seen.len();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), n, "对齐表里有重复的 (进来, 已占) 组合");
    }

    /// FOAF 常量在测试外没被引用，这里确认它拼对了
    #[test]
    fn foaf_namespace_is_used() {
        assert!(lookup(&format!("{FOAF}Person"), &format!("{SCHEMA}Person")).is_some());
    }
}

/// 对齐表里的 IRI 必须真的出现在包里。
///
/// 表是手写的字符串，写错一个字符它就静默失效——撞名照样走 `KeyTaken`，
/// 只是再也不会命中。上游改前缀（schema.org 从 `http://` 迁到 `https://` 就发生过）
/// 也是同一种失效。所以拿真包跑一遍投影，逐条核对。
#[cfg(test)]
mod against_real_packs {
    use super::*;
    use std::collections::HashSet;
    use utopia_ingest::ontology_rdf::{project, RdfFormat};

    /// 把所有预制包投影一遍，收集出现过的 IRI
    fn all_iris() -> HashSet<String> {
        let mut out = HashSet::new();
        for p in crate::ontology_packs::PACKS {
            let bytes = crate::ontology_packs::bytes(p).expect(p.id);
            let fmt = RdfFormat::detect(p.filename, &bytes);
            let proj = project(&bytes, fmt).unwrap_or_else(|e| panic!("{} 投影失败：{e}", p.id));
            out.extend(proj.classes.iter().map(|c| c.iri.clone()));
            out.extend(proj.properties.iter().map(|p| p.iri.clone()));
        }
        out
    }

    #[test]
    fn every_alignment_iri_exists_in_some_pack() {
        let iris = all_iris();
        let mut missing = Vec::new();
        for (incoming, existing, _) in TABLE {
            if !iris.contains(*incoming) {
                missing.push(*incoming);
            }
            if !iris.contains(*existing) {
                missing.push(*existing);
            }
        }
        assert!(
            missing.is_empty(),
            "对齐表里这些 IRI 在任何预制包里都找不到，表已失效：\n  {}",
            missing.join("\n  ")
        );
    }
}
