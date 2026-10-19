//! 推送来的短语正好是一条声明了的关系（#1242）：本体自己判，不问模型。只认键或标签
//! 归一后相等、两端的类落在域与值域里、只有一条对得上的；人的判定不动；推送的文档抽完
//! 排一份物化。真库、推送路径（0054），没有对话模型。
use super::*;
use serde_json::{json, Value};
use std::sync::Arc;
use utopia_store::{materialize, phrase_bindings};

/// 一个库：类 statement、test、other，三个类别词都已绑上；关系 validated_by 声明在
/// statement → test；一个 `statements` 来源
struct Rig {
    pool: sqlx::PgPool,
    state: AppState,
    org: Uuid,
    ws: Uuid,
    kb: Uuid,
    source: Uuid,
    statement: Uuid,
    test: Uuid,
    other: Uuid,
    validated_by: Uuid,
    _dir: tempfile::TempDir,
}

impl Rig {
    async fn new() -> anyhow::Result<Option<Self>> {
        let Some(url) = utopia_store::test_db::url() else {
            return Ok(None);
        };
        let pool = sqlx::PgPool::connect(&url).await?;
        utopia_store::db::migrate(&pool).await?;
        let (org, ws, kb, source, statement, test, other, validated_by) = (
            Uuid::now_v7(),
            Uuid::now_v7(),
            Uuid::now_v7(),
            Uuid::now_v7(),
            Uuid::now_v7(),
            Uuid::now_v7(),
            Uuid::now_v7(),
            Uuid::now_v7(),
        );
        sqlx::raw_sql(&format!(
            "INSERT INTO organizations(id,name) VALUES ('{org}','declared-binding');
             INSERT INTO workspaces(id,org_id,name) VALUES ('{ws}','{org}','declared-binding');
             INSERT INTO knowledge_bases(id,workspace_id,name) VALUES ('{kb}','{ws}','declared-binding');
             INSERT INTO sources(id,kb_id,kind,name) VALUES ('{source}','{kb}','statements','ci');
             INSERT INTO entity_types(id,kb_id,key,label,color,shape) VALUES
                 ('{statement}','{kb}','statement','Statement','#000','circle'),
                 ('{test}','{kb}','test','Test','#000','circle'),
                 ('{other}','{kb}','other','Other','#000','circle');
             INSERT INTO type_bindings(id,kb_id,kind_word,type_id,status,decided_by) VALUES
                 ('{}','{kb}','statement','{statement}','bound','person'),
                 ('{}','{kb}','test','{test}','bound','person'),
                 ('{}','{kb}','other','{other}','bound','person');
             INSERT INTO relation_types(id,kb_id,key,label,kind,temporal,description) VALUES
                 ('{validated_by}','{kb}','validated_by','validatedBy','relation','eternal','a test that validates a statement');
             INSERT INTO relation_type_domains(relation_type_id,entity_type_id) VALUES ('{validated_by}','{statement}');
             INSERT INTO relation_type_ranges(relation_type_id,entity_type_id) VALUES ('{validated_by}','{test}');",
            Uuid::now_v7(),
            Uuid::now_v7(),
            Uuid::now_v7(),
        ))
        .execute(&pool)
        .await?;
        let dir = tempfile::tempdir()?;
        let cfg = utopia_core::config::AppConfig {
            data_dir: dir.path().to_string_lossy().into_owned(),
            ..Default::default()
        };
        let search = Arc::new(utopia_search::SearchIndex::open(
            &dir.path().join("search"),
        )?);
        let state = AppState::new(pool.clone(), &cfg, search, "test-only".into());
        Ok(Some(Self {
            pool,
            state,
            org,
            ws,
            kb,
            source,
            statement,
            test,
            other,
            validated_by,
            _dir: dir,
        }))
    }

    /// 推一篇文档并抽取：一条陈述，写成 [主语, 它的类别词, 短语, 宾语, 它的类别词]。返回文档
    async fn push(&self, external_id: &str, said: [&str; 5]) -> anyhow::Result<Uuid> {
        let [subject, subject_kind, phrase, object, object_kind] = said;
        let payload: Value = json!({
            "external_id": external_id,
            "e": [[subject, subject_kind, true], [object, object_kind, true]],
            "s": [[null, subject, phrase, object, null, {}, null, null]],
            "n": []
        });
        let (doc, chunk) = (Uuid::now_v7(), Uuid::now_v7());
        sqlx::query(
            "INSERT INTO documents(id,kb_id,filename,sha256,source_id) VALUES ($1,$2,$3,$4,$5)",
        )
        .bind(doc)
        .bind(self.kb)
        .bind(format!("{external_id}.json"))
        .bind(format!("x{doc}"))
        .bind(self.source)
        .execute(&self.pool)
        .await?;
        sqlx::query("INSERT INTO chunks(id,kb_id,document_id,seq,text) VALUES ($1,$2,$3,0,$4)")
            .bind(chunk)
            .bind(self.kb)
            .bind(doc)
            .bind(serde_json::to_string_pretty(&payload)?)
            .execute(&self.pool)
            .await?;
        crate::extraction::extract_document(
            &self.state,
            doc,
            utopia_core::models::Proposer::default(),
        )
        .await?;
        Ok(doc)
    }

