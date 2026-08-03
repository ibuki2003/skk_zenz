use std::path::Path;

use anyhow::{Context, Result, bail};

#[derive(Debug)]
pub struct HistoryEntry {
    #[allow(dead_code)]
    pub timestamp_ms: u64,
    pub context: String,
    pub input: String,
    pub okuri: String,
    pub text: String,
}

pub fn read_history(path: &Path) -> Result<Vec<HistoryEntry>> {
    let contents =
        std::fs::read_to_string(path).with_context(|| format!("読めません: {}", path.display()))?;
    let records = split_records(&contents);
    let mut entries = Vec::new();
    for (i, record) in records.iter().enumerate() {
        let entry =
            parse_record(record).with_context(|| format!("レコード{}のパースに失敗", i + 1))?;
        entries.push(entry);
    }
    Ok(entries)
}

/// Splits the file into CSV records, respecting quoted fields that may contain newlines.
fn split_records(contents: &str) -> Vec<&str> {
    let mut records = Vec::new();
    let mut pos = 0;
    let bytes = contents.as_bytes();
    while pos < bytes.len() {
        let start = pos;
        let mut in_quote = false;
        while pos < bytes.len() {
            match bytes[pos] {
                b'"' => in_quote = !in_quote,
                b'\n' if !in_quote => {
                    break;
                }
                _ => {}
            }
            pos += 1;
        }
        let line = &contents[start..pos];
        if !line.is_empty() && line != "\r" {
            records.push(line.trim_end_matches('\r'));
        }
        pos += 1; // skip \n
    }
    records
}

fn parse_record(record: &str) -> Result<HistoryEntry> {
    let mut fields = Vec::new();
    let mut rest = record;

    // timestamp (unquoted)
    let comma = rest.find(',').context("フィールド不足")?;
    fields.push(rest[..comma].to_owned());
    rest = &rest[comma + 1..];

    // 4 quoted CSV fields
    for _ in 0..4 {
        if !rest.starts_with('"') {
            bail!("引用符で始まっていません");
        }
        rest = &rest[1..];
        let mut value = String::new();
        loop {
            if let Some(pos) = rest.find('"') {
                value.push_str(&rest[..pos]);
                rest = &rest[pos + 1..];
                if rest.starts_with('"') {
                    value.push('"');
                    rest = &rest[1..];
                } else {
                    if rest.starts_with(',') {
                        rest = &rest[1..];
                    }
                    break;
                }
            } else {
                bail!("閉じ引用符がありません");
            }
        }
        // フィールド内の改行は最後の行だけ残す
        let value = match value.rfind('\n') {
            Some(pos) => value[pos + 1..].to_owned(),
            None => value,
        };
        fields.push(value);
    }

    if fields.len() < 5 {
        bail!("フィールド不足: {} 個", fields.len());
    }

    Ok(HistoryEntry {
        timestamp_ms: fields[0].parse().context("timestamp のパース失敗")?,
        context: fields[1].clone(),
        input: fields[2].clone(),
        okuri: fields[3].clone(),
        text: fields[4].clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_simple_line() {
        let entry = parse_record(r#"1000,"ctx","inp","","txt""#).unwrap();
        assert_eq!(entry.timestamp_ms, 1000);
        assert_eq!(entry.context, "ctx");
        assert_eq!(entry.input, "inp");
        assert_eq!(entry.okuri, "");
        assert_eq!(entry.text, "txt");
    }

    #[test]
    fn handles_escaped_quotes() {
        let entry = parse_record(r#"1000,"a""b","inp","","txt""#).unwrap();
        assert_eq!(entry.context, "a\"b");
    }

    #[test]
    fn newline_in_field_keeps_last_line() {
        let entry = parse_record("1000,\"first\nsecond\",\"inp\",\"\",\"txt\"").unwrap();
        assert_eq!(entry.context, "second");
    }

    #[test]
    fn split_records_handles_multiline_fields() {
        let input = "1000,\"a\nb\",\"x\",\"\",\"y\"\n2000,\"c\",\"d\",\"\",\"e\"\n";
        let records = split_records(input);
        assert_eq!(records.len(), 2);
        assert_eq!(records[0], "1000,\"a\nb\",\"x\",\"\",\"y\"");
        assert_eq!(records[1], "2000,\"c\",\"d\",\"\",\"e\"");
    }
}
