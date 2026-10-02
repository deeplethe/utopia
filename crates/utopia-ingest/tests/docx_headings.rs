//! Word 文档的标题读成标题。
//!
//! 照中文 Word 写出来的样子拼一份：样式 id 是 `1`、`2`（正文是 `a`），样式名是英文的
//! `heading 1`，大纲级别写在样式里。一份里两份周报各有自己的日期——同一篇里分节，
//! 是 0064 按节取日期要认的形状。

use std::io::{Cursor, Write};

const STYLES: &str = r#"<w:styles xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:style w:type="paragraph" w:default="1" w:styleId="a"><w:name w:val="Normal"/><w:qFormat/></w:style><w:style w:type="paragraph" w:styleId="1"><w:name w:val="heading 1"/><w:basedOn w:val="a"/><w:next w:val="a"/><w:link w:val="10"/><w:uiPriority w:val="9"/><w:qFormat/><w:pPr><w:keepNext/><w:keepLines/><w:spacing w:before="340" w:after="330" w:line="578" w:lineRule="auto"/><w:outlineLvl w:val="0"/></w:pPr><w:rPr><w:b/><w:bCs/><w:kern w:val="44"/><w:sz w:val="44"/><w:szCs w:val="44"/></w:rPr></w:style><w:style w:type="paragraph" w:styleId="2"><w:name w:val="heading 2"/><w:basedOn w:val="a"/><w:next w:val="a"/><w:link w:val="20"/><w:uiPriority w:val="9"/><w:unhideWhenUsed/><w:qFormat/><w:pPr><w:keepNext/><w:keepLines/><w:spacing w:before="260" w:after="260" w:line="416" w:lineRule="auto"/><w:outlineLvl w:val="1"/></w:pPr></w:style><w:style w:type="paragraph" w:styleId="a3"><w:name w:val="Title"/><w:basedOn w:val="a"/><w:next w:val="a"/><w:qFormat/><w:pPr><w:jc w:val="center"/></w:pPr></w:style><w:style w:type="character" w:styleId="10"><w:name w:val="标题 1 字符"/><w:basedOn w:val="a0"/><w:link w:val="1"/></w:style></w:styles>"#;

fn read(body: &str, styles: Option<&str>) -> String {
    let mut parts = vec![
        ("[Content_Types].xml", r#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/><Override PartName="/word/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml"/></Types>"#.to_string()),
        ("_rels/.rels", r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>"#.to_string()),
        ("word/_rels/document.xml.rels", r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/></Relationships>"#.to_string()),
        ("word/document.xml", format!(r#"<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body>{body}<w:sectPr/></w:body></w:document>"#)),
    ];
    if let Some(styles) = styles {
        parts.push(("word/styles.xml", styles.to_string()));
    }
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (part, xml) in parts {
        zip.start_file(part, zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(xml.as_bytes()).unwrap();
    }
    utopia_ingest::parse("周报.docx", &zip.finish().unwrap().into_inner())
        .unwrap()
        .text
}

fn styled(style: &str, runs: &str) -> String {
    format!(r#"<w:p><w:pPr><w:pStyle w:val="{style}"/></w:pPr>{runs}</w:p>"#)
}

fn plain(text: &str) -> String {
    format!("<w:p><w:r><w:t>{text}</w:t></w:r></w:p>")
}

fn run(text: &str) -> String {
    format!("<w:r><w:t>{text}</w:t></w:r>")
}

fn reports() -> String {
    [
        styled("a3", &run("周报汇编")),
        styled("1", &run("第一周周报（2024-03-01）")),
        plain("上周销售额增长 5%。"),
        // 标题里换了行：标题还是一行
        styled(
            "1",
            &format!("{}<w:r><w:br/><w:t>（2024-03-08）</w:t></w:r>", run("第二周周报")),
        ),
        styled("2", &run("明细")),
        plain("上周退货 3 单。"),
        // 段落自己写的大纲级别，不靠样式
        r#"<w:p><w:pPr><w:outlineLvl w:val="0"/></w:pPr><w:r><w:t>附录</w:t></w:r></w:p>"#.into(),
        // 借了标题样式、又把大纲级别改回正文的一段
        r#"<w:p><w:pPr><w:pStyle w:val="1"/><w:outlineLvl w:val="9"/></w:pPr><w:r><w:t>借了标题样式的正文</w:t></w:r></w:p>"#.into(),
        // 修订：原来是标题，改成了正文；w:pPrChange 里记的是改之前的样式
        r#"<w:p><w:pPr><w:pPrChange w:id="1" w:author="A" w:date="2024-03-09T00:00:00Z"><w:pPr><w:pStyle w:val="1"/></w:pPr></w:pPrChange></w:pPr><w:r><w:t>改回正文的一段</w:t></w:r></w:p>"#.into(),
        format!(
            "<w:tbl><w:tr><w:tc>{}</w:tc><w:tc>{}</w:tc></w:tr><w:tr><w:tc>{}</w:tc><w:tc>{}</w:tc></w:tr></w:tbl>",
            plain("项目"),
            plain("数量"),
            styled("1", &run("格子里的标题样式")),
            plain("3")
        ),
    ]
    .concat()
}

#[test]
fn a_paragraph_with_an_outline_level_is_a_heading() {
    let text = read(&reports(), Some(STYLES));
    assert!(
        text.contains("# 第一周周报（2024-03-01）\n\n上周销售额增长 5%。\n"),
        "{text}"
    );
    assert!(
        text.contains("# 第二周周报 （2024-03-08）\n\n## 明细\n\n上周退货 3 单。\n"),
        "{text}"
    );
    assert!(text.contains("# 附录\n"), "{text}");
    for body in ["周报汇编", "借了标题样式的正文", "改回正文的一段"] {
        assert!(text.lines().any(|line| line == body), "{body}: {text}");
    }
    assert!(text.contains("| 格子里的标题样式 | 3 |"), "{text}");
}

#[test]
fn each_report_is_its_own_section_in_the_chunks() {
    let text = read(&reports(), Some(STYLES));
    let pieces = utopia_ingest::chunk_with_budget(&text, 40);
    let chunk = |sentence: &str| {
        pieces
            .iter()
            .find(|p| p.text.contains(sentence))
            .map(|p| p.text[..p.text.find(sentence).unwrap()].to_string())
            .unwrap_or_else(|| panic!("{sentence} is in no chunk: {pieces:#?}"))
    };
    // 分节看的是块里这句话前面的标题行（`headings_at` 读的就是它们）
    let first = chunk("上周销售额增长");
    assert!(first.contains("# 第一周周报（2024-03-01）"), "{first}");
    let second = chunk("上周退货");
    assert!(
        second.contains("# 第二周周报 （2024-03-08）") && second.contains("## 明细"),
        "{second}"
    );
    assert!(!second.contains("第一周周报"), "{second}");
}

#[test]
fn without_a_style_sheet_only_a_paragraphs_own_outline_level_counts() {
    let text = read(&reports(), None);
    assert!(text.contains("# 附录\n"), "{text}");
    assert!(!text.contains("# 第一周周报"), "{text}");
}
