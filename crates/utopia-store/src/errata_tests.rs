//! 勘误（0044 决定 7）：结构先报、撤销经得住物化、闸门留给人、改与加带着原话。
//! 没有 `UTOPIA_DATABASE_URL` 时跳过。
use super::*;
use crate::materialize;
use sqlx::PgPool;
use std::collections::HashMap;

struct Fx {
    org: Uuid,
    kb: Uuid,
    doc: Uuid,
    chunk: Uuid,
    organization: Uuid,
    place: Uuid,
    person: Uuid,
    based_in: Uuid,
    ceo: Uuid,
    founded: Uuid,
    acme: Uuid,
    london: Uuid,
    paris: Uuid,
    jane: Uuid,
    nobody: Uuid,
    /// 答留给人那几笔的人
    user: Uuid,
    text: &'static str,
}

const TEXT: &str = "Acme is based in London. Jane Roe runs Acme. Acme was founded in 1999. Zeta Corp is based in London.";

/// 一个库：organization / place / person；based_in（organization → place）、ceo（organization →
/// person，只许一个）、founded（日期属性）；一份文档一块正文；Acme、London、Paris、Jane Roe、
/// Nobody 五样东西
async fn seed(pool: &PgPool) -> anyhow::Result<Fx> {
    let ids: Vec<Uuid> = (0..17).map(|_| Uuid::now_v7()).collect();
    let (org, ws, kb, doc, chunk, organization, place, person, based_in, ceo, founded) = (
        ids[0], ids[1], ids[2], ids[3], ids[4], ids[5], ids[6], ids[7], ids[8], ids[9], ids[10],
    );
    let (acme, london, paris, jane, nobody, user) =
        (ids[11], ids[12], ids[13], ids[14], ids[15], ids[16]);
    sqlx::raw_sql(&format!(
        "INSERT INTO organizations(id,name) VALUES ('{org}','errata-test');
         INSERT INTO workspaces(id,org_id,name) VALUES ('{ws}','{org}','errata-test');
         INSERT INTO knowledge_bases(id,workspace_id,name) VALUES ('{kb}','{ws}','errata-test');
         INSERT INTO users(id,org_id,email,display_name,password_hash) VALUES ('{user}','{org}','{user}@errata.test','reviewer','unused');
         INSERT INTO documents(id,kb_id,filename,sha256,doc_time) VALUES ('{doc}','{kb}','acme.txt','x','2020-01-01T00:00:00Z');
         INSERT INTO chunks(id,kb_id,document_id,seq,text) VALUES ('{chunk}','{kb}','{doc}',0,'{TEXT}');
         INSERT INTO entity_types(id,kb_id,key,label,color,shape) VALUES
             ('{organization}','{kb}','organization','Organization','#000','circle'),
             ('{place}','{kb}','place','Place','#000','circle'),
             ('{person}','{kb}','person','Person','#000','circle');
         INSERT INTO relation_types(id,kb_id,key,label,kind,temporal,functional,datatype) VALUES
             ('{based_in}','{kb}','based_in','based in','relation','state',false,NULL),
             ('{ceo}','{kb}','ceo','chief executive','relation','state',true,NULL),
             ('{founded}','{kb}','founded','founded','attribute','eternal',false,'date');
         INSERT INTO relation_type_domains(relation_type_id,entity_type_id) VALUES
             ('{based_in}','{organization}'), ('{ceo}','{organization}'), ('{founded}','{organization}');
         INSERT INTO relation_type_ranges(relation_type_id,entity_type_id) VALUES
             ('{based_in}','{place}'), ('{ceo}','{person}');
         INSERT INTO entities(id,kb_id,canonical_name,type_id) VALUES
             ('{acme}','{kb}','Acme','{organization}'), ('{london}','{kb}','London','{place}'),
             ('{paris}','{kb}','Paris','{place}'), ('{jane}','{kb}','Jane Roe','{person}'),
             ('{nobody}','{kb}','Nobody','{person}');"
    ))
    .execute(pool)
    .await?;
    Ok(Fx {
        org,
        kb,
        doc,
        chunk,
        organization,
        place,
        person,
        based_in,
        ceo,
        founded,
        acme,
        london,
        paris,
        jane,
        nobody,
        user,
        text: TEXT,
    })
}

