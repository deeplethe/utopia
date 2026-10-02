//! A spreadsheet or CSV that hits a parser cap must leave a warning behind;
//! otherwise the document reads as if the file were complete.

use std::fmt::Write as _;
use std::io::{Cursor, Write};

fn workbook(data_rows: usize) -> Vec<u8> {
    let mut sheet_data = String::from(
        r#"<row r="1"><c r="A1" t="inlineStr"><is><t>id</t></is></c><c r="B1" t="inlineStr"><is><t>name</t></is></c></row>"#,
    );
    for id in 1..=data_rows {
        let row = id + 1;
        sheet_data.push_str(&format!(
            r#"<row r="{row}"><c r="A{row}"><v>{id}</v></c><c r="B{row}" t="inlineStr"><is><t>customer-{id}</t></is></c></row>"#
        ));
    }
    let worksheet = format!(
        r#"<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData>{sheet_data}</sheetData></worksheet>"#
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
