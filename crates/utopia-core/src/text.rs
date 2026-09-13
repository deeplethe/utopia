//! 进 `TEXT` 列之前的文本整理。
//!
//! Postgres 的 `TEXT` 不收 0x00：一插就报 `invalid byte sequence for encoding "UTF8": 0x00`，
//! 事务回滚，坏的只是几个字节，丢的是整篇文档或整条记忆。正文进 `chunks.text` 有两条路，
//! 两条都要剥：解析出来的文档正文（#611 / #630），和 `remember` 工具写下的记忆
//! （#665——JSON 里 U+0000 的转义是合法的，`serde_json` 解回来就是 NUL）。

use std::borrow::Cow;

/// 把 NUL（`\0`）剥掉。
///
/// 绝大多数文本一个 NUL 都没有，那时原样借用，不为每篇正文复制一整份。
pub fn without_nul(text: &str) -> Cow<'_, str> {
    if text.contains('\0') {
        Cow::Owned(text.replace('\0', ""))
    } else {
        Cow::Borrowed(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn borrows_when_there_is_nothing_to_strip() {
        assert!(matches!(without_nul("plain text"), Cow::Borrowed(_)));
        assert!(matches!(without_nul("中文正文"), Cow::Borrowed(_)));
    }

    #[test]
    fn strips_every_nul_and_keeps_everything_else() {
        assert_eq!(without_nul("a\0b\0\0c"), "abc");
        assert_eq!(without_nul("\0"), "");
        assert_eq!(without_nul("营收\0增长"), "营收增长");
    }
}