async fn cleanup(pool: &PgPool, f: &Fx) -> anyhow::Result<()> {
    sqlx::query("DELETE FROM jobs WHERE payload->>'kb_id'=$1")
        .bind(f.kb.to_string())
        .execute(pool)
        .await?;
    sqlx::query("DELETE FROM organizations WHERE id=$1")
        .bind(f.org)
        .execute(pool)
        .await?;
    Ok(())
}

/// 一条开放陈述加一条从它算出的类型化行，证据落在这份文档上。返回 (陈述, 类型化行)
async fn typed(
    pool: &PgPool,
    f: &Fx,
    subject: Uuid,
    phrase: &str,
    property: Uuid,
    object: Option<Uuid>,
    value: Option<&str>,
) -> anyhow::Result<(Uuid, Uuid)> {
    let (s, t) = (Uuid::now_v7(), Uuid::now_v7());
    let value = value.map(|v| serde_json::json!({ "value": v }));
    sqlx::query(
        "INSERT INTO facts(id,kb_id,subject_id,object_id,object_value,layer,phrase) VALUES ($1,$2,$3,$4,$5,'open',$6)",
    )
    .bind(s)
    .bind(f.kb)
    .bind(subject)
    .bind(object)
    .bind(&value)
    .bind(phrase)
    .execute(pool)
    .await?;
    sqlx::query(
        "INSERT INTO facts(id,kb_id,subject_id,predicate_id,object_id,object_value,layer,from_statement_id)
         VALUES ($1,$2,$3,$4,$5,$6,'typed',$7)",
    )
    .bind(t)
    .bind(f.kb)
    .bind(subject)
    .bind(property)
    .bind(object)
    .bind(&value)
    .bind(s)
    .execute(pool)
    .await?;
    sqlx::query("INSERT INTO typed_fact_sources(fact_id,statement_id) VALUES ($1,$2)")
        .bind(t)
        .bind(s)
        .execute(pool)
        .await?;
    for fact in [s, t] {
        sqlx::query(
            "INSERT INTO fact_evidence(fact_id,chunk_id,quote,document_id,doc_version) VALUES ($1,$2,$3,$4,1)",
        )
        .bind(fact)
        .bind(f.chunk)
        .bind(phrase)
        .bind(f.doc)
        .execute(pool)
        .await?;
    }
    Ok((s, t))
}

async fn live(pool: &PgPool, fact: Uuid) -> anyhow::Result<bool> {
    Ok(
        sqlx::query_scalar("SELECT invalidated_at IS NULL FROM facts WHERE id=$1")
            .bind(fact)
            .fetch_one(pool)
            .await?,
    )
}

async fn status_of(pool: &PgPool, action: Uuid) -> anyhow::Result<(String, Option<String>)> {
    Ok(
        sqlx::query_as("SELECT status, detail FROM errata_actions WHERE id=$1")
            .bind(action)
            .fetch_one(pool)
            .await?,
    )
}

fn input<'a>(
    f: &Fx,
    run: Uuid,
    c: Option<&'a Candidate>,
    proposed: Proposed,
    quote: Option<&'a str>,
) -> ActionInput<'a> {
    ActionInput {
        run_id: run,
        document_id: f.doc,
        candidate: c,
        proposed,
        reason: "because",
        quote,
        document_text: f.text,
    }
}

