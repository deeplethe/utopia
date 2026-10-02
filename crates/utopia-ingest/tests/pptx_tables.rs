use std::io::{Cursor, Write};

fn package(parts: &[(String, String)]) -> Vec<u8> {
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (name, xml) in parts {
        zip.start_file(name, zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(xml.as_bytes()).unwrap();
    }
    zip.finish().unwrap().into_inner()
}

fn deck(body: &str) -> Vec<u8> {
    package(&[
        ("[Content_Types].xml".into(), r#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Override PartName="/ppt/presentation.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.presentation.main+xml"/><Override PartName="/ppt/slides/slide1.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.slide+xml"/></Types>"#.into()),
        ("_rels/.rels".into(), r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="ppt/presentation.xml"/></Relationships>"#.into()),
        ("ppt/presentation.xml".into(), r#"<p:presentation xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><p:sldIdLst><p:sldId id="256" r:id="rId2"/></p:sldIdLst></p:presentation>"#.into()),
        ("ppt/_rels/presentation.xml.rels".into(), r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide" Target="slides/slide1.xml"/></Relationships>"#.into()),
        ("ppt/slides/slide1.xml".into(), format!(r#"<p:sld xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><p:cSld><p:spTree>{body}</p:spTree></p:cSld></p:sld>"#)),
    ])
}

fn parse_slide(body: &str) -> String {
    utopia_ingest::parse("deck.pptx", &deck(body)).unwrap().text
}

fn text_box(text: &str) -> String {
    format!(
        r#"<p:sp><p:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:t>{text}</a:t></a:r></a:p></p:txBody></p:sp>"#
    )
}

fn cell(text: &str) -> String {
    cell_with_props(text, "")
}

fn cell_with_props(text: &str, props: &str) -> String {
    format!(
        r#"<a:tc><a:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:t>{text}</a:t></a:r></a:p></a:txBody><a:tcPr {props}/></a:tc>"#
    )
}

fn cell_with_paragraphs(paragraphs: &[&str]) -> String {
    let paragraphs = paragraphs
        .iter()
        .map(|text| format!(r#"<a:p><a:r><a:t>{text}</a:t></a:r></a:p>"#))
        .collect::<String>();
    format!(r#"<a:tc><a:txBody><a:bodyPr/><a:lstStyle/>{paragraphs}</a:txBody><a:tcPr/></a:tc>"#)
}

fn empty_cell(props: &str) -> String {
    format!(r#"<a:tc><a:txBody><a:bodyPr/><a:lstStyle/><a:p/></a:txBody><a:tcPr {props}/></a:tc>"#)
}

fn row(cells: &[String]) -> String {
    format!("<a:tr>{}</a:tr>", cells.concat())
}

fn table(first_row: bool, rows: &[String]) -> String {
    let first_row = if first_row { r#" firstRow="1""# } else { "" };
    format!(
        r#"<p:graphicFrame><a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/table"><a:tbl><a:tblPr{first_row}/><a:tblGrid/><rows/></a:tbl></a:graphicData></a:graphic></p:graphicFrame>"#
    )
    .replace("<rows/>", &rows.concat())
}

fn single_column_text_table(first_row: bool) -> String {
    let first_row = if first_row { r#" firstRow="1""# } else { "" };
    let rows = ["Action", "Approve contract", "Renew license"]
        .iter()
        .map(|text| format!(r#"<a:tr h="370840">{}</a:tr>"#, cell(text)))
        .collect::<String>();
    format!(
        r#"<p:graphicFrame>
<p:nvGraphicFramePr><p:cNvPr id="3" name="Actions"/><p:cNvGraphicFramePr/><p:nvPr/></p:nvGraphicFramePr>
<p:xfrm><a:off x="914400" y="914400"/><a:ext cx="3657600" cy="1112520"/></p:xfrm>
<a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/table">
<a:tbl><a:tblPr{first_row}/><a:tblGrid><a:gridCol w="3657600"/></a:tblGrid>{rows}</a:tbl>
</a:graphicData></a:graphic></p:graphicFrame>"#
    )
}

#[test]
fn a_single_column_text_table_keeps_every_cell_beside_a_slide_title() {
    let title = r#"<p:sp>
<p:nvSpPr><p:cNvPr id="2" name="Title"/><p:cNvSpPr/><p:nvPr><p:ph type="title"/></p:nvPr></p:nvSpPr>
<p:spPr/><p:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:t>Agenda</a:t></a:r></a:p></p:txBody>
</p:sp>"#;
    for first_row in [false, true] {
        let body = format!("{title}{}", single_column_text_table(first_row));
        let parsed = utopia_ingest::parse("deck.pptx", &deck(&body)).unwrap();
        assert!(parsed.text.contains("Agenda"), "{}", parsed.text);
        for cell in ["Action", "Approve contract", "Renew license"] {
            assert!(
                parsed.text.contains(cell),
                "missing {cell:?} (firstRow={first_row}): text={:?}, warnings={:?}",
                parsed.text,
                parsed.warnings
            );
        }
    }
}

#[test]
fn a_slide_with_only_a_single_column_text_table_keeps_every_cell() {
    for first_row in [false, true] {
        let parsed = utopia_ingest::parse("deck.pptx", &deck(&single_column_text_table(first_row)))
            .expect("a valid slide with text cells must be readable");
        for cell in ["Action", "Approve contract", "Renew license"] {
            assert!(
                parsed.text.contains(cell),
                "missing {cell:?}: {}",
                parsed.text
            );
        }
    }
}

/// Two columns where only one cell in each row has words, and a notice in one cell
/// that spans the row: neither is a table with headers and data, and both keep their words.
#[test]
fn a_slide_table_that_is_not_a_grid_of_data_keeps_its_words() {
    for first_row in [false, true] {
        let text = parse_slide(&table(
            first_row,
            &[
                row(&[cell("Action"), empty_cell("")]),
                row(&[cell("Approve contract"), empty_cell("")]),
                row(&[empty_cell(""), cell("Renew license")]),
            ],
        ));
        for words in ["Action", "Approve contract", "Renew license"] {
            assert!(text.contains(words), "missing {words:?}: {text}");
        }
        let text = parse_slide(&table(
            first_row,
            &[row(&[
                cell_with_props("Notice for the board", r#"gridSpan="2""#),
                empty_cell(r#"hMerge="1""#),
            ])],
        ));
        assert!(text.contains("Notice for the board"), "{text}");
    }
}

#[test]
fn a_three_by_four_slide_table_keeps_its_header_and_rows() {
    let text = parse_slide(&table(
        true,
        &[
            row(&[cell("Quarter"), cell("Revenue"), cell("YoY")]),
            row(&[cell("Q1"), cell("1,200"), cell("+12%")]),
            row(&[cell("Q2"), cell("1,350"), cell("+9%")]),
            row(&[cell("Q3"), cell("1,410"), cell("+7%")]),
        ],
    ));

    assert!(
        text.contains(
            "| Quarter | Revenue | YoY |\n| --- | --- | --- |\n| Q1 | 1,200 | +12% |\n| Q2 | 1,350 | +9% |\n| Q3 | 1,410 | +7% |"
        ),
        "{text}"
    );
}

#[test]
fn a_title_and_text_around_a_slide_table_keep_their_lines() {
    let text = parse_slide(&format!(
        "{}{}{}",
        text_box("2024 年各季度营收"),
        table(
            true,
            &[
                row(&[cell("Quarter"), cell("Revenue")]),
                row(&[cell("Q1"), cell("1,200")]),
            ],
        ),
        text_box("来源：财务部"),
    ));

    let title = text.find("2024 年各季度营收").expect(&text);
    let table_start = text.find("| Quarter | Revenue |").expect(&text);
    let source = text.find("来源：财务部").expect(&text);
    assert!(title < table_start && table_start < source, "{text}");
}

#[test]
fn grid_span_and_horizontal_merge_keep_the_grid_shape() {
    let text = parse_slide(&table(
        true,
        &[
            row(&[
                cell_with_props("Summary", r#"gridSpan="2""#),
                cell_with_props("COVERED", r#"hMerge="1""#),
                cell("Total"),
            ]),
            row(&[cell("Q1"), cell("1,200"), cell("1,350")]),
        ],
    ));

    assert!(
        text.contains("|  | Summary | Total |\n| --- | --- | --- |\n| Q1 | 1,200 | 1,350 |"),
        "{text}"
    );
    assert!(!text.contains("COVERED"), "{text}");
}

#[test]
fn a_vertical_merge_continuation_stays_an_empty_cell() {
    let text = parse_slide(&table(
        true,
        &[
            row(&[cell("Quarter"), cell("Revenue"), cell("YoY")]),
            row(&[cell("Q1"), cell("1,200"), cell("+12%")]),
            row(&[cell("Q2"), cell("1,350"), empty_cell(r#"vMerge="1""#)]),
            row(&[cell("Q3"), cell("1,410"), cell("+7%")]),
        ],
    ));

    assert!(
        text.contains(
            "| Quarter | Revenue | YoY |\n| --- | --- | --- |\n| Q1 | 1,200 | +12% |\n| Q2 | 1,350 |  |\n| Q3 | 1,410 | +7% |"
        ),
        "{text}"
    );
}

#[test]
fn first_row_marks_the_first_row_as_a_header() {
    let text = parse_slide(&table(
        true,
        &[
            row(&[cell("2024"), cell("2025")]),
            row(&[cell("Revenue"), cell("1,200")]),
        ],
    ));

    assert!(
        text.contains("| 2024 | 2025 |\n| --- | --- |\n| Revenue | 1,200 |"),
        "{text}"
    );
}

#[test]
fn without_first_row_rows_are_classified_by_their_content() {
    let text = parse_slide(&table(
        false,
        &[
            row(&[cell("2024"), cell("2025")]),
            row(&[cell("Revenue"), cell("1,200")]),
        ],
    ));

    assert!(
        text.contains("|  |  |\n| --- | --- |\n| 2024 | 2025 |\n| Revenue | 1,200 |"),
        "{text}"
    );
}

#[test]
fn a_slide_can_hold_two_tables_without_joining_them() {
    let text = parse_slide(&format!(
        "{}{}{}",
        table(
            true,
            &[row(&[cell("A"), cell("B")]), row(&[cell("1"), cell("2")]),],
        ),
        text_box("Between"),
        table(
            true,
            &[row(&[cell("C"), cell("D")]), row(&[cell("3"), cell("4")]),],
        ),
    ));

    assert_eq!(text.matches("| --- | --- |").count(), 2, "{text}");
    let first = text.find("| A | B |").expect(&text);
    let between = text.find("Between").expect(&text);
    let second = text.find("| C | D |").expect(&text);
    assert!(first < between && between < second, "{text}");
}

#[test]
fn paragraphs_in_a_cell_are_joined_by_spaces() {
    let text = parse_slide(&table(
        true,
        &[
            row(&[cell("Region"), cell("Sales")]),
            row(&[cell_with_paragraphs(&["North", "America"]), cell("1,200")]),
        ],
    ));

    assert!(
        text.contains("| Region | Sales |\n| --- | --- |\n| North America | 1,200 |"),
        "{text}"
    );
}
