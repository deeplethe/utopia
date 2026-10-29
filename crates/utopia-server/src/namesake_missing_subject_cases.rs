//! 漏列主体只允许恢复唯一既有名字；不修改原来的输入、金标准或具名失败列表。
use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Missing {
    Canonical,
    ExistingAlias,
    Typed,
    MergedOther,
    Ambiguous,
    Absent,
    Forged,
    NoQuote,
    CrossSentence,
    SharedQuote,
    RepeatedQuote,
    RepeatedName,
    MultipleQuotes,
    Described,
    AliasDeclaration,
    CompoundName,
    ObjectOnly,
    Pushed,
    Memory,
    DeclaredControl,
    ForeignBase,
    PdfWhitespace,
    Chinese,
    LowProfile,
    GreyProfile,
    NoProfile,
    TypedNoProfile,
    MixedForgedQuote,
    SubjectAndObject,
    DeclaredLongAlias,
    Bare,
    GreyNoClue,
    OtherSubjectHigh,
    OtherSubjectGrey,
    InvalidatedAlias,
    DescribedHolder,
    GenericCompetitor,
}

async fn run_missing(fx: &Fx, case: Missing, reverse: bool) -> anyhow::Result<Value> {
    let mut holder = None;
    let mut retired = None;
    for i in if reverse { [1, 0] } else { [0, 1] } {
        if i == 0 && case != Missing::Absent {
            holder = Some(fx.seed(0, Check::Resolver { target: 0 }).await?);
        } else if i == 1 && matches!(case, Missing::Ambiguous | Missing::MergedOther) {
            let other = fx.seed(1, Check::Resolver { target: 0 }).await?;
            if case == Missing::MergedOther {
                // 回收已经合并的记录不能让一个活持有者变成两个。
                retired = Some(other);
            }
        } else {
            sqlx::query(
                "INSERT INTO entities(id,kb_id,canonical_name) VALUES ($1,$2,'Other Person')",
            )
            .bind(Uuid::now_v7())
            .bind(fx.kb)
            .execute(&fx.pool)
            .await?;
        }
    }
    if case == Missing::MergedOther {
        sqlx::query("UPDATE entities SET merged_into=$1 WHERE id=$2")
            .bind(holder)
            .bind(retired)
            .execute(&fx.pool)
            .await?;
    }
    if matches!(case, Missing::ExistingAlias | Missing::InvalidatedAlias) {
        sqlx::query("UPDATE entities SET canonical_name='Alexander Lee' WHERE id=$1")
            .bind(holder)
            .execute(&fx.pool)
            .await?;
        utopia_store::names::record(&fx.pool, fx.kb, holder.unwrap(), "Alex Lee", None, None)
            .await?;
    }
    if case == Missing::InvalidatedAlias {
        sqlx::query("UPDATE facts SET invalidated_at=now() WHERE kb_id=$1 AND subject_id=$2 AND object_value->>'value'='Alex Lee'").bind(fx.kb).bind(holder).execute(&fx.pool).await?;
    }
    if case == Missing::DescribedHolder {
        sqlx::query("UPDATE entities SET description='a similarly described thing' WHERE id=$1")
            .bind(holder)
            .execute(&fx.pool)
            .await?;
    }
    if case == Missing::GenericCompetitor {
        sqlx::query("UPDATE entities SET canonical_name='Alex Lee Team' WHERE kb_id=$1 AND canonical_name='Other Person'").bind(fx.kb).execute(&fx.pool).await?;
    }
    if matches!(case, Missing::Typed | Missing::TypedNoProfile) {
        let class = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO entity_types(id,kb_id,key,label) VALUES ($1,$2,'person','Person')",
        )
        .bind(class)
        .bind(fx.kb)
        .execute(&fx.pool)
        .await?;
        sqlx::query("UPDATE entities SET type_id=$1,type_source='human' WHERE id=$2")
            .bind(class)
            .bind(holder)
            .execute(&fx.pool)
            .await?;
    }
    if matches!(case, Missing::NoProfile | Missing::TypedNoProfile) {
        sqlx::query("UPDATE entities SET profile_embedding=NULL WHERE id=$1")
            .bind(holder)
            .execute(&fx.pool)
            .await?;
    }
    if matches!(
        case,
        Missing::LowProfile
            | Missing::GreyProfile
            | Missing::GreyNoClue
            | Missing::OtherSubjectGrey
    ) {
        let vector = if case == Missing::LowProfile {
            "[0,1,0]"
        } else {
            "[0.5,0.866,0]"
        };
        sqlx::query("UPDATE entities SET profile_embedding=$1::text::vector WHERE id=$2")
            .bind(vector)
            .bind(holder)
            .execute(&fx.pool)
            .await?;
    }
    if case == Missing::ForeignBase {
        let ws = Uuid::now_v7();
        let kb = Uuid::now_v7();
        sqlx::query("INSERT INTO workspaces(id,org_id,name) VALUES ($1,$2,'other scope')")
            .bind(ws)
            .bind(fx.org)
            .execute(&fx.pool)
            .await?;
        sqlx::query(
            "INSERT INTO knowledge_bases(id,workspace_id,name) VALUES ($1,$2,'other scope')",
        )
        .bind(kb)
        .bind(ws)
        .execute(&fx.pool)
        .await?;
        sqlx::query("INSERT INTO entities(id,kb_id,canonical_name) VALUES ($1,$2,'Alex Lee')")
            .bind(Uuid::now_v7())
            .bind(kb)
            .execute(&fx.pool)
            .await?;
    }
    let initial_type: Option<(Option<Uuid>, Option<String>, String)> = match holder {
        Some(id) => Some(
            sqlx::query_as("SELECT type_id,specific_type,type_source FROM entities WHERE id=$1")
                .bind(id)
                .fetch_one(&fx.pool)
                .await?,
        ),
        None => None,
    };
    let ordinary = "Alex Lee from Platform Engineering submitted a report.";
    let mut text = ordinary.to_string();
    let mut quote = Some(ordinary.to_string());
    let mut subject = "Alex Lee";
    let mut entities = json!([]);
    let mut names = json!([]);
    let mut object = Value::Null;
    let mut value = json!("report");
    match case {
        Missing::Bare | Missing::GreyNoClue => {
            text = "Alex Lee submitted a report.".into();
            quote = Some(text.clone());
        }
        Missing::Forged => quote = Some("Alex Lee won a prize.".into()),
        Missing::NoQuote => quote = None,
        Missing::CrossSentence => {
            text = "Alex Lee spoke. Platform Engineering submitted a report.".into();
            quote = Some(text.clone());
        }
        Missing::RepeatedQuote => text = format!("{ordinary} {ordinary}"),
        Missing::RepeatedName => {
            text = "Alex Lee thanked Alex Lee for a report.".into();
            quote = Some(text.clone());
        }
        Missing::Described => entities = json!([["Alex Lee", "role", 0]]),
        Missing::AliasDeclaration => {
            text = "Alexander Lee, also known as Alex Lee, submitted a report.".into();
            quote = Some(text.clone());
            entities = json!([["Alexander Lee", "person", 1]]);
            names = json!([["Alexander Lee", "Alex Lee", text]]);
        }
        Missing::CompoundName => {
            subject = "上海";
            text = "上海市政府发布报告。".into();
            quote = Some(text.clone());
            sqlx::query("UPDATE entities SET canonical_name='上海' WHERE id=$1")
                .bind(holder)
                .execute(&fx.pool)
                .await?;
            entities = json!([["上海市政府", "organization", 1]]);
        }
        Missing::DeclaredLongAlias => {
            entities = json!([["Alex Lee Society", "organization", 1]]);
            names = json!([["Alex Lee Society", "Alex Lee", ordinary]]);
        }
        Missing::SubjectAndObject => {
            entities = json!([["Caller", "organization", 1]]);
        }
        Missing::ObjectOnly => {
            text = "Caller assigned a report to Alex Lee.".into();
            quote = Some(text.clone());
            subject = "Caller";
            entities = json!([["Caller", "organization", 1]]);
            object = json!("Alex Lee");
            value = Value::Null;
        }
        Missing::DeclaredControl => entities = json!([["Alex Lee", "person", 1]]),
        Missing::PdfWhitespace => {
            text = "Alex\n Lee from Platform Engineering submitted a report.".into()
        }
        Missing::Chinese => {
            subject = "李娜";
            text = "李娜提交报告。".into();
            quote = Some(text.clone());
            sqlx::query("UPDATE entities SET canonical_name='李娜' WHERE id=$1")
                .bind(holder)
                .execute(&fx.pool)
                .await?;
        }
        _ => {}
    }
    let mut statements = vec![json!([
        quote,
        subject,
        "submitted",
        object,
        value,
        null,
        null,
        null
    ])];
    if case == Missing::SharedQuote {
        text = "Alex Lee and Bob Jones submitted a report.".into();
        statements = vec![
            json!([
                text,
                "Alex Lee",
                "submitted",
                null,
                "report",
                null,
                null,
                null
            ]),
            json!([
                text,
                "Bob Jones",
                "submitted",
                null,
                "report",
                null,
                null,
                null
            ]),
        ];
    }
    if case == Missing::MultipleQuotes {
        let second = "Alex Lee from Finance submitted an annual report.";
        text = format!("{ordinary} {second}");
        statements.push(json!([
            second,
            "Alex Lee",
            "submitted",
            null,
            "annual report",
            null,
            null,
            null
        ]));
    }
    if matches!(case, Missing::OtherSubjectHigh | Missing::OtherSubjectGrey) {
        text = format!("{ordinary} Platform Engineering filed a budget.");
        entities = json!([["Platform Engineering", "department", 1]]);
        statements.push(json!([
            "Platform Engineering filed a budget.",
            "Platform Engineering",
            "filed",
            null,
            "budget",
            null,
            null,
            null
        ]));
    }
    if case == Missing::MixedForgedQuote {
        statements.push(json!([
            "Alex Lee won a prize.",
            "Alex Lee",
            "submitted",
            null,
            "prize",
            null,
            null,
            null
        ]));
    }
    if case == Missing::SubjectAndObject {
        text = format!("{ordinary} Caller assigned a report to Alex Lee.");
        statements.push(json!([
            "Caller assigned a report to Alex Lee.",
            "Caller",
            "submitted",
            "Alex Lee",
            null,
            null,
            null,
            null
        ]));
    }
    let reply = json!({"e":entities,"s":statements,"n":names}).to_string();
    if matches!(case, Missing::Pushed | Missing::Memory) {
        let source = Uuid::now_v7();
        let kind = if case == Missing::Pushed {
            "statements"
        } else {
            "memory"
        };
        sqlx::query(
            "INSERT INTO sources(id,kb_id,kind,name,config) VALUES ($1,$2,$3,'guard','{}')",
        )
        .bind(source)
        .bind(fx.kb)
        .bind(kind)
        .execute(&fx.pool)
        .await?;
        sqlx::query("UPDATE documents SET source_id=$1 WHERE id=$2")
            .bind(source)
            .bind(fx.docs[2])
            .execute(&fx.pool)
            .await?;
        if case == Missing::Pushed {
            text = reply.clone();
        }
    }
    sqlx::query("UPDATE chunks SET text=$1 WHERE id=$2")
        .bind(&text)
        .bind(fx.chunks[2])
        .execute(&fx.pool)
        .await?;
    if case != Missing::Pushed {
        fx.model.steps.lock().unwrap().push_back(Step {
            prompt_has: if case == Missing::PdfWhitespace {
                "Alex".into()
            } else {
                text.clone()
            },
            stream: true,
            text: reply,
        });
    }
    let entities_before: i64 = sqlx::query_scalar("SELECT count(*) FROM entities WHERE kb_id=$1")
        .bind(fx.kb)
        .fetch_one(&fx.pool)
        .await?;
    crate::extraction::extract_document(&fx.state, fx.docs[2], Default::default()).await?;
    let facts:Vec<(Uuid,Option<Uuid>,Option<Value>)>=sqlx::query_as(
        "SELECT f.subject_id,f.object_id,f.object_value FROM facts f JOIN fact_evidence fe ON fe.fact_id=f.id WHERE f.kb_id=$1 AND fe.document_id=$2 AND f.layer='open' AND f.phrase='submitted' ORDER BY f.id")
        .bind(fx.kb).bind(fx.docs[2]).fetch_all(&fx.pool).await?;
    let recovered = matches!(
        case,
        Missing::Canonical
            | Missing::ExistingAlias
            | Missing::Typed
            | Missing::MergedOther
            | Missing::DeclaredControl
            | Missing::ForeignBase
            | Missing::PdfWhitespace
            | Missing::Chinese
            | Missing::GreyProfile
            | Missing::NoProfile
            | Missing::Bare
            | Missing::OtherSubjectHigh
    );
    let correct = if recovered {
        facts.len() == 1 && Some(facts[0].0) == holder
    } else if case == Missing::SubjectAndObject {
        facts.len() == 2
            && facts.iter().any(|f| Some(f.0) == holder)
            && facts.iter().any(|f| {
                Some(f.0) != holder
                    && f.1.is_none()
                    && f.2.as_ref().is_some_and(|v| v["value"] == "Alex Lee")
            })
    } else if case == Missing::Described {
        facts.len() == 1
            && Some(facts[0].0) != holder
            && sqlx::query_scalar::<_, bool>(
                "SELECT description IS NOT NULL FROM entities WHERE id=$1",
            )
            .bind(facts[0].0)
            .fetch_one(&fx.pool)
            .await?
    } else if case == Missing::ObjectOnly {
        facts.len() == 1
            && facts[0].1.is_none()
            && facts[0]
                .2
                .as_ref()
                .is_some_and(|v| v["value"] == "Alex Lee")
    } else {
        facts.is_empty()
    };
    let pending: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM resolution_reviews WHERE kb_id=$1 AND status='pending'",
    )
    .bind(fx.kb)
    .fetch_one(&fx.pool)
    .await?;
    let entities_after: i64 = sqlx::query_scalar("SELECT count(*) FROM entities WHERE kb_id=$1")
        .bind(fx.kb)
        .fetch_one(&fx.pool)
        .await?;
    let type_unchanged = if matches!(case, Missing::Typed | Missing::TypedNoProfile) {
        initial_type
            == Some(
                sqlx::query_as(
                    "SELECT type_id,specific_type,type_source FROM entities WHERE id=$1",
                )
                .bind(holder)
                .fetch_one(&fx.pool)
                .await?,
            )
    } else {
        true
    };
    // 再次抽取已完成的块不能新增事实、实体或模型请求。
    let calls = fx.model.requests.lock().unwrap().len();
    crate::extraction::extract_document(&fx.state, fx.docs[2], Default::default()).await?;
    let final_entities: i64 = sqlx::query_scalar("SELECT count(*) FROM entities WHERE kb_id=$1")
        .bind(fx.kb)
        .fetch_one(&fx.pool)
        .await?;
    let final_facts:i64=sqlx::query_scalar("SELECT count(*) FROM facts f JOIN fact_evidence fe ON fe.fact_id=f.id WHERE f.kb_id=$1 AND fe.document_id=$2 AND f.layer='open' AND f.phrase='submitted'").bind(fx.kb).bind(fx.docs[2]).fetch_one(&fx.pool).await?;
    let final_pending: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM resolution_reviews WHERE kb_id=$1 AND status='pending'",
    )
    .bind(fx.kb)
    .fetch_one(&fx.pool)
    .await?;
    let repeated_calls = fx.model.requests.lock().unwrap().len();
    let no_extra_entities = matches!(
        case,
        Missing::Described
            | Missing::AliasDeclaration
            | Missing::CompoundName
            | Missing::ObjectOnly
            | Missing::SubjectAndObject
            | Missing::DeclaredLongAlias
    ) || entities_before == entities_after;
    Ok(
        json!({"case":format!("{case:?}"),"reverse":reverse,"passed":correct&&pending==0&&type_unchanged&&no_extra_entities&&final_entities==entities_after&&calls==repeated_calls&&final_facts==facts.len() as i64&&final_pending==pending,
        "facts":facts,"existing_holder":holder,"entities_before":entities_before,"entities_after":entities_after,
        "pending_reviews":pending,"type_unchanged":type_unchanged,"model_calls":calls,"repeated_model_calls":repeated_calls}),
    )
}

