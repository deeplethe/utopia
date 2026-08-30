//! 预制本体包：建库时可选的起点。
//!
//! 十个种子关系一个类型签名都没有（`graph.rs` 的 `DEFAULT_RELATION_TYPES`），
//! 而抽取提示词是支持签名的——`- buys_from (employee|team → *)`。没有签名，方向
//! 就只能靠散文描述，而散文约束不了方向。schema.org 的 1521 个属性里 1488 个带
//! domain + range，方向是**声明的**不是描述的。见 `docs/decisions/0008`。
//!
//! **文件内嵌进二进制**，不在运行时下载：README 承诺整套系统可以跑在完全离线的
//! 内网环境，运行时抓取会让这句话失效。原文按 gzip 存放（1.7 MB → 316 KB），
//! 解压在 [`bytes`]。

use utopia_core::{AppError, AppResult};

/// 一个可选的起点本体。
///
/// `classes` / `properties` 是**抓取当天数过的展示数字**，给建库界面用；
/// 真正建了多少以导入返回的 plan 为准——投影只覆盖当下能消费的构造。
pub struct Pack {
    pub id: &'static str,
    pub name: &'static str,
    pub summary: &'static str,
    /// 传给 `owl_import` 的文件名。**格式靠扩展名判定**（`RdfFormat::detect`），
    /// 所以这里必须保留真实后缀。
    pub filename: &'static str,
    pub classes: u32,
    pub properties: u32,
    /// 与已选包重叠时给用户的提示；`None` = 与其他包基本不重叠
    pub overlaps: Option<&'static str>,
    gz: &'static [u8],
}

pub const PACKS: &[Pack] = &[
    Pack {
        id: "schema-org",
        name: "schema.org",
        summary: "通用词表：人、组织、产品、事件、创作。97.8% 的属性带类型签名。",
        filename: "schema-org.ttl",
        classes: 1010,
        properties: 1676,
        overlaps: None,
        gz: include_bytes!("../packs/schema-org.ttl.gz"),
    },
    Pack {
        id: "w3c-org",
        name: "W3C Org",
        summary: "组织架构：部门、职位、任期、汇报关系。补 schema.org 最弱的一块。",
        filename: "w3c-org.ttl",
        classes: 13,
        properties: 34,
        overlaps: Some("与 schema.org 撞名 6 处，已预先对齐"),
        gz: include_bytes!("../packs/w3c-org.ttl.gz"),
    },
    Pack {
        id: "prov-o",
        name: "PROV-O",
        summary: "溯源：某个结论由谁、在何时、依据什么产出。W3C 标准词汇。",
        filename: "prov-o.ttl",
        classes: 49,
        properties: 69,
        overlaps: Some("与 schema.org 撞名 4 处，已预先对齐"),
        gz: include_bytes!("../packs/prov-o.ttl.gz"),
    },
    Pack {
        id: "foaf",
        name: "FOAF",
        summary: "人与社交关系。核心概念 schema.org 已覆盖，按需选用。",
        filename: "foaf.rdf",
        classes: 12,
        properties: 62,
        overlaps: Some("与 schema.org 重叠 21%，是候选里最高的"),
        gz: include_bytes!("../packs/foaf.rdf.gz"),
    },
    Pack {
        id: "iof-core",
        name: "IOF Core",
        summary: "工业制造：Industry Ontology Foundry 的核心层。",
        filename: "iof-core.rdf",
        classes: 294,
        properties: 75,
        overlaps: None,
        gz: include_bytes!("../packs/iof-core.rdf.gz"),
    },
];

pub fn get(id: &str) -> Option<&'static Pack> {
    PACKS.iter().find(|p| p.id == id)
}

/// 解压出原文。**每次调用都解一遍**——建库是低频动作，不值得为它常驻 1.7 MB。
pub fn bytes(pack: &Pack) -> AppResult<Vec<u8>> {
    use std::io::Read;
    let mut out = Vec::new();
    flate2::read::GzDecoder::new(pack.gz)
        .read_to_end(&mut out)
        .map_err(|e| AppError::Other(anyhow::anyhow!("本体包 {} 解压失败：{e}", pack.id)))?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 每个包都能解压，且解出来的不是空文件。
    /// `include_bytes!` 保证文件存在，但保证不了它是有效的 gzip。
    #[test]
    fn every_pack_decompresses() {
        for p in PACKS {
            let b = bytes(p).unwrap_or_else(|e| panic!("{}: {e}", p.id));
            assert!(b.len() > 10_000, "{} 解出来只有 {} 字节", p.id, b.len());
        }
    }

    /// 文件名后缀决定格式判定，写错了整个包会被当成另一种语法送进解析器。
    #[test]
    fn filenames_carry_a_format_suffix() {
        for p in PACKS {
            assert!(
                p.filename.ends_with(".ttl") || p.filename.ends_with(".rdf"),
                "{} 的文件名没有可判定的后缀：{}",
                p.id,
                p.filename
            );
        }
    }

    #[test]
    fn ids_are_unique() {
        let mut ids: Vec<_> = PACKS.iter().map(|p| p.id).collect();
        ids.sort_unstable();
        let n = ids.len();
        ids.dedup();
        assert_eq!(ids.len(), n, "包 id 有重复");
    }
}