#[tokio::test]
async fn structure_flags_facts_and_the_flagged_come_first() -> anyhow::Result<()> {
    let Some(url) = crate::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    crate::db::migrate(&pool).await?;
    let f = seed(&pool).await?;
    let run = async {
        let (_, fine) = typed(
            &pool,
            &f,
            f.acme,
            "based in",
            f.based_in,
            Some(f.london),
            None,
        )
        .await?;
        let (_, absent) = typed(
            &pool,
            &f,
            f.acme,
            "based in",
            f.based_in,
            Some(f.paris),
            None,
        )
        .await?;
        let (_, domain) = typed(
            &pool,
            &f,
            f.jane,
            "based in",
            f.based_in,
            Some(f.london),
            None,
        )
        .await?;
        let (_, range) = typed(&pool, &f, f.acme, "run by", f.ceo, Some(f.london), None).await?;
        let (_, no_date) = typed(
            &pool,
            &f,
            f.acme,
            "founded",
            f.founded,
            None,
            Some("the nineties"),
        )
        .await?;
        let (_, dated) = typed(
            &pool,
            &f,
            f.acme,
            "founded in",
            f.founded,
            None,
            Some("1999"),
        )
        .await?;
        let (_, nobody) = typed(&pool, &f, f.acme, "run by", f.ceo, Some(f.nobody), None).await?;
        let c = candidates(&pool, f.kb, f.doc).await?;
        let flags: HashMap<Uuid, Option<String>> =
            c.iter().map(|c| (c.fact_id, c.flag.clone())).collect();
        assert_eq!(flags[&fine], None);
        assert_eq!(flags[&dated], None);
        assert_eq!(flags[&absent].as_deref(), Some("name_absent"));
        assert_eq!(flags[&domain].as_deref(), Some("domain"));
        assert_eq!(flags[&range].as_deref(), Some("range"));
        assert_eq!(flags[&no_date].as_deref(), Some("no_date"));
        assert_eq!(flags[&nobody].as_deref(), Some("name_absent"));
        // 报了的在前，没报的在后
        let order: Vec<bool> = c.iter().map(|c| c.flag.is_some()).collect();
        assert_eq!(order, vec![true, true, true, true, true, false, false]);
        let first = &c[0];
        assert_eq!(
            (first.subject.as_str(), first.property.as_str()),
            ("Acme", "based_in")
        );
        assert_eq!(first.quote.as_deref(), Some("based in"));
        assert!(first.statement_id.is_some());
        // 这份文档待看；看过一条（keep）之后它还待看，全看完才不
        assert_eq!(documents_due(&pool, f.kb, 10).await?, vec![f.doc]);
        let run = start_run(&pool, f.kb, f.doc, 5, 2).await?;
        let r = record(
            &pool,
            f.kb,
            input(&f, run, Some(first), Proposed::Keep, None),
        )
        .await?;
        assert_eq!(r.status, "applied");
        assert_eq!(candidates(&pool, f.kb, f.doc).await?.len(), 6);
        assert_eq!(documents_due(&pool, f.kb, 10).await?, vec![f.doc]);
        // 抽完了、一条类型化行都没有的文档也该看一次（agent 只能加）；看过一次就不再排
        let empty_doc = Uuid::now_v7();
        sqlx::query("INSERT INTO documents(id,kb_id,filename,sha256,graph_status) VALUES ($1,$2,'empty.txt','y','done')")
            .bind(empty_doc)
            .bind(f.kb)
            .execute(&pool)
            .await?;
        assert_eq!(documents_due(&pool, f.kb, 10).await?, vec![f.doc, empty_doc]);
        let empty_run = start_run(&pool, f.kb, empty_doc, 0, 0).await?;
        assert_eq!(runs_of(&pool, empty_doc).await?, 1);
        finish_run(&pool, empty_run, 1, None, None).await?;
        assert_eq!(documents_due(&pool, f.kb, 10).await?, vec![f.doc]);
        finish_run(&pool, run, 1, Some(100), Some(20)).await?;
        let usage: (i32, Option<i64>) =
            sqlx::query_as("SELECT requests, prompt_tokens FROM errata_runs WHERE id=$1")
                .bind(run)
                .fetch_one(&pool)
                .await?;
        assert_eq!(usage, (1, Some(100)));
        anyhow::Ok(())
    }
    .await;
    cleanup(&pool, &f).await?;
    run
}

