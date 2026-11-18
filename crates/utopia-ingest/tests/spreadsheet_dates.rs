//! 电子表格里的日期格读成它显示的日期，不是 Excel 的序数。
//!
//! 照 Excel 写出来的样子拼一份 xlsx：日期存成从 1899-12-30 起的天数（`<v>45306</v>`），
//! 小数部分是一天里的时刻；它是日期，只因为 styles.xml 里挂的数字格式是日期格式——内置的
//! 14（短日期）、22（日期加时刻）、20（时刻）、46（累计时长），和中文 Excel 自定义的
//! `yyyy"年"m"月"d"日"`。

use std::io::{Cursor, Write};

// cellXfs by index: 0 General, 1 short date (14), 2 a Chinese custom date (176),
// 3 date and time (22), 4 time of day (20), 5 elapsed time (46), 6 "#,##0.00" (4).
const STYLES: &str = r#"<styleSheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><numFmts count="1"><numFmt numFmtId="176" formatCode="yyyy&quot;年&quot;m&quot;月&quot;d&quot;日&quot;"/></numFmts><cellXfs count="7"><xf numFmtId="0" fontId="0" fillId="0" borderId="0" xfId="0"/><xf numFmtId="14" fontId="0" fillId="0" borderId="0" xfId="0" applyNumberFormat="1"/><xf numFmtId="176" fontId="0" fillId="0" borderId="0" xfId="0" applyNumberFormat="1"/><xf numFmtId="22" fontId="0" fillId="0" borderId="0" xfId="0" applyNumberFormat="1"/><xf numFmtId="20" fontId="0" fillId="0" borderId="0" xfId="0" applyNumberFormat="1"/><xf numFmtId="46" fontId="0" fillId="0" borderId="0" xfId="0" applyNumberFormat="1"/><xf numFmtId="4" fontId="0" fillId="0" borderId="0" xfId="0" applyNumberFormat="1"/></cellXfs></styleSheet>"#;

