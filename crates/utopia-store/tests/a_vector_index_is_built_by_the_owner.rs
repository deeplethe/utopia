//! 建索引要表的所有权（#1120）：运行身份是受限角色时，它自己建不出来，
//! 得用迁移那个身份的连接去建。
//!
//! 连库才测得到：建一个只有读写授权、不拥有任何表的登录角色，跟部署里的 `utopia_app`
//! 一个处境。没有 `UTOPIA_DATABASE_URL` 时跳过；角色和索引自建自拆。
use utopia_store::vector_index::{self, Target};

const ROLE: &str = "utopia_index_probe";
const PASSWORD: &str = "index-probe-only";
// 别的测试不用的维度，索引名不会撞
const DIMS: usize = 19;

/// 把连接串里的身份换成受限角色：`postgres://owner:pw@host/db` → `postgres://probe:pw@host/db`
fn as_restricted(url: &str) -> String {
    let (scheme, rest) = url.split_once("://").expect("a postgres URL");
    let host_and_path = rest.rsplit_once('@').map_or(rest, |(_, h)| h);
    format!("{scheme}://{ROLE}:{PASSWORD}@{host_and_path}")
}

#[tokio::test]
async fn a_restricted_role_cannot_build_and_the_owner_can() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let owner = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await?;
    utopia_store::db::migrate(&owner).await?;
    sqlx::raw_sql(&format!(
        "DO $$ BEGIN
           IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = '{ROLE}') THEN
             CREATE ROLE {ROLE} LOGIN PASSWORD '{PASSWORD}';
           END IF;
         END $$;
         GRANT USAGE ON SCHEMA public TO {ROLE};
         GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA public TO {ROLE};"
    ))
    .execute(&owner)
    .await?;
    vector_index::drop(&owner, Target::NameVectors, DIMS).await?;

    let app = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&as_restricted(&url))
        .await?;
    let run = async {
        // 身份没分开时的老路：受限角色自己建，库拒绝
        let refused = vector_index::build_as_owner(&app, None, Target::NameVectors, DIMS)
            .await
            .expect_err("a role that does not own the table cannot index it");
        assert!(
            refused.to_string().contains("must be owner"),
            "the refusal is about ownership: {refused}"
        );
        assert_eq!(
            vector_index::status(&owner, Target::NameVectors, DIMS).await?,
            None,
            "nothing was built"
        );

        // 给了 owner 的连接串：同一个任务建得出来，应用的池子仍是受限角色
        let built =
            vector_index::build_as_owner(&app, Some(&url), Target::NameVectors, DIMS).await?;
        assert!(built.created);
        assert_eq!(
            vector_index::status(&app, Target::NameVectors, DIMS).await?,
            Some(true),
            "the restricted role sees a valid index it can use"
        );
        Ok::<_, anyhow::Error>(())
    }
    .await;
    app.close().await;
    vector_index::drop(&owner, Target::NameVectors, DIMS).await?;
    sqlx::raw_sql(&format!("DROP OWNED BY {ROLE}; DROP ROLE {ROLE};"))
        .execute(&owner)
        .await?;
    owner.close().await;
    run
}