#[tokio::test]
async fn a_retraction_sticks_through_materialisation() -> anyhow::Result<()> {
    let Some(url) = crate::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    crate::db::migrate(&pool).await?;
    let f = seed(&pool).await?;
    let run = async {
        // 一条陈述 Acme —based in→ Paris，签名绑到 based_in；物化算出类型化行
        let s = Uuid::now_v7();
        sqlx::query("INSERT INTO facts(id,kb_id,subject_id,object_id,layer,phrase) VALUES ($1,$2,$3,$4,'open','based in')")
            .bind(s).bind(f.kb).bind(f.acme).bind(f.paris).execute(&pool).await?;
        sqlx::query("INSERT INTO fact_evidence(fact_id,chunk_id,quote,document_id,doc_version) VALUES ($1,$2,'based in',$3,1)")
            .bind(s).bind(f.chunk).bind(f.doc).execute(&pool).await?;
        sqlx::query(
            "INSERT INTO phrase_bindings (id, kb_id, phrase, subject_type_id, object_type_id, object_is_value,
                                          relation_type_id, direction, status)
             VALUES ($1, $2, 'based in', $3, $4, false, $5, 'forward', 'bound')",
        )
        .bind(Uuid::now_v7()).bind(f.kb).bind(f.organization).bind(f.place).bind(f.based_in)
        .execute(&pool).await?;
        let o = materialize::materialize(&pool, f.kb).await?;
        assert_eq!(o.added, 1);
        let c = candidates(&pool, f.kb, f.doc).await?;
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].flag.as_deref(), Some("name_absent"), "Paris is not in the document");
        assert_eq!(c[0].statement_id, Some(s));
        let typed_row = c[0].fact_id;
        let run = start_run(&pool, f.kb, f.doc, 1, 0).await?;
        let r = record(&pool, f.kb, input(&f, run, Some(&c[0]), Proposed::Retract, Some("Acme is based in London"))).await?;
        assert_eq!((r.status, r.detail), ("applied", None));
        assert!(!live(&pool, typed_row).await?);
        // 陈述还活着、绑定还在：物化再跑，那一对不再算出来
        let o = materialize::materialize(&pool, f.kb).await?;
        assert_eq!((o.added, o.merged, o.retired), (0, 0, 0));
        let live_typed: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM facts WHERE kb_id=$1 AND layer='typed' AND invalidated_at IS NULL",
        )
        .bind(f.kb)
        .fetch_one(&pool)
        .await?;
        assert_eq!(live_typed, 0);
        assert!(documents_due(&pool, f.kb, 10).await?.is_empty(), "nothing left to look at");
        // 引文不是原话的撤：记下来，不执行
        let (_, other) = typed(&pool, &f, f.acme, "based in", f.based_in, Some(f.london), None).await?;
        let c = candidates(&pool, f.kb, f.doc).await?;
        let r = record(&pool, f.kb, input(&f, run, Some(&c[0]), Proposed::Retract, Some("Acme is based in Paris"))).await?;
        assert_eq!(r.status, "refused");
        assert_eq!(r.detail.as_deref(), Some("the quote is not in the document"));
        assert!(live(&pool, other).await?);
        anyhow::Ok(())
    }
    .await;
    cleanup(&pool, &f).await?;
    run
}

