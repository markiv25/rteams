//! xlsx / csv table extraction (calamine for workbooks, a small parser for csv).

use std::io::Cursor;

use calamine::{open_workbook_auto_from_rs, Data, Reader};

use super::{DocPreview, Limits, Sheet};

pub fn extract_workbook(bytes: &[u8], limits: &Limits) -> Result<DocPreview, String> {
    let mut wb =
        open_workbook_auto_from_rs(Cursor::new(bytes.to_vec())).map_err(|e| e.to_string())?;
    let names: Vec<String> = wb.sheet_names().to_vec();
    let mut sheets = Vec::new();
    for name in names.into_iter().take(limits.max_sheets) {
        let range = wb.worksheet_range(&name).map_err(|e| e.to_string())?;
        let (h, w) = range.get_size();
        let mut rows = Vec::new();
        for row in range.rows().take(limits.max_rows) {
            rows.push(
                row.iter()
                    .take(limits.max_cols)
                    .map(|c| match c {
                        Data::Empty => String::new(),
                        other => other.to_string(),
                    })
                    .collect(),
            );
        }
        sheets.push(Sheet {
            name,
            rows,
            truncated: h > limits.max_rows || w > limits.max_cols,
        });
    }
    Ok(DocPreview::Sheets(sheets))
}

/// Parse csv text (RFC 4180 quoting) into rows.
pub fn parse_csv(text: &str) -> Vec<Vec<String>> {
    let mut rows = Vec::new();
    let mut row = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let mut chars = text.chars().peekable();
    let mut any = false;
    while let Some(c) = chars.next() {
        any = true;
        if quoted {
            if c == '"' {
                if chars.peek() == Some(&'"') {
                    chars.next();
                    field.push('"');
                } else {
                    quoted = false;
                }
            } else {
                field.push(c);
            }
            continue;
        }
        match c {
            '"' => quoted = true,
            ',' => row.push(std::mem::take(&mut field)),
            '\r' => {}
            '\n' => {
                row.push(std::mem::take(&mut field));
                rows.push(std::mem::take(&mut row));
                any = false;
            }
            _ => field.push(c),
        }
    }
    if any {
        row.push(field);
        rows.push(row);
    }
    rows
}

pub fn extract_csv(text: &str, limits: &Limits) -> DocPreview {
    let mut rows = parse_csv(text);
    let truncated = rows.len() > limits.max_rows || rows.iter().any(|r| r.len() > limits.max_cols);
    rows.truncate(limits.max_rows);
    for r in &mut rows {
        r.truncate(limits.max_cols);
    }
    DocPreview::Sheets(vec![Sheet {
        name: "csv".into(),
        rows,
        truncated,
    }])
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    #[test]
    fn csv_quotes() {
        let r = parse_csv("a,b\n\"x,1\",\"he said \"\"hi\"\"\"\r\nlast,");
        assert_eq!(r[0], vec!["a", "b"]);
        assert_eq!(r[1], vec!["x,1", "he said \"hi\""]);
        assert_eq!(r[2], vec!["last", ""]);
    }

    #[test]
    fn csv_limits() {
        let l = Limits {
            max_rows: 2,
            max_cols: 1,
            ..Limits::default()
        };
        match extract_csv("a,b\nc,d\ne,f\n", &l) {
            DocPreview::Sheets(s) => {
                assert_eq!(s[0].rows, vec![vec!["a"], vec!["c"]]);
                assert!(s[0].truncated);
            }
            _ => panic!(),
        }
    }

    fn tiny_xlsx() -> Vec<u8> {
        let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let o = zip::write::SimpleFileOptions::default();
        let files = [
            (
                "[Content_Types].xml",
                r#"<?xml version="1.0"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/><Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/></Types>"#,
            ),
            (
                "_rels/.rels",
                r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>"#,
            ),
            (
                "xl/workbook.xml",
                r#"<?xml version="1.0"?><workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets><sheet name="Data" sheetId="1" r:id="rId1"/></sheets></workbook>"#,
            ),
            (
                "xl/_rels/workbook.xml.rels",
                r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/></Relationships>"#,
            ),
            (
                "xl/worksheets/sheet1.xml",
                r#"<?xml version="1.0"?><worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData><row r="1"><c r="A1" t="inlineStr"><is><t>name</t></is></c><c r="B1" t="inlineStr"><is><t>qty</t></is></c></row><row r="2"><c r="A2" t="inlineStr"><is><t>apple</t></is></c><c r="B2"><v>3</v></c></row></sheetData></worksheet>"#,
            ),
        ];
        for (n, b) in files {
            w.start_file(n, o).unwrap();
            w.write_all(b.as_bytes()).unwrap();
        }
        w.finish().unwrap().into_inner()
    }

    #[test]
    fn xlsx_grid() {
        match extract_workbook(&tiny_xlsx(), &Limits::default()).unwrap() {
            DocPreview::Sheets(s) => {
                assert_eq!(s[0].name, "Data");
                assert_eq!(s[0].rows[0], vec!["name", "qty"]);
                assert_eq!(s[0].rows[1], vec!["apple", "3"]);
            }
            _ => panic!(),
        }
    }
}
