//! viz::parse: find ```viz fenced JSON blocks in message text and validate them.

use std::collections::HashSet;
use std::ops::Range;

use serde_json::Value;

use super::spec::*;

/// One fenced block: byte range of the whole fence (opening line through closing fence) and the
/// parse outcome.
#[derive(Debug, Clone, PartialEq)]
pub struct Block {
    pub range: Range<usize>,
    pub spec: Result<VizSpec, String>,
}

/// Every closed ```viz fence in `text`, valid or not.
pub fn extract_all(text: &str) -> Vec<Block> {
    let mut out = Vec::new();
    let mut pos = 0;
    let mut open: Option<(usize, usize)> = None; // (fence start, body start)
    for line in text.split_inclusive('\n') {
        let start = pos;
        pos += line.len();
        let t = line.trim();
        match open {
            None => {
                if let Some(info) = t.strip_prefix("```") {
                    if info.trim().eq_ignore_ascii_case("viz") {
                        open = Some((start, pos));
                    }
                }
            }
            Some((fs, bs)) => {
                if t == "```" {
                    let body = &text[bs..start];
                    let end = start + line.trim_end_matches(['\n', '\r']).len();
                    out.push(Block {
                        range: fs..end,
                        spec: parse(body),
                    });
                    open = None;
                }
            }
        }
    }
    out
}

/// Valid specs only, with the fence range, for the messages view.
pub fn extract(text: &str) -> Vec<(Range<usize>, VizSpec)> {
    extract_all(text)
        .into_iter()
        .filter_map(|b| b.spec.ok().map(|s| (b.range, s)))
        .collect()
}