#[tokio::test]
async fn missing_subjects_only_recover_grounded_unique_existing_names() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = utopia_store::db::connect(&url, Some(4)).await?;
    let cases = [
        Missing::Canonical,
        Missing::ExistingAlias,
        Missing::Typed,
        Missing::MergedOther,
        Missing::Ambiguous,
        Missing::Absent,
        Missing::Forged,
        Missing::NoQuote,
        Missing::CrossSentence,
        Missing::SharedQuote,
        Missing::RepeatedQuote,
        Missing::RepeatedName,
        Missing::MultipleQuotes,
        Missing::Described,
        Missing::AliasDeclaration,
        Missing::CompoundName,
        Missing::ObjectOnly,
        Missing::Pushed,
        Missing::Memory,
        Missing::DeclaredControl,
        Missing::ForeignBase,
        Missing::PdfWhitespace,
        Missing::Chinese,
        Missing::LowProfile,
        Missing::GreyProfile,
        Missing::NoProfile,
        Missing::TypedNoProfile,
        Missing::MixedForgedQuote,
        Missing::SubjectAndObject,
        Missing::DeclaredLongAlias,
        Missing::Bare,
        Missing::GreyNoClue,
        Missing::OtherSubjectHigh,
        Missing::OtherSubjectGrey,
        Missing::InvalidatedAlias,
        Missing::DescribedHolder,
        Missing::GenericCompetitor,
    ];
    let mut failures = Vec::new();
    for case in cases {
        for reverse in [false, true] {
            let template = Case {
                id: "missing-subject",
                check: Check::Resolver { target: 0 },
            };
            let fx = Fx::new(&pool, &template, Uuid::now_v7()).await?;
            let result = run_missing(&fx, case, reverse).await;
            let cleanup = fx.cleanup().await;
            let observation = result?;
            cleanup?;
            println!("GROUNDED_SUBJECT_CASE {observation}");
            if observation["passed"] != true {
                failures.push(observation)
            }
        }
    }
    anyhow::ensure!(
        failures.is_empty(),
        "subject recovery mismatches: {}",
        json!(failures)
    );
    pool.close().await;
    Ok(())
}