    async fn bindings(&self) -> anyhow::Result<Vec<phrase_bindings::Binding>> {
        Ok(phrase_bindings::bindings(&self.pool, self.kb).await?)
    }

    /// statement → test 这条签名
    fn signature(&self, phrase: &str) -> PhraseSignature {
        PhraseSignature {
            phrase: phrase.into(),
            subject_type_id: Some(self.statement),
            subject_type_key: Some("statement".into()),
            object_type_id: Some(self.test),
            object_type_key: Some("test".into()),
            object_is_value: false,
            count: 1,
            examples: Vec::new(),
            quotes: Vec::new(),
            example_statement_ids: Vec::new(),
        }
    }

    /// 配上一个不通的对话端点：对齐照跑，真问了模型这一轮会记失败、再排
    async fn unreachable_model(&self) -> anyhow::Result<()> {
        utopia_store::settings::upsert(
            &self.pool,
            self.ws,
            Some("http://127.0.0.1:9"),
            None,
            Some("unreachable"),
            None,
            None,
            None,
            None,
        )
        .await?;
        Ok(())
    }

    async fn queued(&self, kind: &str) -> anyhow::Result<i64> {
        Ok(sqlx::query_scalar(
            "SELECT count(*) FROM jobs WHERE kind=$1 AND status='queued' AND payload->>'kb_id'=$2",
        )
        .bind(kind)
        .bind(self.kb.to_string())
        .fetch_one(&self.pool)
        .await?)
    }

