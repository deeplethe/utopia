//! 集成测试连库的入口（#248）。
//!
//! 每个连库测试都以同一句开头：没有 `UTOPIA_DATABASE_URL` 就跳过而不是失败，
//! 本地随手 `cargo test` 不必先起库。可 CI 上也这么跳，绿色就成了假的：backend job
//! 没有库，24 个 store 集成测试全部静默返回，而有库的 migrations job 只跑了一个。
//!
//! 所以跳过要分场合：设了 `UTOPIA_TEST_REQUIRE_DB` 的地方（CI 的连库 job），
//! 没有库就是失败——「本该跑的没跑」得看得见。

/// 连库测试用的数据库地址。`None` = 这次跳过。
///
/// 设了 `UTOPIA_TEST_REQUIRE_DB` 而没有地址时 panic：这是给 CI 的——那里跳过
/// 等于测试根本没执行，不能显示绿色
pub fn url() -> Option<String> {
    match std::env::var("UTOPIA_DATABASE_URL") {
        Ok(u) if !u.trim().is_empty() => Some(u),
        _ => {
            if std::env::var_os("UTOPIA_TEST_REQUIRE_DB").is_some() {
                panic!(
                    "UTOPIA_TEST_REQUIRE_DB is set but UTOPIA_DATABASE_URL is not:                      this run must not skip database-backed tests"
                );
            }
            eprintln!(
                "跳过：未设 UTOPIA_DATABASE_URL（设 UTOPIA_TEST_REQUIRE_DB=1 让跳过变成失败）"
            );
            None
        }
    }
}
