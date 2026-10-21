//! GitHub 仓库文件来源：默认分支头的一棵文件树，一条路径一篇文档。
//! 同步在 `ingest_sources`；这里只放不联网的那一半，于是单测得动。

use serde::Deserialize;
use sha1::Digest as _;
use std::collections::HashSet;

#[derive(Debug, Clone, Deserialize)]
pub struct TreeEntry {
    pub path: String,
    pub mode: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub sha: String,
    pub size: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct TreeResponse {
    pub tree: Vec<TreeEntry>,
    #[serde(default)]
    pub truncated: bool,
}

/// 哪条路径收、收多大的文件。只收 markdown 是为了不把整个代码仓当语料倒进来。
pub struct FileFilter {
    include: Vec<glob::Pattern>,
    exclude: Vec<glob::Pattern>,
    max_file_bytes: i64,
}

impl FileFilter {
    pub fn from_config(config: &serde_json::Value) -> anyhow::Result<Self> {
        // 配置给了这一列就整列替换默认值
        let compile = |key: &str, defaults: &[&str]| -> anyhow::Result<Vec<glob::Pattern>> {
            let list: Vec<&str> = match config[key].as_array() {
                Some(a) => a.iter().filter_map(|v| v.as_str()).collect(),
                None => defaults.to_vec(),
            };
            list.into_iter()
                .map(|p| {
                    glob::Pattern::new(p)
                        .map_err(|e| anyhow::anyhow!("config.{key} has an invalid glob {p:?}: {e}"))
                })
                .collect()
        };
        Ok(Self {
            include: compile("include", &["**/*.md", "**/*.markdown", "**/*.mdx"])?,
            exclude: compile(
                "exclude",
                &["**/.*/**", "**/node_modules/**", "**/vendor/**"],
            )?,
            max_file_bytes: config["max_file_bytes"].as_i64().unwrap_or(1 << 20),
        })
    }

    fn eligible(&self, e: &TreeEntry) -> bool {
        let opts = glob::MatchOptions {
            case_sensitive: true,
            require_literal_separator: true,
            require_literal_leading_dot: false,
        };
        // 子模块是 commit、目录是 tree，符号链接指向别处——都不是文件本体
        e.kind == "blob"
            && e.mode != "120000"
            && e.size.is_some_and(|s| s <= self.max_file_bytes)
            && self.include.iter().any(|p| p.matches_with(&e.path, opts))
            && !self.exclude.iter().any(|p| p.matches_with(&e.path, opts))
    }
}

/// 按路径排序、同一 blob 只留第一条：相同文件在几个路径出现时，文档不该在路径之间横跳。
pub fn choose(entries: Vec<TreeEntry>, filter: &FileFilter) -> Vec<TreeEntry> {
    let mut entries: Vec<_> = entries.into_iter().filter(|e| filter.eligible(e)).collect();
    entries.sort_by(|a, b| a.path.cmp(&b.path));
    let mut seen = HashSet::new();
    entries.retain(|e| seen.insert(e.sha.clone()));
    entries
}

/// git 里一个 blob 的 SHA-1（先喂 `blob <字节数>\0` 再喂正文）。树里给的就是这个值，
/// 拿已存的字节算一遍就知道没变，不必再下文件。
pub fn git_blob_sha(bytes: &[u8]) -> String {
    let mut h = sha1::Sha1::new();
    h.update(format!("blob {}\0", bytes.len()).as_bytes());
    h.update(bytes);
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_globs_take_markdown_outside_hidden_and_vendored_folders() {
        let f = FileFilter::from_config(&serde_json::json!({})).unwrap();
        let entry = |path: &str, mode: &str| TreeEntry {
            path: path.into(),
            mode: mode.into(),
            kind: "blob".into(),
            sha: "s".into(),
            size: Some(10),
        };
        for path in ["README.md", "docs/a/b.mdx"] {
            assert!(f.eligible(&entry(path, "100644")), "{path}");
        }
        for path in [
            ".github/PULL_REQUEST_TEMPLATE.md",
            "web/node_modules/x/README.md",
            "vendor/a.md",
            "Cargo.toml",
        ] {
            assert!(!f.eligible(&entry(path, "100644")), "{path}");
        }
        assert!(!f.eligible(&entry("link.md", "120000")));
    }
}
