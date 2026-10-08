//! 签名与类别词的扫描一趟聚完（#1096）：多于三条的组只取最早的三条例句，引文跟着自己的
//! 例句走；计数与短语都相同、两端的类不同的签名按类的 id 定序，每次一样。
use sqlx::PgPool;
use utopia_store::graph::{self, FactObject};
use utopia_store::{phrase_bindings, type_bindings};
use uuid::Uuid;

/// 一条签名的键与计数：短语、主语的类、宾语的类、宾语是不是字面值、陈述数
type Shape = (String, Option<Uuid>, Option<Uuid>, bool, i64);

#[tokio::test]
async fn tied_signatures_come_in_a_fixed_order() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    utopia_store::db::migrate(&pool).await?;
    let (org, ws, kb) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    // v7 按生成先后递增：a 类的 id 小于 b 类
    let (a, b) = (Uuid::now_v7(), Uuid::now_v7());
    sqlx::raw_sql(&format!(
        "INSERT INTO organizations(id,name) VALUES ('{org}','tied-signatures');
         INSERT INTO workspaces(id,org_id,name) VALUES ('{ws}','{org}','tied-signatures');
         INSERT INTO knowledge_bases(id,workspace_id,name) VALUES ('{kb}','{ws}','tied-signatures');
         INSERT INTO entity_types(id,kb_id,key,label,color,shape) VALUES
             ('{a}','{kb}','a','A','#000','circle'), ('{b}','{kb}','b','B','#000','circle');"
    ))
    .execute(&pool)
    .await?;
    let result = async {
        // 类别词 company 下四种写法，各 4、3、2、1 个实体：写法只留前三
        let mut subjects = Vec::new();
        for (spelling, n) in [
            ("Company", 4),
            ("company", 3),
            ("COMPANY", 2),
            ("CoMpany", 1),
        ] {
            for i in 0..n {
                subjects.push(entity(&pool, kb, &format!("{spelling}-{i}"), a, spelling).await?);
            }
        }
        let target = entity(&pool, kb, "target", b, "place").await?;

        // 一组五条：例句取最早的三条，引文跟着各自的陈述（第一条没有证据，引文为空）
        let mut supplies = Vec::new();
        for s in &subjects[..5] {
            supplies.push(say(&pool, kb, *s, "supplies", FactObject::Entity(target)).await?);
        }
        let doc = Uuid::now_v7();
        sqlx::query("INSERT INTO documents(id,kb_id,filename,sha256) VALUES($1,$2,$3,$3)")
            .bind(doc)
            .bind(kb)
            .bind(doc.to_string())
            .execute(&pool)
            .await?;
        for (seq, fact) in supplies.iter().enumerate().skip(1) {
            let (chunk, text) = (Uuid::now_v7(), format!("q{seq}"));
            sqlx::query("INSERT INTO chunks(id,kb_id,document_id,seq,text) VALUES($1,$2,$3,$4,$5)")
                .bind(chunk)
                .bind(kb)
                .bind(doc)
                .bind(seq as i32)
                .bind(&text)
                .execute(&pool)
                .await?;
            graph::add_evidence_located(&pool, *fact, chunk, Some(&text), None, Some((0, 2)))
                .await?;
        }

        // 三组「owns」各两条，计数与短语都相同，只差两端的类与宾语是不是字面值
        let (x, y) = (
            serde_json::json!({ "value": "x" }),
            serde_json::json!({ "value": "y" }),
        );
        for s in &subjects[..2] {
            say(&pool, kb, target, "owns", FactObject::Entity(*s)).await?; // b → a
            say(&pool, kb, *s, "owns", FactObject::Entity(target)).await?; // a → b
        }
        say(&pool, kb, subjects[0], "owns", FactObject::Value(&x)).await?; // a → 字面值
        say(&pool, kb, subjects[1], "owns", FactObject::Value(&y)).await?;

        let sigs = phrase_bindings::signatures(&pool, kb).await?;
        let shape: Vec<Shape> = sigs
            .iter()
            .map(|s| {
                (
                    s.phrase.clone(),
                    s.subject_type_id,
                    s.object_type_id,
                    s.object_is_value,
                    s.count,
                )
            })
            .collect();
        anyhow::ensure!(
            shape
                == vec![
                    ("supplies".into(), Some(a), Some(b), false, 5),
                    // 平手按主语的类、宾语的类（空的在后）、宾语是不是字面值
                    ("owns".into(), Some(a), Some(b), false, 2),
                    ("owns".into(), Some(a), None, true, 2),
                    ("owns".into(), Some(b), Some(a), false, 2),
                ],
            "{shape:#?}"
        );
        let supplied = &sigs[0];
        anyhow::ensure!(
            supplied.examples
                == vec![
                    "Company-0 —supplies→ target",
                    "Company-1 —supplies→ target",
                    "Company-2 —supplies→ target",
                ],
            "the first three by when they were recorded: {:?}",
            supplied.examples
        );
        anyhow::ensure!(
            supplied.quotes == vec!["", "q1", "q2"],
            "each quote follows its own example: {:?}",
            supplied.quotes
        );
        anyhow::ensure!(sigs[2].examples == vec!["Company-0 —owns→ x", "Company-1 —owns→ y"]);
        anyhow::ensure!(sigs[2].quotes == vec!["", ""]);
        for _ in 0..3 {
            let again: Vec<_> = phrase_bindings::signatures(&pool, kb)
                .await?
                .iter()
                .map(|s| (s.phrase.clone(), s.subject_type_id, s.object_type_id))
                .collect();
            let first: Vec<_> = sigs
                .iter()
                .map(|s| (s.phrase.clone(), s.subject_type_id, s.object_type_id))
                .collect();
            anyhow::ensure!(again == first, "the order holds from call to call");
        }

        let kinds = type_bindings::signatures(&pool, kb).await?;
        let company = kinds
            .iter()
            .find(|k| k.kind_word == "company")
            .ok_or_else(|| anyhow::anyhow!("no company: {kinds:?}"))?;
        anyhow::ensure!(company.count == 10);
        anyhow::ensure!(
            company.words == vec!["Company", "company", "COMPANY"],
            "the three most frequent spellings: {:?}",
            company.words
        );
        anyhow::ensure!(company.examples == vec!["Company-0", "Company-1", "Company-2"]);
        anyhow::ensure!(
            company.phrases == vec!["supplies", "owns"],
            "phrases by how often a company says them: {:?}",
            company.phrases
        );
        anyhow::ensure!(
            kinds
                .iter()
                .map(|k| k.kind_word.as_str())
                .collect::<Vec<_>>()
                == vec!["company", "place"]
        );
        anyhow::Ok(())
    }
    .await;
    sqlx::query("DELETE FROM organizations WHERE id=$1")
        .bind(org)
        .execute(&pool)
        .await?;
    pool.close().await;
    result
}

async fn entity(
    pool: &PgPool,
    kb: Uuid,
    name: &str,
    type_id: Uuid,
    kind_word: &str,
) -> anyhow::Result<Uuid> {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO entities (id, kb_id, canonical_name, type_id, specific_type)
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(id)
    .bind(kb)
    .bind(name)
    .bind(type_id)
    .bind(kind_word)
    .execute(pool)
    .await?;
    Ok(id)
}

async fn say(
    pool: &PgPool,
    kb: Uuid,
    subject: Uuid,
    phrase: &str,
    object: FactObject<'_>,
) -> anyhow::Result<Uuid> {
    Ok(
        graph::insert_open_statement(pool, kb, subject, phrase, object, None, 1.0)
            .await?
            .0,
    )
}
