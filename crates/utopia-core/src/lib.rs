//! utopia-core: 领域模型、错误类型与配置。

pub mod config;
pub mod error;
pub mod models;
pub mod secrets;
pub mod text;

pub use error::{is_deferred, is_terminal, AppError, AppResult, Deferred, Terminal};
pub use text::without_nul;

/// 审核对的 `reason` 里，召回通道留下的记号。名字向量召回（0041 第 2 刀通道 2）提的是两个
/// **不同的字符串**，和同名家族（`shared_name|`、`ambiguous_name|`、`namesake_tie|`）是两种
/// 证据强度：裁决器读它、治理闸门看它、召回写它，都从这里认，不各自拼前缀
pub mod review_reasons {
    /// `name_vector|<余弦>`：名字向量召回提的对
    pub const NAME_VECTOR: &str = "name_vector|";

    /// 这一对是名字向量召回提出来的（两个相近但不同的字符串）
    pub fn similarity_proposed(reason: Option<&str>) -> bool {
        reason.is_some_and(|r| r.starts_with(NAME_VECTOR))
    }

    /// 名字向量召回记下的余弦文本；不是这种对时为 None
    pub fn name_vector_cosine(reason: Option<&str>) -> Option<&str> {
        reason?.strip_prefix(NAME_VECTOR)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn only_the_name_vector_prefix_counts() {
            assert!(similarity_proposed(Some("name_vector|0.78")));
            assert_eq!(name_vector_cosine(Some("name_vector|0.78")), Some("0.78"));
            for r in [
                "ambiguous_name|0.41",
                "namesake_tie|0.55",
                "shared_name|张伟",
                "contains",
                "",
            ] {
                assert!(!similarity_proposed(Some(r)), "{r}");
                assert_eq!(name_vector_cosine(Some(r)), None, "{r}");
            }
            assert!(!similarity_proposed(None));
        }
    }
}
