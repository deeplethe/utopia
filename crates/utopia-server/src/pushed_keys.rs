//! 推送来源（`api`、`statements`）的文档怎么按推送方给的 identity 存 `external_key`、
//! 怎么从它取回来。推送的处理器、按 `external_id` 读回和检索都用这一份（#1271）

/// 有推送密钥的来源：`api` 推文档，`statements` 推陈述（0054）。两种的密钥同一套
/// 生成、查看、轮换
pub fn is_push_kind(kind: &str) -> bool {
    matches!(kind, "api" | "statements")
}

/// The `external_key` a push source files a pushed identity under (`api:…`, `statements:…`).
/// The push handlers write it and the read by `external_id` (#1276) looks it up, so the two
/// cannot drift apart.
pub fn pushed_key(source: &utopia_core::models::Source, identity: &str) -> String {
    format!("{}:{identity}", source.kind)
}

/// `pushed_key` 倒过来：推送来源的文档交回推送方给的 identity，别的来源没有（#1271）
pub fn pushed_identity<'k>(kind: &str, key: &'k str) -> Option<&'k str> {
    if !is_push_kind(kind) {
        return None;
    }
    key.strip_prefix(kind)?.strip_prefix(':')
}
