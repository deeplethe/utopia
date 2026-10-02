use std::io::{Cursor, Write};

fn read(body: &str) -> String {
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (part, xml) in [
        ("[Content_Types].xml", r#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/></Types>"#.to_string()),
        ("_rels/.rels", r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>"#.to_string()),
        ("word/document.xml", format!(r#"<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body>{body}</w:body></w:document>"#)),
    ] {
        zip.start_file(part, zip::write::SimpleFileOptions::default()).unwrap();
        zip.write_all(xml.as_bytes()).unwrap();
    }
    utopia_ingest::parse("breaks.docx", &zip.finish().unwrap().into_inner())
        .unwrap()
        .text
}

fn table(run: &str) -> String {
    format!(
        r#"<w:tbl><w:tr><w:tc><w:p><w:r><w:t>Item</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>Readings</w:t></w:r></w:p></w:tc></w:tr>
    <w:tr><w:tc><w:p><w:r><w:t>Sample</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r>{run}</w:r></w:p></w:tc></w:tr></w:tbl>"#
    )
}

#[test]
fn explicit_cell_breaks_separate_numbers_and_words() {
    for br in [
        "<w:br/>",
        "<w:cr/>",
        "<w:br></w:br>",
        "<w:cr></w:cr>",
        "<w:br/><w:br/>",
    ] {
        for (left, right) in [("10", "20"), ("hello", "world"), ("甲", "乙")] {
            let text = read(&table(&format!("<w:t>{left}</w:t>{br}<w:t>{right}</w:t>")));
            assert!(
                text.contains(&format!("| Sample | {left} {right} |")),
                "{br}: {text}"
            );
            assert_eq!(text.lines().filter(|line| line.starts_with('|')).count(), 3);
        }
    }
}

#[test]
fn paragraph_ends_are_blank_lines_but_explicit_breaks_are_newlines() {
    for br in ["<w:br/>", "<w:cr/>", "<w:br></w:br>", "<w:cr></w:cr>"] {
        let text = read(&format!(
            "<w:p><w:r><w:t>Hel</w:t></w:r><w:r><w:t>lo</w:t>{br}<w:t>world</w:t></w:r></w:p>"
        ));
        assert_eq!(
            text.trim(),
            "Hello\nworld",
            "a break inside one paragraph: {br}"
        );
    }
    let text = read(&table(
        "<w:t>Hel</w:t></w:r><w:r><w:t>lo</w:t><w:tab/><w:t>world</w:t>",
    ));
    assert!(text.contains("| Sample | Hello world |"), "{text}");
    let text = read("<w:p><w:r><w:t>first</w:t><w:tab/><w:t>line</w:t></w:r></w:p><w:p><w:r><w:t>second</w:t></w:r></w:p>");
    assert_eq!(text.trim(), "first line\n\nsecond");
}

#[test]
fn empty_word_paragraphs_do_not_make_empty_blocks_or_blank_runs() {
    let text = read(concat!(
        "<w:p><w:r><w:t>first</w:t></w:r></w:p>",
        "<w:p/>",
        "<w:p><w:r><w:t> </w:t></w:r></w:p>",
        "<w:p><w:r><w:t>second</w:t></w:r></w:p>",
    ));
    assert_eq!(text.trim(), "first\n\nsecond");
    assert!(!text.contains("\n\n\n"), "{text}");
}

#[test]
fn a_long_word_list_packs_multiple_items_per_chunk() {
    let body: String = (1..=30)
        .map(|i| format!("<w:p><w:r><w:t>List item {i} is short.</w:t></w:r></w:p>"))
        .collect();
    let text = read(&body);
    assert_eq!(text.matches("List item").count(), 30, "{text}");

    let pieces = utopia_ingest::chunk_with_budget(&text, 40);
    assert!(pieces.len() < 30, "{pieces:#?}");
    assert!(
        pieces
            .iter()
            .any(|piece| piece.text.matches("List item").count() > 1),
        "{pieces:#?}"
    );
}

fn long_table(rows: usize) -> String {
    let mut table = String::from(
        r#"<w:tbl><w:tr><w:tc><w:p><w:r><w:t>区域</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>2024 年营收</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>单位</w:t></w:r></w:p></w:tc></w:tr>"#,
    );
    for row in 1..=rows {
        table.push_str(&format!(
            "<w:tr><w:tc><w:p><w:r><w:t>区域 {row}</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>{row}00</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>万元</w:t></w:r></w:p></w:tc></w:tr>"
        ));
    }
    table.push_str("</w:tbl>");
    table
}

#[test]
fn a_word_caption_is_repeated_for_every_piece_of_a_split_table() {
    let analysis = "本段分析说明营业收入同比变化、客户结构和主要风险，不是表格的说明句。".repeat(4);
    let caption = "表1 2024 年分区域营收（单位：万元）";
    let text = read(&format!(
        "<w:p><w:r><w:t>{analysis}</w:t></w:r></w:p><w:p><w:r><w:t>{caption}</w:t></w:r></w:p>{}",
        long_table(40)
    ));
    assert!(
        text.contains(&format!("{analysis}\n\n{caption}\n\n|")),
        "{text}"
    );

    let pieces = utopia_ingest::chunk_text(&text);
    let table_pieces: Vec<_> = pieces
        .iter()
        .filter(|piece| piece.text.contains("| --- |"))
        .collect();
    assert!(table_pieces.len() >= 2, "{pieces:#?}");
    for piece in table_pieces {
        assert!(piece.text.contains(caption), "{:#?}", piece.text);
        assert!(!piece.text.contains(&analysis), "{:#?}", piece.text);
    }
}

#[test]
fn a_non_breaking_hyphen_keeps_the_numbers_on_either_side_apart() {
    let nbh = "<w:noBreakHyphen/>";
    let text = read(&format!(
        r#"<w:p><w:r><w:t xml:space="preserve">合同于 2024</w:t>{nbh}<w:t>01</w:t>{nbh}<w:t xml:space="preserve">15 签订，电话 010</w:t></w:r><w:r>{nbh}</w:r><w:r><w:t>62345678。</w:t></w:r></w:p>"#
    ));
    assert_eq!(text.trim(), "合同于 2024-01-15 签订，电话 010-62345678。");
    let text = read(&table(&format!(
        "<w:t>2024</w:t>{nbh}<w:t>01</w:t>{nbh}<w:t>15</w:t>"
    )));
    assert!(text.contains("| Sample | 2024-01-15 |"), "{text}");
}

/// A table with one column of words, or with words in only one cell of each row, is not a
/// table with headers and data. Its words are still read, one row to a paragraph.
#[test]
fn a_word_table_that_is_not_a_grid_of_data_keeps_its_words() {
    let cell = |text: &str| format!("<w:tc><w:p><w:r><w:t>{text}</w:t></w:r></w:p></w:tc>");
    let one_column: String = ["Action", "Approve contract", "Renew license"]
        .iter()
        .map(|text| format!("<w:tr>{}</w:tr>", cell(text)))
        .collect();
    let text = read(&format!(
        "<w:p><w:r><w:t>Before.</w:t></w:r></w:p><w:tbl>{one_column}</w:tbl><w:p><w:r><w:t>After.</w:t></w:r></w:p>"
    ));
    assert_eq!(
        text.trim(),
        "Before.\n\nAction\n\nApprove contract\n\nRenew license\n\nAfter."
    );
    let left_only: String = ["Action", "Approve contract"]
        .iter()
        .map(|text| format!("<w:tr>{}{}</w:tr>", cell(text), cell("")))
        .collect();
    let text = read(&format!("<w:tbl>{left_only}</w:tbl>"));
    assert_eq!(text.trim(), "Action\n\nApprove contract");
}