fn cell(v: &Value) -> String {
    match v {
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// Parse and validate one JSON spec.
pub fn parse(json: &str) -> Result<VizSpec, String> {
    let v: Value = serde_json::from_str(json.trim()).map_err(|e| format!("invalid JSON: {e}"))?;
    let obj = v.as_object().ok_or("spec must be a JSON object")?;
    let title = obj
        .get("title")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from);
    let kind = obj
        .get("type")
        .and_then(Value::as_str)
        .ok_or("missing \"type\"")?;
    match kind {
        "table" => {
            let cols = obj
                .get("columns")
                .and_then(Value::as_array)
                .ok_or("table needs \"columns\"")?;
            if cols.is_empty() || cols.len() > MAX_COLUMNS {
                return Err(format!("table needs 1-{MAX_COLUMNS} columns"));
            }
            let columns: Vec<String> = cols.iter().map(cell).collect();
            let rows_v = obj
                .get("rows")
                .and_then(Value::as_array)
                .ok_or("table needs \"rows\"")?;
            if rows_v.len() > MAX_ROWS {
                return Err(format!("table has more than {MAX_ROWS} rows"));
            }
            let mut rows = Vec::new();
            for (i, r) in rows_v.iter().enumerate() {
                let r = r
                    .as_array()
                    .ok_or_else(|| format!("row {} is not an array", i + 1))?;
                if r.len() != columns.len() {
                    return Err(format!(
                        "row {} has {} cells, expected {}",
                        i + 1,
                        r.len(),
                        columns.len()
                    ));
                }
                rows.push(r.iter().map(cell).collect());
            }
            Ok(VizSpec::Table(TableSpec {
                title,
                columns,
                rows,
            }))
        }
        "bar" | "line" | "pie" => {
            let kind = match kind {
                "bar" => ChartKind::Bar,
                "line" => ChartKind::Line,
                _ => ChartKind::Pie,
            };
            let labels: Vec<String> = obj
                .get("labels")
                .and_then(Value::as_array)
                .ok_or("chart needs \"labels\"")?
                .iter()
                .map(cell)
                .collect();
            if labels.is_empty() || labels.len() > MAX_LABELS {
                return Err(format!("chart needs 1-{MAX_LABELS} labels"));
            }
            let ser = obj
                .get("series")
                .and_then(Value::as_array)
                .ok_or("chart needs \"series\"")?;
            if ser.is_empty() || ser.len() > MAX_SERIES {
                return Err(format!("chart needs 1-{MAX_SERIES} series"));
            }
            if kind == ChartKind::Pie && ser.len() != 1 {
                return Err("pie charts take exactly one series".into());
            }
            let mut series = Vec::new();
            let mut names = HashSet::new();
            for (i, s) in ser.iter().enumerate() {
                let name = s
                    .get("name")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|n| !n.is_empty())
                    .ok_or_else(|| format!("series {} needs a name", i + 1))?
                    .to_string();
                if !names.insert(name.clone()) {
                    return Err(format!("duplicate series name \"{name}\""));
                }
                let vals = s
                    .get("values")
                    .and_then(Value::as_array)
                    .ok_or_else(|| format!("series \"{name}\" needs values"))?;
                if vals.len() != labels.len() {
                    return Err(format!(
                        "series \"{name}\" has {} values for {} labels",
                        vals.len(),
                        labels.len()
                    ));
                }
                let mut values = Vec::new();
                for x in vals {
                    let n = x
                        .as_f64()
                        .filter(|n| n.is_finite())
                        .ok_or_else(|| format!("series \"{name}\" has a non-numeric value"))?;
                    if kind == ChartKind::Pie && n < 0.0 {
                        return Err("pie values must not be negative".into());
                    }
                    values.push(n);
                }
                if kind == ChartKind::Pie && values.iter().sum::<f64>() <= 0.0 {
                    return Err("pie values must sum to more than zero".into());
                }
                series.push(Series { name, values });
            }
            Ok(VizSpec::Chart(ChartSpec {
                kind,
                title,
                labels,
                series,
            }))
        }
        other => Err(format!("unknown type \"{other}\"")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BAR: &str =
        r#"{"type":"bar","title":"T","labels":["a","b"],"series":[{"name":"s","values":[1,2.5]}]}"#;

    #[test]
    fn parses_valid_specs() {
        let VizSpec::Chart(c) = parse(BAR).unwrap() else {
            panic!()
        };
        assert_eq!(c.kind, ChartKind::Bar);
        assert_eq!(c.series[0].values, vec![1.0, 2.5]);
        let t =
            parse(r#"{"type":"table","columns":["a","b"],"rows":[[1,"x"],[null,true]]}"#).unwrap();
        let VizSpec::Table(t) = t else { panic!() };
        assert_eq!(t.rows[1], vec!["", "true"]);
    }

    #[test]
    fn rejects_invalid() {
        for bad in [
            "nope",
            "[]",
            r#"{"type":"radar"}"#,
            r#"{"type":"bar","labels":["a"],"series":[{"name":"s","values":[1,2]}]}"#,
            r#"{"type":"bar","labels":["a"],"series":[{"name":"s","values":[1]},{"name":"s","values":[2]}]}"#,
            r#"{"type":"bar","labels":["a"],"series":[{"name":"s","values":["x"]}]}"#,
            r#"{"type":"pie","labels":["a"],"series":[{"name":"s","values":[-1]}]}"#,
            r#"{"type":"pie","labels":["a"],"series":[{"name":"s","values":[0]}]}"#,
            r#"{"type":"table","columns":["a"],"rows":[[1,2]]}"#,
            r#"{"type":"bar","labels":[],"series":[]}"#,
        ] {
            assert!(parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn extracts_fences_with_ranges() {
        let text = format!("before\n```viz\n{BAR}\n```\nafter\n```viz\n{{bad\n```\n```rust\nx\n```\n```viz\nunclosed");
        let all = extract_all(&text);
        assert_eq!(all.len(), 2);
        assert!(all[0].spec.is_ok());
        assert!(all[1].spec.is_err());
        let r = all[0].range.clone();
        assert!(text[r.clone()].starts_with("```viz") && text[r].ends_with("```"));
        assert_eq!(extract(&text).len(), 1);
        assert!(extract("no fences").is_empty());
    }

    #[test]
    fn crlf_fences() {
        let text = format!("```viz\r\n{BAR}\r\n```\r\n");
        assert_eq!(extract(&text).len(), 1);
    }
}