    async fn clear_jobs(&self) -> anyhow::Result<()> {
        sqlx::query(
            "DELETE FROM jobs WHERE payload->>'kb_id'=$1
                OR payload->>'document_id' IN (SELECT id::text FROM documents WHERE kb_id=$2)",
        )
        .bind(self.kb.to_string())
        .bind(self.kb)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn cleanup(self) -> anyhow::Result<()> {
        self.clear_jobs().await?;
        sqlx::query("DELETE FROM organizations WHERE id=$1")
            .bind(self.org)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}

#[tokio::test]
async fn a_pushed_phrase_equal_to_a_declared_relation_binds_and_materializes_without_a_model(
) -> anyhow::Result<()> {
    let Some(rig) = Rig::new().await? else {
        return Ok(());
    };
    rig.push("obs-1", ["S-1", "statement", "validated_by", "T-1", "test"])
        .await?;
    let found: Vec<_> = rig
        .bindings()
        .await?
        .into_iter()
        .map(|b| {
            (
                b.phrase,
                b.status,
                b.relation_type_id,
                b.direction,
                b.decided_by,
            )
        })
        .collect();
    assert_eq!(
        found,
        vec![(
            "validated_by".to_string(),
            "bound".to_string(),
            Some(rig.validated_by),
            Some("forward".to_string()),
            "ontology".to_string(),
        )]
    );
    materialize::materialize(&rig.pool, rig.kb).await?;
    assert_eq!(materialize::count(&rig.pool, rig.kb).await?, 1);
    rig.cleanup().await
}

/// 像不算、两端不在域与值域里不算、两条关系叫同一个名字也不算：这些都留给对齐器。
/// 同一个库里那条恰好对得上的照样绑上，否则「什么都没绑」什么也证明不了
#[tokio::test]
async fn only_a_phrase_naming_one_relation_that_fits_both_ends_binds() -> anyhow::Result<()> {
    let Some(rig) = Rig::new().await? else {
        return Ok(());
    };
    let (checked_by, reviewed_by) = (Uuid::now_v7(), Uuid::now_v7());
    sqlx::raw_sql(&format!(
        "INSERT INTO relation_types(id,kb_id,key,label,kind,temporal,description) VALUES
             ('{checked_by}','{kb}','checked_by','checkedBy','relation','eternal',''),
             ('{reviewed_by}','{kb}','reviewed_by','checked_by','relation','eternal','');
         INSERT INTO relation_type_domains(relation_type_id,entity_type_id) VALUES
             ('{checked_by}','{s}'), ('{reviewed_by}','{s}');
         INSERT INTO relation_type_ranges(relation_type_id,entity_type_id) VALUES
             ('{checked_by}','{t}'), ('{reviewed_by}','{t}');",
        kb = rig.kb,
        s = rig.statement,
        t = rig.test,
    ))
    .execute(&rig.pool)
    .await?;
    for (n, said) in [
        ["S-1", "statement", "validated_by", "T-1", "test"],
        ["S-2", "statement", "validated by tests", "T-2", "test"],
        ["O-3", "other", "validated_by", "T-3", "test"],
        ["T-4", "test", "validated_by", "S-4", "statement"],
        ["S-5", "statement", "checked_by", "T-5", "test"],
    ]
    .into_iter()
    .enumerate()
    {
        rig.push(&format!("obs-{n}"), said).await?;
    }
    let bound: Vec<_> = rig
        .bindings()
        .await?
        .into_iter()
        .map(|b| (b.phrase, b.subject_type_id))
        .collect();
    assert_eq!(
        bound,
        vec![("validated_by".to_string(), Some(rig.statement))]
    );
    rig.cleanup().await
}

/// 签名都已判过的推送不数库里的签名（#1296）。一条连接的池上 sqlx 缓存它跑过的每条语句，
/// 没跑的不在 `pg_prepared_statements` 里
#[tokio::test]
async fn a_push_whose_signatures_are_settled_skips_the_base_signatures() -> anyhow::Result<()> {
    let Some(rig) = Rig::new().await? else {
        return Ok(());
    };
    rig.push("obs-1", ["S-1", "statement", "validated_by", "T-1", "test"])
        .await?;
    let second = rig
        .push("obs-2", ["S-2", "statement", "validated_by", "T-2", "test"])
        .await?;
    let one = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect_with((*rig.pool.connect_options()).clone())
        .await?;
    assert_eq!(bind_declared(&one, rig.kb, second).await?, 0);
    let grouped: Vec<String> = sqlx::query_scalar(
        "SELECT statement FROM pg_prepared_statements
         WHERE statement LIKE '%WITH live AS%' AND statement NOT LIKE '%pg_prepared_statements%'",
    )
    .fetch_all(&one)
    .await?;
    assert_eq!(grouped, Vec::<String>::new());
    one.close().await;
    rig.cleanup().await
}

#[tokio::test]
async fn a_person_decision_is_not_overwritten_by_the_ontology() -> anyhow::Result<()> {
    let Some(rig) = Rig::new().await? else {
        return Ok(());
    };
    phrase_bindings::decide(
        &rig.pool,
        rig.kb,
        &rig.signature("validated_by"),
        Decision {
            relation_type_id: None,
            direction: None,
            status: "none",
            votes: &json!({}),
            decided_by: "person",
            basis: None,
            marks: None,
            marks_asked: false,
        },
    )
    .await?;
    rig.push("obs-1", ["S-1", "statement", "validated_by", "T-1", "test"])
        .await?;
    let found: Vec<_> = rig
        .bindings()
        .await?
        .into_iter()
        .map(|b| (b.status, b.decided_by))
        .collect();
    assert_eq!(found, vec![("none".to_string(), "person".to_string())]);
    rig.cleanup().await
}

#[tokio::test]
async fn pushed_documents_queue_one_materialization() -> anyhow::Result<()> {
    let Some(rig) = Rig::new().await? else {
        return Ok(());
    };
    rig.push("obs-1", ["S-1", "statement", "validated_by", "T-1", "test"])
        .await?;
    rig.push("obs-2", ["S-2", "statement", "validated_by", "T-2", "test"])
        .await?;
    assert_eq!(rig.queued(phrase_bindings::MATERIALIZE_KIND).await?, 1);
    rig.cleanup().await
}

/// 配了对话模型的库照样跑对齐：本体判的签名不送模型，也不让这一轮以为它过期了再排一次。
/// 端点是个不通的地址——真问了，这一轮会记失败、再排
#[tokio::test]
async fn the_aligner_leaves_a_binding_the_ontology_decided_alone() -> anyhow::Result<()> {
    let Some(rig) = Rig::new().await? else {
        return Ok(());
    };
    rig.push("obs-1", ["S-1", "statement", "validated_by", "T-1", "test"])
        .await?;
    rig.unreachable_model().await?;
    rig.clear_jobs().await?;
    align_phrases_reasking(&rig.state, rig.kb, 0).await?;
    let found: Vec<_> = rig
        .bindings()
        .await?
        .into_iter()
        .map(|b| (b.status, b.decided_by))
        .collect();
    assert_eq!(found, vec![("bound".to_string(), "ontology".to_string())]);
    assert_eq!(rig.queued("align_phrases").await?, 0);
    rig.cleanup().await
}

/// 关系改了名，本体判的行就像代理的一样重判：签名送去问模型。端点不通——这一轮记失败、再排；
/// 本体还认的行不会走到这一步（上一条）
#[tokio::test]
async fn a_renamed_relation_sends_the_ontology_binding_back_to_the_aligner() -> anyhow::Result<()> {
    let Some(rig) = Rig::new().await? else {
        return Ok(());
    };
    rig.push("obs-1", ["S-1", "statement", "validated_by", "T-1", "test"])
        .await?;
    sqlx::query("UPDATE relation_types SET key='verified_by', label='verifiedBy' WHERE id=$1")
        .bind(rig.validated_by)
        .execute(&rig.pool)
        .await?;
    rig.unreachable_model().await?;
    rig.clear_jobs().await?;
    align_phrases_reasking(&rig.state, rig.kb, 0).await?;
    assert_eq!(rig.queued("align_phrases").await?, 1);
    rig.cleanup().await
}

/// 状态关系本体不判：绑上它还得说只带一个日期的陈述那个日期标哪一端，那是对齐器的题——
/// 它绑上、顺带问这一格，问不到的进队列等人。同一个库里那条不是状态的照样绑上
#[tokio::test]
async fn a_pushed_phrase_equal_to_a_declared_state_relation_is_left_to_the_aligner(
) -> anyhow::Result<()> {
    let Some(rig) = Rig::new().await? else {
        return Ok(());
    };
    let assigned_to = Uuid::now_v7();
    sqlx::raw_sql(&format!(
        "INSERT INTO relation_types(id,kb_id,key,label,kind,temporal,description) VALUES
             ('{assigned_to}','{kb}','assigned_to','assignedTo','relation','state','');
         INSERT INTO relation_type_domains(relation_type_id,entity_type_id) VALUES ('{assigned_to}','{s}');
         INSERT INTO relation_type_ranges(relation_type_id,entity_type_id) VALUES ('{assigned_to}','{t}');",
        kb = rig.kb,
        s = rig.statement,
        t = rig.test,
    ))
    .execute(&rig.pool)
    .await?;
    rig.push("obs-1", ["S-1", "statement", "assigned_to", "T-1", "test"])
        .await?;
    rig.push("obs-2", ["S-2", "statement", "validated_by", "T-2", "test"])
        .await?;
    let bound: Vec<_> = rig
        .bindings()
        .await?
        .into_iter()
        .map(|b| (b.phrase, b.decided_by))
        .collect();
    assert_eq!(
        bound,
        vec![("validated_by".to_string(), "ontology".to_string())]
    );
    rig.cleanup().await
}

/// 本体判的绑定，关系不再声明它（这里是域收窄到别的类）就和代理的判定一样重判：这条签名
/// 没了候选，对齐器不问模型就记成 none，物化收回那一行。端点不通——真问了，这一轮会记失败、再排
#[tokio::test]
async fn an_ontology_binding_is_redecided_once_the_relation_stops_declaring_it(
) -> anyhow::Result<()> {
    let Some(rig) = Rig::new().await? else {
        return Ok(());
    };
    rig.push("obs-1", ["S-1", "statement", "validated_by", "T-1", "test"])
        .await?;
    materialize::materialize(&rig.pool, rig.kb).await?;
    assert_eq!(materialize::count(&rig.pool, rig.kb).await?, 1);
    sqlx::query("UPDATE relation_type_domains SET entity_type_id=$1 WHERE relation_type_id=$2")
        .bind(rig.other)
        .bind(rig.validated_by)
        .execute(&rig.pool)
        .await?;
    rig.unreachable_model().await?;
    rig.clear_jobs().await?;
    align_phrases_reasking(&rig.state, rig.kb, 0).await?;
    let found: Vec<(String, String, Option<String>)> = sqlx::query_as(
        "SELECT status, decided_by, votes->>'reason' FROM phrase_bindings WHERE kb_id=$1",
    )
    .bind(rig.kb)
    .fetch_all(&rig.pool)
    .await?;
    assert_eq!(
        found,
        vec![(
            "none".to_string(),
            "agent".to_string(),
            Some("no_candidates".to_string())
        )]
    );
    assert_eq!(rig.queued("align_phrases").await?, 0);
    materialize::materialize(&rig.pool, rig.kb).await?;
    assert_eq!(materialize::count(&rig.pool, rig.kb).await?, 0);
    rig.cleanup().await
}
