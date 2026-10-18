//! A spreadsheet or CSV that hits a parser cap must leave a warning behind;
//! otherwise the document reads as if the file were complete.

use std::fmt::Write as _;
use std::io::{Cursor, Write};

fn ledger_rows(data_rows: usize) -> String {
    let mut sheet_data = String::from(
        r#"<row r="1"><c r="A1" t="inlineStr"><is><t>id</t></is></c><c r="B1" t="inlineStr"><is><t>name</t></is></c></row>"#,
    );
    for id in 1..=data_rows {
        let row = id + 1;
        sheet_data.push_str(&format!(
            r#"<row r="{row}"><c r="A{row}"><v>{id}</v></c><c r="B{row}" t="inlineStr"><is><t>customer-{id}</t></is></c></row>"#
        ));
    }
    sheet_data
}

fn workbook(data_rows: usize) -> Vec<u8> {
    workbook_xml(&ledger_rows(data_rows))
}

fn workbook_xml(sheet_data: &str) -> Vec<u8> {
    workbook_sheets(&[("Ledger", sheet_data)])
}

fn workbook_sheets(sheets: &[(&str, &str)]) -> Vec<u8> {
    workbook_sheets_with_chart(sheets, false, "")
}

fn workbook_sheets_with_chart(
    sheets: &[(&str, &str)],
    chart: bool,
    worksheet_suffix: &str,
) -> Vec<u8> {
    let mut content_types = String::from(
        r#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/>"#,
    );
    let mut sheet_entries = String::new();
    let mut sheet_rels = String::new();
    let mut parts = Vec::new();
    for (i, (name, sheet_data)) in sheets.iter().enumerate() {
        let id = i + 1;
        write!(content_types, r#"<Override PartName="/xl/worksheets/sheet{id}.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/>"#).unwrap();
        write!(
            sheet_entries,
            r#"<sheet name="{name}" sheetId="{id}" r:id="rId{id}"/>"#
        )
        .unwrap();
        write!(sheet_rels, r#"<Relationship Id="rId{id}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet{id}.xml"/>"#).unwrap();
        parts.push((format!("xl/worksheets/sheet{id}.xml"), format!(r#"<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData>{sheet_data}</sheetData>{worksheet_suffix}</worksheet>"#)));
    }
    if chart {
        let id = sheets.len() + 1;
        write!(content_types, r#"<Override PartName="/xl/chartsheets/sheet{id}.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.chartsheet+xml"/><Override PartName="/xl/drawings/drawing1.xml" ContentType="application/vnd.openxmlformats-officedocument.drawing+xml"/><Override PartName="/xl/charts/chart1.xml" ContentType="application/vnd.openxmlformats-officedocument.drawingml.chart+xml"/>"#).unwrap();
        write!(
            sheet_entries,
            r#"<sheet name="Chart" sheetId="{id}" r:id="rId{id}"/>"#
        )
        .unwrap();
        write!(sheet_rels, r#"<Relationship Id="rId{id}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/chartsheet" Target="chartsheets/sheet{id}.xml"/>"#).unwrap();
        parts.extend([
            (format!("xl/chartsheets/sheet{id}.xml"), r#"<chartsheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheetViews><sheetView workbookViewId="0"/></sheetViews><drawing r:id="rId1"/></chartsheet>"#.into()),
            (format!("xl/chartsheets/_rels/sheet{id}.xml.rels"), r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/drawing" Target="../drawings/drawing1.xml"/></Relationships>"#.into()),
            ("xl/drawings/drawing1.xml".into(), r#"<xdr:wsDr xmlns:xdr="http://schemas.openxmlformats.org/drawingml/2006/spreadsheetDrawing" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><xdr:absoluteAnchor><xdr:pos x="0" y="0"/><xdr:ext cx="3600000" cy="2400000"/><xdr:graphicFrame macro=""><xdr:nvGraphicFramePr><xdr:cNvPr id="1" name="Chart 1"/><xdr:cNvGraphicFramePr/></xdr:nvGraphicFramePr><xdr:xfrm><a:off x="0" y="0"/><a:ext cx="0" cy="0"/></xdr:xfrm><a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/chart"><c:chart xmlns:c="http://schemas.openxmlformats.org/drawingml/2006/chart" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" r:id="rId1"/></a:graphicData></a:graphic></xdr:graphicFrame><xdr:clientData/></xdr:absoluteAnchor></xdr:wsDr>"#.into()),
            ("xl/drawings/_rels/drawing1.xml.rels".into(), r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/chart" Target="../charts/chart1.xml"/></Relationships>"#.into()),
            ("xl/charts/chart1.xml".into(), r#"<c:chartSpace xmlns:c="http://schemas.openxmlformats.org/drawingml/2006/chart"><c:chart><c:plotArea><c:layout/><c:pieChart><c:varyColors val="1"/><c:ser><c:idx val="0"/><c:order val="0"/><c:tx><c:v>Series 1</c:v></c:tx><c:cat><c:strLit><c:ptCount val="1"/><c:pt idx="0"><c:v>Item</c:v></c:pt></c:strLit></c:cat><c:val><c:numLit><c:formatCode>General</c:formatCode><c:ptCount val="1"/><c:pt idx="0"><c:v>1</c:v></c:pt></c:numLit></c:val></c:ser></c:pieChart></c:plotArea><c:plotVisOnly val="1"/></c:chart></c:chartSpace>"#.into()),
        ]);
    }
    content_types.push_str("</Types>");
    parts.extend([
        ("[Content_Types].xml".into(), content_types),
        ("_rels/.rels".into(), r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>"#.into()),
        ("xl/workbook.xml".into(), format!(r#"<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets>{sheet_entries}</sheets></workbook>"#)),
        ("xl/_rels/workbook.xml.rels".into(), format!(r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">{sheet_rels}</Relationships>"#)),
    ]);
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (part, xml) in parts {
        zip.start_file(part, zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(xml.as_bytes()).unwrap();
    }
    zip.finish().unwrap().into_inner()
}

fn words(cell: &str, text: &str) -> String {
    format!(r#"<c r="{cell}" t="inlineStr"><is><t>{text}</t></is></c>"#)
}

fn column(mut col: usize) -> String {
    let mut letters = Vec::new();
    loop {
        letters.push(b'A' + (col % 26) as u8);
        if col < 26 {
            break;
        }
        col = col / 26 - 1;
    }
    letters.reverse();
    String::from_utf8(letters).unwrap()
}

fn csv(records: usize) -> String {
    let mut csv = String::from("id,name\n");
    for id in 1..=records {
        writeln!(csv, "{id},customer-{id}").unwrap();
    }
    csv
}

#[test]
fn a_spreadsheet_past_the_row_cap_reports_the_rows_it_left_out() {
    let parsed = utopia_ingest::parse("ledger.xlsx", &workbook(2_000)).unwrap();

    assert!(
        parsed.text.contains("| 1999 | customer-1999 |"),
        "the last row within the cap is still read:\n{}",
        parsed.text
    );
    assert!(
        !parsed.text.contains("customer-2000"),
        "the first row past the cap is not presented as read"
    );
    assert_eq!(parsed.warnings.len(), 1, "{:?}", parsed.warnings);
    let warning = &parsed.warnings[0];
    assert_eq!(warning.kind, "spreadsheet.rows_truncated");
    assert_eq!(
        warning.detail,
        serde_json::json!({
            "sheet": "Ledger",
            "rows_read": 2_000,
            "rows_total": 2_001,
            "rows_omitted": 1,
        })
    );
}

#[test]
fn a_spreadsheet_at_the_row_cap_has_no_warning() {
    let parsed = utopia_ingest::parse("ledger.xlsx", &workbook(1_999)).unwrap();

    assert!(
        parsed.text.contains("| 1999 | customer-1999 |"),
        "the last data row is inside the cap"
    );
    assert!(parsed.warnings.is_empty(), "{:?}", parsed.warnings);
}

#[test]
fn a_distant_cell_past_the_row_window_does_not_shrink_the_retained_ledger() {
    let mut rows = ledger_rows(1_999);
    rows.push_str(&format!(
        r#"<row r="1000000">{}</row>"#,
        words("P1000000", "omitted-distant-note")
    ));
    let parsed = utopia_ingest::parse("ledger.xlsx", &workbook_xml(&rows)).unwrap();
    assert!(parsed.text.contains("| 1999 | customer-1999 |"));
    assert!(!parsed.text.contains("omitted-distant-note"));
    assert_eq!(parsed.warnings.len(), 1);
    assert_eq!(parsed.warnings[0].kind, "spreadsheet.rows_truncated");
    assert_eq!(
        parsed.warnings[0].detail,
        serde_json::json!({
            "sheet": "Ledger", "rows_read": 2000,
            "rows_total": 1000000, "rows_omitted": 998000,
        })
    );
}

#[test]
fn a_header_note_beyond_empty_columns_does_not_truncate_a_ledger() {
    let rows = ledger_rows(1_500);
    let expected = utopia_ingest::parse("ledger.xlsx", &workbook_xml(&rows)).unwrap();
    assert!(expected.text.contains("| 1500 | customer-1500 |"));
    for reference in ["AZZ1", "XFD1"] {
        let with_note = rows.replacen(
            "</row>",
            &format!("{}</row>", words(reference, "header-only-note")),
            1,
        );
        let parsed = utopia_ingest::parse("ledger.xlsx", &workbook_xml(&with_note)).unwrap();
        assert!(
            parsed.text == expected.text,
            "note at {reference} changed the ledger"
        );
        assert!(
            parsed.warnings.is_empty(),
            "{reference}: {:?}",
            parsed.warnings
        );
    }
}

#[test]
fn an_ordinary_dense_sheet_retains_two_thousand_rows_and_one_hundred_fifty_columns() {
    let columns: Vec<_> = (0..150).map(column).collect();
    let mut rows = String::from(r#"<row r="1">"#);
    for (col, name) in columns.iter().enumerate() {
        rows.push_str(&words(&format!("{name}1"), &format!("column-{col}")));
    }
    rows.push_str("</row>");
    for row in 2..=2_000 {
        write!(
            rows,
            r#"<row r="{row}">{}"#,
            words(&format!("A{row}"), &format!("customer-{}", row - 1))
        )
        .unwrap();
        for (col, name) in columns.iter().enumerate().skip(1) {
            write!(
                rows,
                r#"<c r="{name}{row}"><v>{}</v></c>"#,
                row * 1_000 + col
            )
            .unwrap();
        }
        rows.push_str("</row>");
    }
    let parsed = utopia_ingest::parse("ledger.xlsx", &workbook_xml(&rows)).unwrap();
    assert!(parsed.warnings.is_empty(), "{:?}", parsed.warnings);
    let last = parsed
        .text
        .lines()
        .find(|line| line.starts_with("| customer-1999 |"))
        .expect("the final row is retained");
    assert!(last.ends_with("| 2000149 |"), "the last column is retained");
    assert_eq!(last.split('|').count(), 152, "all 150 columns are present");
}

#[test]
fn a_broken_sheet_does_not_prevent_reading_the_other_sheets() {
    let good = ledger_rows(1);
    let expected = utopia_ingest::parse("ledger.xlsx", &workbook_xml(&good)).unwrap();
    let broken = r#"<row r="1"><c r="A1" t="n"><v>not-a-number</v></c></row>"#;
    let bytes = workbook_sheets(&[("Ledger", &good), ("Broken", broken)]);
    let parsed = utopia_ingest::parse("ledger.xlsx", &bytes).unwrap();
    assert_eq!(parsed.text, expected.text);
    assert!(parsed.warnings.is_empty(), "{:?}", parsed.warnings);
}

#[test]
fn unreadable_merges_do_not_prevent_reading_the_worksheet_cells() {
    use calamine::Reader as _;

    let rows = ledger_rows(1);
    let expected = utopia_ingest::parse("ledger.xlsx", &workbook_xml(&rows)).unwrap();
    let merges = r#"<mergeCells count="1"><mergeCell ref="not-a-range"/></mergeCells>"#;
    let bytes = workbook_sheets_with_chart(&[("Ledger", &rows)], false, merges);
    let mut book: calamine::Xlsx<_> =
        calamine::open_workbook_from_rs(Cursor::new(bytes.clone())).unwrap();
    assert_eq!(book.worksheet_range("Ledger").unwrap().height(), 2);
    assert!(book.merge_cells_by_sheet_name("Ledger").is_err());

    let parsed = utopia_ingest::parse("ledger.xlsx", &bytes).unwrap();
    assert_eq!(parsed.text, expected.text);
    assert!(parsed.warnings.is_empty(), "{:?}", parsed.warnings);
}

#[test]
fn a_chartsheet_does_not_prevent_reading_the_worksheets() {
    let rows = ledger_rows(1);
    let expected = utopia_ingest::parse("ledger.xlsx", &workbook_xml(&rows)).unwrap();
    let bytes = workbook_sheets_with_chart(&[("Ledger", &rows)], true, "");
    let parsed = utopia_ingest::parse("ledger.xlsx", &bytes).unwrap();
    assert_eq!(parsed.text, expected.text);
    assert!(parsed.warnings.is_empty(), "{:?}", parsed.warnings);
}

#[test]
fn a_csv_past_the_record_cap_reports_the_records_it_left_out() {
    let parsed = utopia_ingest::parse("customers.csv", csv(10_000).as_bytes()).unwrap();

    assert!(
        parsed.text.contains("| 9999 | customer-9999 |"),
        "the last record within the cap is still read"
    );
    assert!(
        !parsed.text.contains("customer-10000"),
        "the first record past the cap is not presented as read"
    );
    assert_eq!(parsed.warnings.len(), 1, "{:?}", parsed.warnings);
    let warning = &parsed.warnings[0];
    assert_eq!(warning.kind, "csv.records_truncated");
    assert_eq!(
        warning.detail,
        serde_json::json!({
            "records_read": 10_000,
            "records_total": 10_001,
            "records_omitted": 1,
        })
    );
}

#[test]
fn a_csv_at_the_record_cap_has_no_warning() {
    let parsed = utopia_ingest::parse("customers.csv", csv(9_999).as_bytes()).unwrap();

    assert!(
        parsed.text.contains("| 9999 | customer-9999 |"),
        "the last record is inside the cap"
    );
    assert!(parsed.warnings.is_empty(), "{:?}", parsed.warnings);
}