fn workbook(rows: &[Vec<String>], date1904: bool, styles: &str) -> Vec<u8> {
    let sheet_data: String = rows
        .iter()
        .enumerate()
        .map(|(i, cells)| format!(r#"<row r="{}">{}</row>"#, i + 1, cells.concat()))
        .collect();
    let workbook_pr = if date1904 {
        r#"<workbookPr date1904="1"/>"#
    } else {
        "<workbookPr/>"
    };
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (part, xml) in [
        ("[Content_Types].xml", r#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/><Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/><Override PartName="/xl/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.styles+xml"/></Types>"#.to_string()),
        ("_rels/.rels", r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>"#.to_string()),
        ("xl/workbook.xml", format!(r#"<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">{workbook_pr}<sheets><sheet name="Ledger" sheetId="1" r:id="rId1"/></sheets></workbook>"#)),
        ("xl/_rels/workbook.xml.rels", r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/></Relationships>"#.to_string()),
        ("xl/styles.xml", styles.to_string()),
        ("xl/worksheets/sheet1.xml", format!(r#"<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData>{sheet_data}</sheetData></worksheet>"#)),
    ] {
        zip.start_file(part, zip::write::SimpleFileOptions::default()).unwrap();
        zip.write_all(xml.as_bytes()).unwrap();
    }
    zip.finish().unwrap().into_inner()
}

fn read(rows: &[Vec<String>], date1904: bool) -> String {
    utopia_ingest::parse("ledger.xlsx", &workbook(rows, date1904, STYLES))
        .unwrap()
        .text
}

fn words(cell: &str, text: &str) -> String {
    format!(r#"<c r="{cell}" t="inlineStr"><is><t>{text}</t></is></c>"#)
}

fn number(cell: &str, style: u8, value: &str) -> String {
    format!(r#"<c r="{cell}" s="{style}"><v>{value}</v></c>"#)
}

fn header(row: u32) -> Vec<String> {
    vec![
        words(&format!("A{row}"), "Item"),
        words(&format!("B{row}"), "When"),
        words(&format!("C{row}"), "Amount"),
    ]
}

#[test]
fn a_date_cell_reads_as_the_date_it_shows() {
    let text = read(
        &[
            header(1),
            vec![
                words("A2", "Short date"),
                number("B2", 1, "45306"),
                number("C2", 6, "1200"),
            ],
            vec![
                words("A3", "Chinese date"),
                number("B3", 2, "45307"),
                number("C3", 6, "800"),
            ],
            // 没挂日期格式的数就是数，哪怕它碰巧落在日期的范围里
            vec![
                words("A4", "Plain number"),
                number("B4", 0, "45306"),
                number("C4", 6, "5"),
            ],
        ],
        false,
    );
    assert!(
        text.contains("| Short date | 2024-01-15 | 1200 |"),
        "{text}"
    );
    assert!(
        text.contains("| Chinese date | 2024-01-16 | 800 |"),
        "{text}"
    );
    assert!(text.contains("| Plain number | 45306 | 5 |"), "{text}");
}

#[test]
fn a_time_of_day_and_an_elapsed_time_read_as_clock_time() {
    let text = read(
        &[
            header(1),
            vec![
                words("A2", "Opened"),
                number("B2", 3, "45306.395833333336"),
                number("C2", 6, "1"),
            ],
            vec![
                words("A3", "Daily call"),
                number("B3", 4, "0.39583333333333331"),
                number("C3", 6, "2"),
            ],
            vec![
                words("A4", "Downtime"),
                number("B4", 5, "1.5104166666666667"),
                number("C4", 6, "3"),
            ],
        ],
        false,
    );
    assert!(text.contains("| Opened | 2024-01-15 09:30 | 1 |"), "{text}");
    assert!(text.contains("| Daily call | 09:30 | 2 |"), "{text}");
    assert!(text.contains("| Downtime | 36:15:00 | 3 |"), "{text}");
}

#[test]
fn a_workbook_counting_from_1904_reads_the_same_date() {
    // Excel for Mac 从前默认 1904 纪年：同一天存成少 1462 的数
    let text = read(
        &[
            header(1),
            vec![
                words("A2", "Short date"),
                number("B2", 1, "43844"),
                number("C2", 6, "1200"),
            ],
        ],
        true,
    );
    assert!(
        text.contains("| Short date | 2024-01-15 | 1200 |"),
        "{text}"
    );
}

fn read_format(format_id: u16, value: &str, date1904: bool, format_code: Option<&str>) -> String {
    let custom = format_code.map_or_else(String::new, |code| {
        let code = quick_xml::escape::escape(code);
        format!(
            r#"<numFmts count="1"><numFmt numFmtId="{format_id}" formatCode="{code}"/></numFmts>"#
        )
    });
    let styles = format!(
        r#"<styleSheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">{custom}<cellXfs count="1"><xf numFmtId="{format_id}" applyNumberFormat="1"/></cellXfs></styleSheet>"#
    );
    let rows = [
        header(1),
        vec![
            words("A2", "Value"),
            number("B2", 0, value),
            words("C2", "ok"),
        ],
    ];
    utopia_ingest::parse("ledger.xlsx", &workbook(&rows, date1904, &styles))
        .unwrap()
        .text
}

#[test]
fn east_asian_builtin_dates_need_no_custom_format_definition() {
    for format_id in [27, 31, 50, 58] {
        for (date1904, serial) in [(false, "45306"), (true, "43844")] {
            let text = read_format(format_id, serial, date1904, None);
            assert!(
                text.contains("| Value | 2024-01-15 | ok |"),
                "format {format_id}: {text}"
            );
        }
    }
}

#[test]
fn east_asian_fractional_clock_times_and_explicit_formats_are_preserved() {
    // calamine does not retain clock-only metadata; a serial >= 1 still gains a date.
    for format_id in [32, 33] {
        let text = read_format(format_id, "0.5", false, None);
        assert!(
            text.contains("| Value | 12:00 | ok |"),
            "format {format_id}: {text}"
        );
    }
    for (format_id, code, serial, expected) in [
        (27, "0.00", "45306", "45306"),
        (32, "yyyy-mm-dd", "45306", "2024-01-15"),
        (27, "[h]:mm:ss", "1.5104166666666667", "36:15:00"),
    ] {
        let text = read_format(format_id, serial, false, Some(code));
        assert!(
            text.contains(&format!("| Value | {expected} | ok |")),
            "{text}"
        );
    }
}

#[test]
fn east_asian_builtin_formats_are_normalized_in_xls() {
    // Fixture from calamine PR #734, commit d93c5a463399a5554d287ebda0ed1d2a43377ec2,
    // tests/date_east_asian.xls (MIT; license alongside). Rows 0..19 are IDs 27..36/50..58;
    // each row stores 45306, 0.5, 45306.123456789 with no explicit format definition.
    let text = utopia_ingest::parse(
        "ledger.xls",
        include_bytes!("fixtures/calamine/date_east_asian.xls"),
    )
    .unwrap()
    .text;
    let rows: Vec<_> = text
        .lines()
        .filter(|line| line.starts_with("| ") && !line.starts_with("| ---"))
        .collect();
    assert_eq!(rows.len(), 28, "{text}");
    for row in [0, 4, 10, 18] {
        // formats 27, 31, 50, 58
        assert!(rows[row].starts_with("| 2024-01-15 |"), "row {row}: {text}");
    }
    for row in [5, 6] {
        // formats 32, 33
        assert_eq!(
            rows[row].split('|').nth(2).unwrap().trim(),
            "12:00",
            "{text}"
        );
    }
    assert!(rows[19].starts_with("| 45306 | 0.5 |"), "{text}"); // General
    assert!(rows[24].starts_with("| 2024-01-15 |"), "{text}"); // Chinese custom date
}
