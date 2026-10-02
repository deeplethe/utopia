//! 电子表格里的合并区域投影到它覆盖的所有格，和 HTML 的 colspan/rowspan 一样。

use std::io::{Cursor, Write};

fn workbook(sheet_data: &str, merges: &[&str]) -> Vec<u8> {
    let merge_cells = (!merges.is_empty()).then(|| {
        let cells = merges
            .iter()
            .map(|range| format!(r#"<mergeCell ref="{range}"/>"#))
            .collect::<String>();
        format!(
            r#"<mergeCells count="{}">{cells}</mergeCells>"#,
            merges.len()
        )
    });
    let worksheet = format!(
        r#"<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData>{sheet_data}</sheetData>{}</worksheet>"#,
        merge_cells.unwrap_or_default()
    );
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (part, xml) in [
        ("[Content_Types].xml", r#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/><Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/></Types>"#.to_string()),
        ("_rels/.rels", r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>"#.to_string()),
        ("xl/workbook.xml", r#"<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets><sheet name="Ledger" sheetId="1" r:id="rId1"/></sheets></workbook>"#.to_string()),
        ("xl/_rels/workbook.xml.rels", r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/></Relationships>"#.to_string()),
        ("xl/worksheets/sheet1.xml", worksheet),
    ] {
        zip.start_file(part, zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(xml.as_bytes()).unwrap();
    }
    zip.finish().unwrap().into_inner()
}

fn words(cell: &str, text: &str) -> String {
    format!(r#"<c r="{cell}" t="inlineStr"><is><t>{text}</t></is></c>"#)
}

fn number(cell: &str, value: i32) -> String {
    format!(r#"<c r="{cell}"><v>{value}</v></c>"#)
}

fn formula(cell: &str, expression: &str, value: i32) -> String {
    format!(r#"<c r="{cell}"><f>{expression}</f><v>{value}</v></c>"#)
}

fn read(sheet_data: &str, merges: &[&str]) -> String {
    utopia_ingest::parse("ledger.xlsx", &workbook(sheet_data, merges))
        .unwrap()
        .text
}

#[test]
fn a_two_row_merged_header_stacks_each_column() {
    let sheet_data = [
        format!(
            "<row r=\"1\">{}{}{}</row>",
            words("A1", "区域"),
            words("B1", "2024年"),
            words("F1", "2025年")
        ),
        format!(
            "<row r=\"2\">{}{}{}{}{}{}{}{}{}</row>",
            words("B2", "Q1"),
            words("C2", "Q2"),
            words("D2", "Q3"),
            words("E2", "Q4"),
            words("F2", "Q1"),
            words("G2", "Q2"),
            words("H2", "Q3"),
            words("I2", "Q4"),
            words("A2", "")
        ),
        format!(
            "<row r=\"3\">{}{}{}{}{}{}{}{}{}</row>",
            words("A3", "华东"),
            number("B3", 120),
            number("C3", 135),
            number("D3", 141),
            number("E3", 150),
            number("F3", 160),
            number("G3", 171),
            number("H3", 180),
            number("I3", 190)
        ),
    ]
    .concat();
    let text = read(&sheet_data, &["A1:A2", "B1:E1", "F1:I1"]);
    assert!(
        text.contains("| 区域 | 2024年 Q1 | 2024年 Q2 | 2024年 Q3 | 2024年 Q4 | 2025年 Q1 | 2025年 Q2 | 2025年 Q3 | 2025年 Q4 |"),
        "{text}"
    );
    assert!(
        text.contains("| 华东 | 120 | 135 | 141 | 150 | 160 | 171 | 180 | 190 |"),
        "{text}"
    );
}

#[test]
fn a_vertical_merge_carries_its_anchor_down_every_covered_row() {
    let sheet_data = [
        format!(
            "<row r=\"1\">{}{}{}</row>",
            words("A1", "大区 城市"),
            words("B1", "门店数"),
            words("C1", "营收（万元）")
        ),
        format!(
            "<row r=\"2\">{}{}{}</row>",
            words("A2", "华东"),
            number("B2", 12),
            number("C2", 860)
        ),
        format!(
            "<row r=\"3\">{}{}{}</row>",
            words("A3", "上海"),
            number("B3", 8),
            number("C3", 540)
        ),
        format!(
            "<row r=\"4\">{}{}{}</row>",
            words("A4", "杭州"),
            number("B4", 6),
            number("C4", 410)
        ),
    ]
    .concat();
    let text = read(&sheet_data, &["A2:A4"]);
    assert!(
        text.contains("| 大区 城市 | 门店数 | 营收（万元） |"),
        "{text}"
    );
    assert!(text.contains("| 华东 | 12 | 860 |"), "{text}");
    assert!(text.contains("| 华东 | 8 | 540 |"), "{text}");
    assert!(text.contains("| 华东 | 6 | 410 |"), "{text}");
}

#[test]
fn a_merge_uses_the_top_left_value_not_a_stale_covered_cell() {
    let sheet_data = [
        format!(
            "<row r=\"1\">{}{}{}</row>",
            words("A1", "项目"),
            words("B1", "金额"),
            words("C1", "备注")
        ),
        format!(
            "<row r=\"2\">{}{}{}</row>",
            words("A2", "合计"),
            number("B2", 7),
            words("C2", "旧值")
        ),
    ]
    .concat();
    let text = read(&sheet_data, &["B2:C2"]);
    assert!(text.contains("| 合计 | 7 |"), "{text}");
    assert!(!text.contains("旧值"), "{text}");
}

#[test]
fn a_formula_value_is_projected_and_an_empty_merge_does_not_leak_stale_cells() {
    let sheet_data = [
        format!(
            "<row r=\"1\">{}{}{}</row>",
            words("A1", "项目"),
            words("B1", "金额"),
            words("C1", "备注")
        ),
        format!(
            "<row r=\"2\">{}{}</row>",
            words("A2", "公式"),
            formula("B2", "SUM(B4:B4)", 7)
        ),
        format!(
            "<row r=\"3\">{}{}{}</row>",
            words("A3", "普通"),
            number("B3", 99),
            words("C3", "旧值")
        ),
    ]
    .concat();
    let text = read(&sheet_data, &["B2:B3", "C2:C3"]);
    assert!(text.contains("| 公式 | 7 |"), "{text}");
    assert!(text.contains("| 普通 | 7 |"), "{text}");
    assert!(!text.contains("99"), "{text}");
    assert!(!text.contains("旧值"), "{text}");
}

#[test]
fn a_merge_past_the_used_range_is_clipped_without_moving_later_columns() {
    let sheet_data = [
        format!("<row r=\"1\">{}</row>", words("A1", "标题")),
        format!(
            "<row r=\"2\">{}{}{}</row>",
            words("A2", "项目"),
            words("B2", "数量"),
            words("C2", "备注")
        ),
        format!(
            "<row r=\"3\">{}{}{}</row>",
            words("A3", "合计"),
            number("B3", 3),
            number("C3", 4)
        ),
    ]
    .concat();
    let text = read(&sheet_data, &["A1:Z1"]);
    assert!(text.contains("\n标题:\n"), "{text}");
    assert!(text.contains("| 项目 | 数量 | 备注 |"), "{text}");
    assert!(text.contains("| 合计 | 3 | 4 |"), "{text}");
}

#[test]
fn non_merged_cells_keep_their_order() {
    let sheet_data = [
        format!(
            "<row r=\"1\">{}{}{}</row>",
            words("A1", "项目"),
            words("B1", "年份"),
            words("C1", "金额")
        ),
        format!(
            "<row r=\"2\">{}{}{}</row>",
            words("A2", "甲"),
            number("B2", 2024),
            number("C2", 1)
        ),
        format!(
            "<row r=\"3\">{}{}{}</row>",
            words("A3", "乙"),
            number("B3", 2025),
            number("C3", 2)
        ),
    ]
    .concat();
    let text = read(&sheet_data, &[]);
    assert!(text.contains("| 项目 | 年份 | 金额 |"), "{text}");
    assert!(text.contains("| 甲 | 2024 | 1 |"), "{text}");
    assert!(text.contains("| 乙 | 2025 | 2 |"), "{text}");
}
