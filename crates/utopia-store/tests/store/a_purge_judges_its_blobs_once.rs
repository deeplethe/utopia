//! #516：purge 一句话判完所有指纹，答案与逐个问一模一样。
//!
//! 原文按内容寻址、跨库共用，一份指纹只有在**没有任何地方**再引用时才交给调用方去删文件。
//! 判错的两个方向不对称：多留是漏磁盘，多交是把别人还在用的原文删掉。从前每个指纹
//! 单独问一次库，问多少次锁就多握多久；现在一句 unnest 问完，这里守的是答案一个不移：
//!
//! 1. 另一篇文档还拿着的指纹不交。
//! 2. 另一个库的文档拿着也不交——那一问故意不带 kb_id。
//! 3. 只有旧版本拿过、如今谁都不拿的指纹交出去：判断发生在版本行删掉之后。
//! 4. 已 purge 的文档不算引用：它的行还在，指纹已经不归它了。
//! 5. 同一个指纹在多个版本里出现，名单上只有一次。
//! 6. 一句话的答案与逐个问的答案相等，这是整个改法的正确性论证。
//!
//! 没有 `UTOPIA_DATABASE_URL` 时跳过而不是失败。自建自拆，绝不碰已有的库。

use sqlx::PgPool;
use utopia_store::documents;
use uuid::Uuid;

struct Fx {
    pool: PgPool,
    org: Uuid,
    ws: Uuid,
    kb: Uuid,
    tag: String,
}

async fn seed() -> anyhow::Result<Option<Fx>> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(None);
    };
    let pool = PgPool::connect(&url).await?;
    let (org, ws) = (Uuid::now_v7(), Uuid::now_v7());
    sqlx::query("INSERT INTO organizations (id, name) VALUES ($1, 'purge-blobs-test')")
        .bind(org)
        .execute(&pool)
        .await?;
    sqlx::query("INSERT INTO workspaces (id, org_id, name) VALUES ($1, $2, 'purge-blobs-test')")
        .bind(ws)
        .bind(org)
        .execute(&pool)
        .await?;
    let kb = base(&pool, ws).await?;
    // 指纹带本次运行的随机后缀，别和库里残留的撞上
    let tag = Uuid::now_v7().simple().to_string();
    Ok(Some(Fx {
        pool,
        org,
        ws,
        kb,
        tag,
    }))
}

async fn base(pool: &PgPool, ws: Uuid) -> anyhow::Result<Uuid> {
    let kb = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO knowledge_bases (id, workspace_id, name) VALUES ($1, $2, 'purge-blobs-test')",
    )
    .bind(kb)
    .bind(ws)
    .execute(pool)
    .await?;
    Ok(kb)
}

impl Fx {
    fn sha(&self, name: &str) -> String {
        format!("sha-{name}-{}", self.tag)
    }

