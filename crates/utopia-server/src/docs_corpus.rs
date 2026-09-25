//! Charter 语料：与前端 Docs 页同一批 markdown（单一事实来源，随二进制升级）。
//! 新增文章 = 前端 Docs.tsx 清单加一行 + 这里 ARTICLES 加一行。

use utopia_search::{DocsIndex, DocsSection};

/// (slug, 标题, 正文)。slug 必须与前端 DOCS 清单一致（引用链接 /docs/{slug} 才对得上）。
const ARTICLES: &[(&str, &str, &str)] = &[
    (
        "ingest",
        "Ingest interfaces",
        include_str!("../../../web/src/docs/ingest.md"),
    ),
    (
        "mcp",
        "Agents over MCP",
        include_str!("../../../web/src/docs/mcp.md"),
    ),
];

/// 启动时建索引；语料是编译期常量，失败即程序错误，响亮地死。
pub fn build_index() -> DocsIndex {
    DocsIndex::build(&sections()).expect("Charter 文档索引构建失败")
}

/// 按 h2 切节：一节一条索引记录，命中返回具体小节而不是整篇。
/// h2 之前的引言归入"标题节"（anchor 为空，链接落到文章顶部）。
fn sections() -> Vec<DocsSection> {
    let mut out = Vec::new();
    for (slug, title, body) in ARTICLES {
        let mut heading = (*title).to_string();
        let mut anchor = String::new();
        let mut buf: Vec<&str> = Vec::new();
        let mut flush = |heading: &str, anchor: &str, buf: &mut Vec<&str>| {
            let text = buf.join("\n").trim().to_string();
            if !text.is_empty() {
                out.push(DocsSection {
                    slug: (*slug).to_string(),
                    title: (*title).to_string(),
                    heading: heading.to_string(),
                    anchor: anchor.to_string(),
                    body: text,
                });
            }
            buf.clear();
        };
        for line in body.lines() {
            if let Some(h) = line.strip_prefix("## ") {
                flush(&heading, &anchor, &mut buf);
                // 与前端 tocOf 同法清洗：去掉行内 code/强调符号
                heading = h.replace(['`', '*'], "").trim().to_string();
                anchor = slugify(&heading);
            } else if !line.starts_with("# ") {
                buf.push(line);
            }
        }
        flush(&heading, &anchor, &mut buf);
    }
    out
}

/// 与前端 Docs.tsx 的 slugify 逐字对齐（锚点跳转依赖两边一致）：
/// 小写后，[a-z0-9一-龥] 之外的连续串折成单个 '-'，再去掉首尾 '-'。
fn slugify(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut dash = false;
    for c in s.to_lowercase().chars() {
        let keep =
            c.is_ascii_lowercase() || c.is_ascii_digit() || ('\u{4e00}'..='\u{9fa5}').contains(&c);
        if keep {
            if dash && !out.is_empty() {
                out.push('-');
            }
            dash = false;
            out.push(c);
        } else {
            dash = true;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugify_matches_frontend() {
        assert_eq!(slugify("Choosing between them"), "choosing-between-them");
        assert_eq!(
            slugify("Custom source — the pull interface"),
            "custom-source-the-pull-interface"
        );
        assert_eq!(
            slugify("API source — the push interface"),
            "api-source-the-push-interface"
        );
        assert_eq!(slugify("共享语义 Shared"), "共享语义-shared");
    }

    #[test]
    fn ingest_splits_into_sections() {
        let secs = sections();
        assert!(secs.len() >= 4, "ingest.md 应切出引言 + 3 个以上小节");
        assert!(secs.iter().any(|s| s.anchor == "shared-semantics"));
        // 引言节：anchor 空，heading 用文章标题
        assert!(secs
            .iter()
            .any(|s| s.anchor.is_empty() && s.heading == "Ingest interfaces"));
    }

    /// 每一份语料文件都得进 `ARTICLES`，否则 chat 的 search_docs 工具找不到它——
    /// 这一行是把 `web/src/docs/*.md` 当事实来源核对一遍，防「前端能看、chat 搜不到」
    /// 的漂移（之前 `mcp.md` 就落过这一摔）。
    ///
    /// **不要**写「ARTICLES 里有几个就检查几个」：那样加文件时反而不报错；这里的
    /// 不变式是「文件 → 索引」单向覆盖，文件多出来就算 bug。
    #[test]
    fn every_corpus_md_file_is_indexed() {
        let docs_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../web/src/docs");
        let mut on_disk: Vec<String> = std::fs::read_dir(&docs_dir)
            .unwrap_or_else(|e| panic!("read {}: {e}", docs_dir.display()))
            .filter_map(|entry| {
                let entry = entry.ok()?;
                let name = entry.file_name().to_string_lossy().to_string();
                if name.ends_with(".md") {
                    Some(name)
                } else {
                    None
                }
            })
            .collect();
        on_disk.sort();

        let indexed: Vec<String> = ARTICLES
            .iter()
            .map(|(slug, _, _)| format!("{slug}.md"))
            .collect();
        // indexed 也得排序好让两条 assert 一对一
        let mut indexed_sorted = indexed.clone();
        indexed_sorted.sort();

        assert_eq!(
            on_disk, indexed_sorted,
            "web/src/docs/*.md 列表与 ARTICLES 不一致：\n  on disk: {on_disk:?}\n  indexed:  {indexed:?}\n\
             新增一份语料要在两边都登记；删一份同理。drift = chat 搜不到。"
        );
    }
}