/// 保留三份历史回复与原文；这里只重放抽取落库，不冒充新模型测量或完整历史图重放。
/// 画像用等余弦的三维向量隔离门槛，完整 BGE-M3 向量仍留在冻结快照中。
#[tokio::test]
async fn historical_omitted_subject_replies_keep_grounded_facts() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = utopia_store::db::connect(&url, Some(4)).await?;
    let rows: Vec<Value> = serde_json::from_str(include_str!(
        "../tests/fixtures/grounded_subject_replies.json"
    ))?;
    let mut failures = Vec::new();
    for row in rows {
        for reverse in [false, true] {
            let template = Case {
                id: "historical-missing-subject",
                check: Check::Resolver { target: 0 },
            };
            let fx = Fx::new(&pool, &template, Uuid::now_v7()).await?;
            let result=async {
                let mut holder=None;
                for i in if reverse {[1,0]} else {[0,1]} {
                    if i==0 && row["has_holder"]==true {
                        let id=fx.seed(0,Check::Resolver {target:0}).await?;
                        let class=Uuid::now_v7();
                        sqlx::query("INSERT INTO entity_types(id,kb_id,key,label) VALUES ($1,$2,'person','Person')").bind(class).bind(fx.kb).execute(&pool).await?;
                        let score=row["profile_cosine"].as_f64().unwrap();
                        let profile=format!("[{score},{},0]",(1.0-score*score).sqrt());
                        sqlx::query("UPDATE entities SET canonical_name='李娜',type_id=$1,type_source='human',profile_embedding=$2::text::vector WHERE id=$3").bind(class).bind(profile).bind(id).execute(&pool).await?;
                        holder=Some(id);
                    } else {
                        sqlx::query("INSERT INTO entities(id,kb_id,canonical_name) VALUES ($1,$2,'Other Person')").bind(Uuid::now_v7()).bind(fx.kb).execute(&pool).await?;
                    }
                }
                let text=row["text"].as_str().unwrap();
                sqlx::query("UPDATE chunks SET text=$1 WHERE id=$2").bind(text).bind(fx.chunks[2]).execute(&pool).await?;
                fx.model.steps.lock().unwrap().push_back(Step {prompt_has:"李娜主持编写".into(),stream:true,text:row["reply"].as_str().unwrap().into()});
                crate::extraction::extract_document(&fx.state,fx.docs[2],Default::default()).await?;
                let subjects:Vec<Uuid>=sqlx::query_scalar("SELECT f.subject_id FROM facts f JOIN fact_evidence fe ON fe.fact_id=f.id WHERE f.kb_id=$1 AND fe.document_id=$2 AND f.layer='open' AND f.phrase='主持编写了'").bind(fx.kb).bind(fx.docs[2]).fetch_all(&pool).await?;
                let people:i64=sqlx::query_scalar("SELECT count(*) FROM entities WHERE kb_id=$1 AND canonical_name='李娜' AND merged_into IS NULL").bind(fx.kb).fetch_one(&pool).await?;
                let pending:i64=sqlx::query_scalar("SELECT count(*) FROM resolution_reviews WHERE kb_id=$1 AND status='pending'").bind(fx.kb).fetch_one(&pool).await?;
                let passed=match holder {Some(id)=>subjects==vec![id] && people==1,None=>subjects.is_empty() && people==0};
                Ok::<_,anyhow::Error>(json!({"snapshot":row["snapshot"],"reverse_seed":reverse,"subjects":subjects,"people":people,"pending_reviews":pending,"model_calls":fx.model.requests.lock().unwrap().len(),"passed":passed&&pending==0}))
            }.await;
            let cleanup = fx.cleanup().await;
            let observation = result?;
            cleanup?;
            println!("HISTORICAL_SUBJECT_CASE {observation}");
            if observation["passed"] != true {
                failures.push(observation);
            }
        }
    }
    anyhow::ensure!(
        failures.is_empty(),
        "historical subject recovery mismatches: {}",
        json!(failures)
    );
    pool.close().await;
    Ok(())
}
