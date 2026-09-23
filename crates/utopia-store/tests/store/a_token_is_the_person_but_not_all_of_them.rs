//! 个人访问令牌（0014 / 迁移 0017）。
//!
//! 这一枚令牌是给 MCP 客户端用的：长命、配在别人机器上的一个文件里、以发它的
//! 人的身份行事。所以它必须撤得掉、过得了期、而且**范围只能比这个人更小**。
//!
//! 钉五件事：
//!
//! - **明文只出现一次**。库里只有哈希，拿到整张表也复原不出那串字符
//! - **撤销立刻生效**。而且是打戳不删行——「这把钥匙存在过」要查得到
//! - **过期立刻失效**，判断在 SQL 里做，不在取回来之后做
//! - **`kb_ids` 只收窄**。限定到一个库的令牌，够不着另一个
//! - **`last_used_at` 会被写上**。撤之前人要答得出「这把还在用吗」

use chrono::{Duration, Utc};
use sqlx::PgPool;
use utopia_store::tokens;
use uuid::Uuid;

struct Fixture {
    org: Uuid,
    user: Uuid,
    kb_a: Uuid,
    kb_b: Uuid,
}

async fn seed(pool: &PgPool) -> anyhow::Result<Fixture> {
    let org = Uuid::now_v7();
    let ws = Uuid::now_v7();
    let user = Uuid::now_v7();
    let (kb_a, kb_b) = (Uuid::now_v7(), Uuid::now_v7());

    sqlx::query("INSERT INTO organizations (id, name) VALUES ($1, 'token-test')")
        .bind(org)
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO workspaces (id, org_id, name) VALUES ($1, $2, 'token-test')")
        .bind(ws)
        .bind(org)
        .execute(pool)
        .await?;
    for (kb, name) in [(kb_a, "kb-a"), (kb_b, "kb-b")] {
        sqlx::query("INSERT INTO knowledge_bases (id, workspace_id, name) VALUES ($1, $2, $3)")
            .bind(kb)
            .bind(ws)
            .bind(name)
            .execute(pool)
            .await?;
    }
    sqlx::query(
        "INSERT INTO users (id, org_id, email, password_hash, display_name)
         VALUES ($1, $2, $3, 'x', 'Token Test')",
    )
    .bind(user)
    .bind(org)
    .bind(format!("{user}@token.test"))
    .execute(pool)
    .await?;

    Ok(Fixture {
        org,
        user,
        kb_a,
        kb_b,
    })
}

#[tokio::test]
async fn a_token_is_the_person_but_not_all_of_them() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let f = seed(&pool).await?;

    let run = async {
        // ---- 一、发一枚，明文只这一次拿得到
        let (view, plain) = tokens::issue(&pool, f.user, "我的笔记本", "read", None, None).await?;
        assert!(plain.starts_with("utp_pat_"), "前缀要认得出是哪一种令牌");
        assert_eq!(view.scope, "read", "**缺省只读**：要写得显式勾");
        assert!(view.revoked_at.is_none());

        let stored: String =
            sqlx::query_scalar("SELECT token_hash FROM personal_tokens WHERE id = $1")
                .bind(view.id)
                .fetch_one(&pool)
                .await?;
        assert_ne!(stored, plain, "库里存的必须是哈希");
        assert!(
            !stored.contains(&plain[8..24]),
            "**明文的任何一段都不该出现在库里**——拿到整张表也复原不出来"
        );

        // ---- 二、认得回来，而且顺手写了 last_used_at
        let auth = tokens::authenticate(&pool, &plain).await?;
        assert_eq!(auth.user_id, f.user, "令牌以发它的人的身份行事");
        assert!(!auth.can_write(), "read 的令牌不能写");
        let used: Option<chrono::DateTime<Utc>> =
            sqlx::query_scalar("SELECT last_used_at FROM personal_tokens WHERE id = $1")
                .bind(view.id)
                .fetch_one(&pool)
                .await?;
        assert!(used.is_some(), "**撤之前人要答得出「这把还在用吗」**");

        // ---- 三、伪造的、前缀不对的，一律不认
        assert!(
            tokens::authenticate(&pool, "utp_pat_deadbeef")
                .await
                .is_err(),
            "编一串出来不该认"
        );
        assert!(
            tokens::authenticate(&pool, &plain.replace("utp_pat_", "utp_"))
                .await
                .is_err(),
            "摄入令牌的前缀不该走这条路"
        );

        // ---- 四、kb_ids 只收窄
        let (_, scoped) =
            tokens::issue(&pool, f.user, "只给 A 库", "write", Some(&[f.kb_a]), None).await?;
        let auth = tokens::authenticate(&pool, &scoped).await?;
        assert!(auth.covers(f.kb_a), "授权过的库够得着");
        assert!(
            !auth.covers(f.kb_b),
            "**限定到一个库的令牌够不着另一个**——哪怕这个人两个库都能进"
        );
        assert!(auth.can_write(), "write 的令牌能写");
        // 不限定的那一枚照样覆盖全部
        assert!(tokens::authenticate(&pool, &plain).await?.covers(f.kb_b));

        // ---- 五、撤销立刻生效，而且行还在
        tokens::revoke(&pool, f.user, view.id).await?;
        assert!(
            tokens::authenticate(&pool, &plain).await.is_err(),
            "**撤了就立刻不认**——判断在 SQL 里，不在取回来之后"
        );
        let still_there: i64 =
            sqlx::query_scalar("SELECT count(*) FROM personal_tokens WHERE id = $1")
                .bind(view.id)
                .fetch_one(&pool)
                .await?;
        assert_eq!(still_there, 1, "打戳不删行：这把钥匙存在过要查得到");
        assert!(
            tokens::revoke(&pool, f.user, view.id).await.is_err(),
            "撤两次的第二次该说没有这一行可撤"
        );

        // ---- 六、过期
        let (expired_view, expired) = tokens::issue(
            &pool,
            f.user,
            "早就过期的",
            "read",
            None,
            Some(Utc::now() - Duration::hours(1)),
        )
        .await?;
        assert!(
            tokens::authenticate(&pool, &expired).await.is_err(),
            "过了期就不认"
        );

        // ---- 七、列表把撤过的、过期的都列出来
        let all = tokens::list(&pool, f.user).await?;
        assert_eq!(all.len(), 3, "撤销过的也要列——撤过这件事本身要看得见");
        assert!(
            all.iter()
                .any(|t| t.id == view.id && t.revoked_at.is_some()),
            "列表要标出哪一把被撤了"
        );
        assert!(
            all.iter().all(|t| t.token_prefix.starts_with("utp_pat_")),
            "列表只给前缀，不给明文"
        );
        assert!(all.iter().any(|t| t.id == expired_view.id));

        // 别人的令牌撤不动
        let other = Uuid::now_v7();
        assert!(
            tokens::revoke(&pool, other, expired_view.id).await.is_err(),
            "**令牌是谁的谁才撤得动**"
        );
        Ok::<_, anyhow::Error>(())
    }
    .await;

    sqlx::query("DELETE FROM organizations WHERE id = $1")
        .bind(f.org)
        .execute(&pool)
        .await?;
    run
}