    async fn document(&self, kb: Uuid, name: &str, sha: &str) -> anyhow::Result<Uuid> {
        let id = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO documents (id, kb_id, filename, sha256, status)
             VALUES ($1, $2, $3, $4, 'ready')",
        )
        .bind(id)
        .bind(kb)
        .bind(name)
        .bind(sha)
        .execute(&self.pool)
        .await?;
        Ok(id)
    }

    async fn version(&self, doc: Uuid, version: i32, sha: &str) -> anyhow::Result<()> {
        sqlx::query(
            "INSERT INTO document_versions (id, document_id, version, sha256) VALUES ($1, $2, $3, $4)",
        )
        .bind(Uuid::now_v7())
        .bind(doc)
        .bind(version)
        .bind(sha)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn purge(&self, kb: Uuid, doc: Uuid) -> anyhow::Result<Vec<String>> {
        documents::delete(&self.pool, kb, doc, None).await?;
        Ok(documents::purge(&self.pool, kb, doc).await?.blobs)
    }

    /// 从前的判法，逐个指纹问一次；第 6 条拿它当真值
    async fn judged_one_by_one(&self, shas: &[String]) -> anyhow::Result<Vec<String>> {
        let mut out = Vec::new();
        for s in shas {
            let (referenced,): (bool,) = sqlx::query_as(
                "SELECT EXISTS (SELECT 1 FROM documents WHERE sha256 = $1 AND purged_at IS NULL)
                     OR EXISTS (SELECT 1 FROM document_versions WHERE sha256 = $1)",
            )
            .bind(s)
            .fetch_one(&self.pool)
            .await?;
            if !referenced {
                out.push(s.clone());
            }
        }
        Ok(out)
    }

    async fn cleanup(self) -> anyhow::Result<()> {
        sqlx::query("DELETE FROM organizations WHERE id = $1")
            .bind(self.org)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}

#[tokio::test]
async fn a_blob_another_document_still_holds_is_never_collected() -> anyhow::Result<()> {
    let Some(f) = seed().await? else {
        return Ok(());
    };
    // 同一个库里两篇活文档不能同 sha（documents_kb_sha_idx），所以同库的另一个
    // 持有者只能是别人的历史版本
    let shared = f.sha("shared");
    let a = f.document(f.kb, "a.md", &shared).await?;
    let b = f.document(f.kb, "b.md", &f.sha("b-now")).await?;
    f.version(b, 1, &shared).await?;
    assert!(
        f.purge(f.kb, a).await?.is_empty(),
        "b still holds the blob in a version"
    );
    f.cleanup().await
}

#[tokio::test]
async fn a_blob_shared_across_knowledge_bases_is_never_collected() -> anyhow::Result<()> {
    let Some(f) = seed().await? else {
        return Ok(());
    };
    let shared = f.sha("shared");
    let other_kb = base(&f.pool, f.ws).await?;
    let a = f.document(f.kb, "a.md", &shared).await?;
    let _elsewhere = f.document(other_kb, "e.md", &shared).await?;
    assert!(
        f.purge(f.kb, a).await?.is_empty(),
        "the holder in another base counts; the check carries no kb_id on purpose"
    );
    f.cleanup().await
}

#[tokio::test]
async fn a_blob_only_an_old_version_held_is_collected() -> anyhow::Result<()> {
    let Some(f) = seed().await? else {
        return Ok(());
    };
    let (current, old) = (f.sha("current"), f.sha("old"));
    let a = f.document(f.kb, "a.md", &current).await?;
    f.version(a, 1, &old).await?;
    let mut blobs = f.purge(f.kb, a).await?;
    blobs.sort();
    let mut expected = vec![current, old];
    expected.sort();
    assert_eq!(
        blobs, expected,
        "the old version's blob is judged after the version row is gone"
    );
    f.cleanup().await
}

#[tokio::test]
async fn a_purged_document_no_longer_holds_its_blob() -> anyhow::Result<()> {
    let Some(f) = seed().await? else {
        return Ok(());
    };
    // 两篇活文档同 sha 只能在两个库里（documents_kb_sha_idx）
    let shared = f.sha("shared");
    let other_kb = base(&f.pool, f.ws).await?;
    let x = f.document(f.kb, "x.md", &shared).await?;
    let y = f.document(other_kb, "y.md", &shared).await?;
    assert!(f.purge(f.kb, x).await?.is_empty(), "y still holds it");
    assert_eq!(
        f.purge(other_kb, y).await?,
        vec![shared],
        "x's row is still there but purged, so it is not a holder"
    );
    f.cleanup().await
}

#[tokio::test]
async fn every_sha_is_judged_exactly_once() -> anyhow::Result<()> {
    let Some(f) = seed().await? else {
        return Ok(());
    };
    let repeated = f.sha("repeated");
    let a = f.document(f.kb, "a.md", &repeated).await?;
    f.version(a, 1, &repeated).await?;
    f.version(a, 2, &repeated).await?;
    assert_eq!(
        f.purge(f.kb, a).await?,
        vec![repeated],
        "one blob, one line, however many versions carried it"
    );
    f.cleanup().await
}

#[tokio::test]
async fn the_batched_answer_equals_the_one_by_one_answer() -> anyhow::Result<()> {
    let Some(f) = seed().await? else {
        return Ok(());
    };
    let other_kb = base(&f.pool, f.ws).await?;
    // 甲拿着五个指纹：现行一个、历史四个；其中两个别处还有人拿，一个已被 purge 的文档拿过
    let (own, shared_doc, shared_far, held_by_purged, old_only) = (
        f.sha("own"),
        f.sha("shared-doc"),
        f.sha("shared-far"),
        f.sha("held-by-purged"),
        f.sha("old-only"),
    );
    let a = f.document(f.kb, "a.md", &own).await?;
    f.version(a, 1, &shared_doc).await?;
    f.version(a, 2, &shared_far).await?;
    f.version(a, 3, &held_by_purged).await?;
    f.version(a, 4, &old_only).await?;
    let _b = f.document(f.kb, "b.md", &shared_doc).await?;
    let _far = f.document(other_kb, "far.md", &shared_far).await?;
    let p = f.document(f.kb, "p.md", &held_by_purged).await?;
    assert!(
        f.purge(f.kb, p).await?.is_empty(),
        "a still holds it in a version"
    );

    let shas: Vec<String> = sqlx::query_scalar(
        "SELECT DISTINCT sha256 FROM document_versions WHERE document_id = $1
         UNION SELECT sha256 FROM documents WHERE id = $1",
    )
    .bind(a)
    .fetch_all(&f.pool)
    .await?;
    let mut batched = f.purge(f.kb, a).await?;
    // 提交之后的库状态与事务内判断时相同：版本行已删、文档已标 purged
    let mut one_by_one = f.judged_one_by_one(&shas).await?;
    batched.sort();
    one_by_one.sort();
    assert_eq!(batched, one_by_one);
    let mut expected = vec![own, held_by_purged, old_only];
    expected.sort();
    assert_eq!(batched, expected);
    f.cleanup().await
}
