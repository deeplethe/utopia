//! GitLab 项目文件来源（gitlab.com）：默认分支头的一棵文件树，一条路径一篇文档，与
//! [`crate::github_repo`] 同一个形状（#1327）。同步在 `ingest_sources`；这里只放不联网的那一半。
//!
//! 与 GitHub 不同的三处：树是分页的（keyset，每页至多 100 条，没有 `truncated`）；树条目
//! 有 blob SHA 却没有大小，于是大小上限要等 `HEAD` 文件拿到 `X-Gitlab-Size` 才判得了；
//! 提交时间带毫秒，落库前截到秒（0013）。

use chrono::{DateTime, SubsecRound as _, Utc};
use serde::Deserialize;
use std::collections::HashSet;

use crate::github_repo::FileFilter;

/// 一棵树最多翻这么多页（每页 100 条）。翻不完就当树被截断：本轮照收列出来的文件，
/// 但不判「不在来源中」——缺席可能只是还没列到，与 GitHub 的 `truncated` 同一个处理
pub const MAX_TREE_PAGES: usize = 100;

/// 空文件的 git blob SHA 是固定的。树里没有大小，靠它在问大小之前就把空文件挡掉；不挡的话，
/// 空文件从不落库，每一轮都要为它白发一次 `HEAD`
pub const EMPTY_BLOB: &str = "e69de29bb2d1d6434b8b29ae775ad8c2e48c5391";

#[derive(Debug, Clone, Deserialize)]
pub struct TreeEntry {
    /// git blob SHA，与 GitHub 树里的 `sha` 是同一个值
    pub id: String,
    pub path: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub mode: String,
}

/// 按路径排序、同一 blob 只留第一条，与 [`crate::github_repo::choose`] 一致。大小此时还不知道，
/// 由同步在 `HEAD` 之后再判
pub fn choose(entries: Vec<TreeEntry>, filter: &FileFilter) -> Vec<TreeEntry> {
    let mut entries: Vec<_> = entries
        .into_iter()
        .filter(|e| e.id != EMPTY_BLOB && filter.path_ok(&e.path, &e.kind, &e.mode))
        .collect();
    entries.sort_by(|a, b| a.path.cmp(&b.path));
    let mut seen = HashSet::new();
    entries.retain(|e| seen.insert(e.id.clone()));
    entries
}

/// `Link` 头里 `rel="next"` 的那一页。只认与 API 同源的地址：这个地址是响应给的，
/// 照单全收就等于让对方决定我们带着令牌去访问哪里
pub fn next_page(link: Option<&str>, api_base: &reqwest::Url) -> Option<reqwest::Url> {
    link?.split(',').find_map(|part| {
        let (target, params) = part.split_once(';')?;
        if !params.split(';').any(|p| p.trim() == r#"rel="next""#) {
            return None;
        }
        let url = reqwest::Url::parse(target.trim().strip_prefix('<')?.strip_suffix('>')?).ok()?;
        (url.origin() == api_base.origin() && url.path().starts_with(api_base.path()))
            .then_some(url)
    })
}

/// `committed_date` 形如 `2026-10-05T14:30:00.000+00:00`，截到秒
pub fn commit_time(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|t| t.with_timezone(&Utc).trunc_subsecs(0))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(path: &str, kind: &str, mode: &str, id: &str) -> TreeEntry {
        TreeEntry {
            id: id.into(),
            path: path.into(),
            kind: kind.into(),
            mode: mode.into(),
        }
    }

    #[test]
    fn the_tree_keeps_markdown_files_once_and_leaves_links_and_folders_out() {
        let f = FileFilter::from_config(&serde_json::json!({})).unwrap();
        let chosen = choose(
            vec![
                entry("docs/guide.md", "blob", "100644", "9292"),
                entry("docs/copy-of-guide.md", "blob", "100644", "9292"),
                entry("docs", "tree", "040000", "5964"),
                entry("link.md", "blob", "120000", "eafc"),
                entry(".gitlab/notes.md", "blob", "100644", "3e42"),
                entry("node_modules/pkg/README.md", "blob", "100644", "08e8"),
                entry("src/main.rs", "blob", "100644", "f328"),
                entry("README.md", "blob", "100644", "9e4e"),
                entry("notes/empty.md", "blob", "100644", EMPTY_BLOB),
            ],
            &f,
        );
        let paths: Vec<_> = chosen.iter().map(|e| e.path.as_str()).collect();
        assert_eq!(paths, ["README.md", "docs/copy-of-guide.md"]);
    }

    #[test]
    fn only_a_next_page_on_the_same_api_is_followed() {
        let base = reqwest::Url::parse("https://gitlab.com/api/v4").unwrap();
        let link = r#"<https://gitlab.com/api/v4/projects/a%2Fb/repository/tree?page_token=x&pagination=keyset>; rel="next""#;
        assert_eq!(
            next_page(Some(link), &base).map(|u| u
                .query_pairs()
                .find(|(k, _)| k == "page_token")
                .unwrap()
                .1
                .into_owned()),
            Some("x".into())
        );
        assert!(next_page(
            Some(r#"<https://evil.example/api/v4/x>; rel="next""#),
            &base
        )
        .is_none());
        assert!(next_page(Some(r#"<https://gitlab.com/other>; rel="next""#), &base).is_none());
        assert!(next_page(Some(r#"<https://gitlab.com/api/v4/x>; rel="first""#), &base).is_none());
        assert!(next_page(None, &base).is_none());
    }

    #[test]
    fn a_commit_time_is_cut_to_the_second() {
        let t = commit_time("2026-10-05T14:30:00.987+02:00").unwrap();
        assert_eq!(t.to_rfc3339(), "2026-10-05T12:30:00+00:00");
        assert!(commit_time("yesterday").is_none());
    }
}