#[tokio::test]
async fn a_retraction_something_rests_on_is_held_until_a_person_decides() -> anyhow::Result<()> {
    let Some(url) = crate::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    crate::db::migrate(&pool).await?;
    let f = seed(&pool).await?;
    let run = async {
        let (_, t1) = typed(&pool, &f, f.acme, "based in", f.based_in, Some(f.paris), None).await?;
        let (_, t2) = typed(&pool, &f, f.acme, "run by", f.ceo, Some(f.nobody), None).await?;
        // 两条各有一条派生靠着
        let rule = Uuid::now_v7();
        sqlx::query("INSERT INTO rules(id,kb_id,predicate_id,kind) VALUES ($1,$2,$3,'transitive')")
            .bind(rule).bind(f.kb).bind(f.based_in).execute(&pool).await?;
        for (premise, object) in [(t1, f.london), (t2, f.paris)] {
            let d = Uuid::now_v7();
            sqlx::query("INSERT INTO derived_facts(id,kb_id,subject_id,predicate_id,object_id,rule_id) VALUES ($1,$2,$3,$4,$5,$6)")
                .bind(d).bind(f.kb).bind(f.acme).bind(f.based_in).bind(object).bind(rule).execute(&pool).await?;
            sqlx::query("INSERT INTO fact_derivations(derived_fact_id,premise_fact_id,seq) VALUES ($1,$2,0)")
                .bind(d).bind(premise).execute(&pool).await?;
        }
        let c = candidates(&pool, f.kb, f.doc).await?;
        let run = start_run(&pool, f.kb, f.doc, 2, 0).await?;
        let mut held_ids = Vec::new();
        for cand in &c {
            let r = record(&pool, f.kb, input(&f, run, Some(cand), Proposed::Retract, Some("Acme is based in London"))).await?;
            assert_eq!(r.status, "held");
            assert_eq!(r.detail.as_deref(), Some("derived 1"));
            held_ids.push(r.id);
        }
        assert!(live(&pool, t1).await? && live(&pool, t2).await?, "held means untouched");
        let queue = held(&pool, f.kb, 10, 0).await?;
        assert_eq!(queue.len(), 2);
        assert_eq!(queue[0].document, "acme.txt");
        assert_eq!(queue[0].detail.as_deref(), Some("derived 1"));
        assert_eq!(queue[0].proposed.as_ref().unwrap()["subject"], "Acme");
        assert_eq!(waiting(&pool, f.kb).await?.0, 2);
        // 人否第一笔：事实还在；批第二笔：撤了
        let actor = f.user;
        assert!(decide_held(&pool, f.kb, held_ids[0], false, actor).await?);
        assert!(decide_held(&pool, f.kb, held_ids[1], true, actor).await?);
        assert!(!decide_held(&pool, f.kb, held_ids[1], true, actor).await?, "answered once");
        assert_eq!(status_of(&pool, held_ids[0]).await?.0, "rejected");
        assert_eq!(status_of(&pool, held_ids[1]).await?.0, "applied");
        assert!(live(&pool, c[0].fact_id).await?);
        assert!(!live(&pool, c[1].fact_id).await?);
        assert_eq!(waiting(&pool, f.kb).await?.0, 0);
        anyhow::Ok(())
    }
    .await;
    cleanup(&pool, &f).await?;
    run
}

#[tokio::test]
async fn an_unflagged_fact_is_retracted_only_by_a_person() -> anyhow::Result<()> {
    let Some(url) = crate::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    crate::db::migrate(&pool).await?;
    let f = seed(&pool).await?;
    let run = async {
        let (_, fine) = typed(
            &pool,
            &f,
            f.acme,
            "based in",
            f.based_in,
            Some(f.london),
            None,
        )
        .await?;
        let c = candidates(&pool, f.kb, f.doc).await?;
        assert_eq!(c[0].flag, None);
        let run = start_run(&pool, f.kb, f.doc, 0, 1).await?;
        let r = record(
            &pool,
            f.kb,
            input(
                &f,
                run,
                Some(&c[0]),
                Proposed::Retract,
                Some("Acme is based in London"),
            ),
        )
        .await?;
        assert_eq!((r.status, r.detail.as_deref()), ("held", Some("unflagged")));
        assert!(live(&pool, fine).await?);
        assert_eq!(
            held(&pool, f.kb, 10, 0).await?[0].detail.as_deref(),
            Some("unflagged")
        );
        assert!(decide_held(&pool, f.kb, r.id, true, f.user).await?);
        assert!(!live(&pool, fine).await?);
        anyhow::Ok(())
    }
    .await;
    cleanup(&pool, &f).await?;
    run
}

