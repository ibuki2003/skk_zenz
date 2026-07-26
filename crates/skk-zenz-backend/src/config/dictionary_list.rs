use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use super::paths::{data_home, resolve_path};

const DICTIONARY_LIST: &str = "dictionary_list";

pub(super) fn load_dictionary_paths(config_dir: &Path) -> Result<Vec<PathBuf>> {
    let primary = config_dir.join(DICTIONARY_LIST);
    if primary.exists() {
        let contents = fs::read_to_string(&primary)
            .with_context(|| format!("辞書リストを読めません: {}", primary.display()))?;
        return Ok(parse_plain_dictionary_list(&contents, &primary));
    }

    let fallback = data_home()?.join("fcitx5/skk").join(DICTIONARY_LIST);
    let contents = fs::read_to_string(&fallback).with_context(|| {
        format!(
            "辞書リストがありません: {} または {}",
            primary.display(),
            fallback.display()
        )
    })?;
    Ok(parse_fcitx_dictionary_list(&contents))
}

pub(super) fn parse_plain_dictionary_list(contents: &str, path: &Path) -> Vec<PathBuf> {
    let base = path.parent().unwrap_or_else(|| Path::new(""));
    contents
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(PathBuf::from)
        .map(|entry| resolve_path(base, entry))
        .collect()
}

pub(super) fn parse_fcitx_dictionary_list(contents: &str) -> Vec<PathBuf> {
    contents
        .lines()
        .filter_map(|line| {
            let fields: Vec<_> = line
                .split(',')
                .filter_map(|field| field.trim().split_once('='))
                .map(|(key, value)| (key.trim(), value.trim()))
                .collect();
            let readonly = fields
                .iter()
                .any(|(key, value)| *key == "mode" && *value == "readonly");
            let file_type = fields
                .iter()
                .any(|(key, value)| *key == "type" && *value == "file");
            (readonly && file_type)
                .then(|| {
                    fields
                        .iter()
                        .find(|(key, _)| *key == "file")
                        .map(|(_, value)| PathBuf::from(value))
                })
                .flatten()
        })
        .collect()
}
