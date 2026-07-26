use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};

pub(crate) fn append(
    path: &Path,
    left_context: &str,
    input: &str,
    okuri: Option<&str>,
    text: &str,
) -> Result<()> {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("システム時刻がUnix epochより前です")?
        .as_millis();
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("変換履歴を開けません: {}", path.display()))?;

    writeln!(
        file,
        "{timestamp},{},{},{},{}",
        csv_field(left_context),
        csv_field(input),
        csv_field(okuri.unwrap_or_default()),
        csv_field(text),
    )
    .with_context(|| format!("変換履歴へ書き込めません: {}", path.display()))
}

fn csv_field(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\"\""))
}

#[cfg(test)]
mod tests {
    use super::csv_field;

    #[test]
    fn csv_field_preserves_machine_readable_boundaries() {
        assert_eq!(csv_field("a,\"b\"\nc"), "\"a,\"\"b\"\"\nc\"");
    }
}
