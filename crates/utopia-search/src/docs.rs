//! 内置文档（Charter）的内存索引：启动时从打包 markdown 建，进程生命周期内只读。
//! 协议中立——chat 的 search_docs 工具臂与将来的 MCP 工具面共用这一个入口。

use anyhow::Context;
use tantivy::collector::TopDocs;
use tantivy::query::{BooleanQuery, Occur, Query, TermQuery};
use tantivy::schema::{
    Field, IndexRecordOption, Schema, TextFieldIndexing, TextOptions, Value, STORED,
};
use tantivy::tokenizer::TextAnalyzer;
use tantivy::{Index, IndexReader, TantivyDocument, Term};

const JIEBA: &str = "jieba";

/// 一节文档（按 h2 切分；anchor 与前端 Docs 页的标题锚点同法生成）。
#[derive(Debug, Clone)]
pub struct DocsSection {
    pub slug: String,
    pub title: String,
    pub heading: String,
    pub anchor: String,
    pub body: String,
}

pub struct DocsIndex {
    reader: IndexReader,
    analyzer: TextAnalyzer,
    f_slug: Field,
    f_title: Field,
    f_heading: Field,
    f_anchor: Field,
    f_body: Field,
    f_text: Field,
}

impl DocsIndex {
    pub fn build(sections: &[DocsSection]) -> anyhow::Result<Self> {
        let mut schema_builder = Schema::builder();
        let text_indexing = TextFieldIndexing::default()
            .set_tokenizer(JIEBA)
            .set_index_option(IndexRecordOption::WithFreqsAndPositions);
        let f_slug = schema_builder.add_text_field("slug", STORED);
        let f_title = schema_builder.add_text_field("title", STORED);
        let f_heading = schema_builder.add_text_field("heading", STORED);
        let f_anchor = schema_builder.add_text_field("anchor", STORED);
        let f_body = schema_builder.add_text_field("body", STORED);
        // 检索域 = 节标题 + 正文（标题词权重靠重复出现自然获得）
        let f_text = schema_builder.add_text_field(
            "text",
            TextOptions::default().set_indexing_options(text_indexing),
        );
        let schema = schema_builder.build();

        let index = Index::create_in_ram(schema);
        index
            .tokenizers()
            .register(JIEBA, tantivy_jieba::JiebaTokenizer::new());

        let mut writer = index.writer(16 * 1024 * 1024)?;
        for s in sections {
            let mut doc = TantivyDocument::default();
            doc.add_text(f_slug, &s.slug);
            doc.add_text(f_title, &s.title);
            doc.add_text(f_heading, &s.heading);
            doc.add_text(f_anchor, &s.anchor);
            doc.add_text(f_body, &s.body);
            doc.add_text(f_text, format!("{}\n{}", s.heading, s.body));
            writer.add_document(doc)?;
        }
        writer.commit()?;

        let reader = index.reader()?;
        let analyzer = index
            .tokenizers()
            .get(JIEBA)
            .context("jieba tokenizer 未注册")?;
        Ok(Self {
            reader,
            analyzer,
            f_slug,
            f_title,
            f_heading,
            f_anchor,
            f_body,
            f_text,
        })
    }

    /// BM25 检索。切词方式与 SearchIndex 一致（手工 OR 组合，避开 CJK 短语查询陷阱）。
    pub fn search(&self, query: &str, limit: usize) -> anyhow::Result<Vec<DocsSection>> {
        let mut analyzer = self.analyzer.clone();
        let mut stream = analyzer.token_stream(query);
        let mut term_queries: Vec<(Occur, Box<dyn Query>)> = Vec::new();
        while stream.advance() {
            let word = stream.token().text.trim().to_string();
            if word.is_empty() || word.chars().all(|c| !c.is_alphanumeric()) {
                continue;
            }
            term_queries.push((
                Occur::Should,
                Box::new(TermQuery::new(
                    Term::from_field_text(self.f_text, &word),
                    IndexRecordOption::WithFreqs,
                )),
            ));
        }
        if term_queries.is_empty() {
            return Ok(Vec::new());
        }

        let searcher = self.reader.searcher();
        let top = searcher.search(
            &BooleanQuery::new(term_queries),
            &TopDocs::with_limit(limit).order_by_score(),
        )?;
        let get = |doc: &TantivyDocument, f: Field| {
            doc.get_first(f)
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string()
        };
        let mut hits = Vec::with_capacity(top.len());
        for (_score, addr) in top {
            let doc: TantivyDocument = searcher.doc(addr)?;
            hits.push(DocsSection {
                slug: get(&doc, self.f_slug),
                title: get(&doc, self.f_title),
                heading: get(&doc, self.f_heading),
                anchor: get(&doc, self.f_anchor),
                body: get(&doc, self.f_body),
            });
        }
        Ok(hits)
    }
}