#[tokio::test]
async fn a_revision_and_an_addition_write_typed_facts_with_the_documents_words(
) -> anyhow::Result<()> {
    let Some(url) = crate::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    crate::db::migrate(&pool).await?;
    let f = seed(&pool).await?;
    let run = async {
        let (_, wrong_ceo) = typed(&pool, &f, f.acme, "run by", f.ceo, Some(f.london), None).await?;
        let c = candidates(&pool, f.kb, f.doc).await?;
        assert_eq!(c[0].flag.as_deref(), Some("range"));
        let run = start_run(&pool, f.kb, f.doc, 1, 0).await?;
        // 改宾语：London → Jane Roe。旧行撤了，新行活着、带引文
        let r = record(
            &pool,
            f.kb,
            input(&f, run, Some(&c[0]), Proposed::Revise { property: None, object: Some("Jane Roe".into()) }, Some("Jane Roe runs Acme")),
        )
        .await?;
        assert_eq!((r.status, r.detail.clone()), ("applied", None));
        let new = r.new_fact_id.expect("a revision writes a row");
        assert!(!live(&pool, wrong_ceo).await?);
        let row: (Uuid, Uuid, Option<Uuid>, bool) = sqlx::query_as(
            "SELECT subject_id, predicate_id, object_id, invalidated_at IS NULL FROM facts WHERE id=$1",
        )
        .bind(new)
        .fetch_one(&pool)
        .await?;
        assert_eq!(row, (f.acme, f.ceo, Some(f.jane), true));
        let quote: (Uuid, Option<String>) =
            sqlx::query_as("SELECT chunk_id, quote FROM fact_evidence WHERE fact_id=$1")
                .bind(new)
                .fetch_one(&pool)
                .await?;
        assert_eq!(quote, (f.chunk, Some("Jane Roe runs Acme".into())));
        // 加一条日期属性：值落下
        let r = record(
            &pool,
            f.kb,
            input(&f, run, None, Proposed::Add { subject: "Acme".into(), property: "founded".into(), object: "1999".into() }, Some("founded in 1999")),
        )
        .await?;
        assert_eq!(r.status, "applied");
        let value: Option<serde_json::Value> =
            sqlx::query_scalar("SELECT object_value FROM facts WHERE id=$1")
                .bind(r.new_fact_id.unwrap())
                .fetch_one(&pool)
                .await?;
        assert_eq!(value, Some(serde_json::json!({ "value": "1999" })));
        // 名字不在库里：勘误不造东西
        let r = record(
            &pool,
            f.kb,
            input(&f, run, None, Proposed::Add { subject: "Acme".into(), property: "ceo".into(), object: "Bob".into() }, Some("runs Acme")),
        )
        .await?;
        assert_eq!(r.status, "refused");
        assert_eq!(
            r.detail.as_deref(),
            Some("no thing named \"Bob\" and the document does not name it")
        );
        // 文档提到的新东西：建一个带名字的实体，事实落下
        let r = record(
            &pool,
            f.kb,
            input(&f, run, None, Proposed::Add { subject: "Zeta Corp".into(), property: "based_in".into(), object: "London".into() }, Some("Zeta Corp is based in London")),
        )
        .await?;
        assert_eq!((r.status, r.detail.clone()), ("applied", None));
        let zeta: (String, Option<Uuid>) = sqlx::query_as(
            "SELECT e.canonical_name, f.object_id FROM facts f JOIN entities e ON e.id = f.subject_id WHERE f.id = $1",
        )
        .bind(r.new_fact_id.unwrap())
        .fetch_one(&pool)
        .await?;
        assert_eq!(zeta, ("Zeta Corp".into(), Some(f.london)));
        // 只许一个值的谓词已经有 Jane：再加一个 London 会开出违规，留给人
        let r = record(
            &pool,
            f.kb,
            input(&f, run, None, Proposed::Add { subject: "Acme".into(), property: "ceo".into(), object: "London".into() }, Some("based in London")),
        )
        .await?;
        assert_eq!(r.status, "held");
        assert_eq!(r.detail.as_deref(), Some("contradiction chief executive"));
        // 属性不存在
        let r = record(
            &pool,
            f.kb,
            input(&f, run, None, Proposed::Add { subject: "Acme".into(), property: "owner".into(), object: "Jane Roe".into() }, Some("runs Acme")),
        )
        .await?;
        assert_eq!(r.status, "refused");
        assert_eq!(r.detail.as_deref(), Some("no property with key \"owner\""));
        let _ = (f.person, f.place, f.organization, f.paris, f.founded);
        anyhow::Ok(())
    }
    .await;
    cleanup(&pool, &f).await?;
    run
}
