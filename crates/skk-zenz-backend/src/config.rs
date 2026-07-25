use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use hf_hub::api::sync::ApiBuilder;
use serde::Deserialize;

use crate::zenz::ZenzConfig;

const CONFIG_FILE: &str = "config.toml";
const DICTIONARY_LIST: &str = "dictionary_list";
const USER_DICTIONARY: &str = "user.sqlite3";

#[derive(Clone, Debug)]
pub struct BackendConfig {
    pub zenz: ZenzConfig,
    pub system_dictionaries: Vec<PathBuf>,
    pub user_dictionary: PathBuf,
    pub rom_kana_rule: String,
    pub rule_search_paths: Vec<PathBuf>,
}

#[derive(Debug, Deserialize)]
struct ConfigFile {
    model_path: Option<PathBuf>,
    #[serde(default = "default_huggingface_model")]
    huggingface: Option<String>,
    #[serde(default = "default_rom_kana_rule")]
    rom_kana_rule: String,
    #[serde(default = "default_context_size")]
    context_size: u32,
    threads: Option<i32>,
    #[serde(default = "default_gpu_layers")]
    gpu_layers: u32,
}

impl BackendConfig {
    pub fn from_xdg() -> Result<Self> {
        Self::from_toml_file(Self::xdg_config_file()?)
    }

    pub fn xdg_config_file() -> Result<PathBuf> {
        Ok(config_dir()?.join(CONFIG_FILE))
    }

    pub fn from_toml_file(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let contents = fs::read_to_string(path)
            .with_context(|| format!("設定ファイルを読めません: {}", path.display()))?;
        let file: ConfigFile = toml::from_str(&contents)
            .with_context(|| format!("設定ファイルを解釈できません: {}", path.display()))?;
        let base = path.parent().unwrap_or_else(|| Path::new(""));
        let config_dir = config_dir()?;
        let model_path = match file.model_path {
            Some(path) => resolve_path(base, path),
            None => download_huggingface_model(
                file.huggingface
                    .as_deref()
                    .context("model_path または huggingface を指定してください")?,
            )?,
        };
        let mut zenz = ZenzConfig::new(model_path);
        zenz.context_size = file.context_size;
        zenz.threads = file.threads;
        zenz.gpu_layers = file.gpu_layers;
        Ok(Self {
            zenz,
            system_dictionaries: load_dictionary_paths(&config_dir)?,
            user_dictionary: config_dir.join(USER_DICTIONARY),
            rom_kana_rule: file.rom_kana_rule,
            rule_search_paths: rule_search_paths(&config_dir)?,
        })
    }
}

fn download_huggingface_model(reference: &str) -> Result<PathBuf> {
    let (repo, filename) = parse_huggingface_reference(reference)?;
    let api = ApiBuilder::new()
        .with_progress(true)
        .build()
        .context("Hugging Face API の初期化に失敗しました")?;
    api.model(repo.clone())
        .get(filename)
        .with_context(|| format!("{repo}/{filename} の取得に失敗しました"))
}

fn parse_huggingface_reference(reference: &str) -> Result<(String, &str)> {
    let mut fields = reference.splitn(3, '/');
    let owner = fields.next().unwrap_or_default();
    let repository = fields.next().unwrap_or_default();
    let filename = fields.next().unwrap_or_default();
    if owner.is_empty() || repository.is_empty() || filename.is_empty() {
        anyhow::bail!("huggingface は owner/repository/path 形式で指定してください: {reference}");
    }
    Ok((format!("{owner}/{repository}"), filename))
}

fn config_dir() -> Result<PathBuf> {
    if let Some(path) = env::var_os("XDG_CONFIG_HOME") {
        return Ok(PathBuf::from(path).join("skk-zenz"));
    }
    Ok(home_dir()?.join(".config/skk-zenz"))
}

fn data_home() -> Result<PathBuf> {
    if let Some(path) = env::var_os("XDG_DATA_HOME") {
        return Ok(PathBuf::from(path));
    }
    Ok(home_dir()?.join(".local/share"))
}

fn home_dir() -> Result<PathBuf> {
    env::var_os("HOME")
        .map(PathBuf::from)
        .context("HOMEが設定されていません")
}

fn rule_search_paths(config_dir: &Path) -> Result<Vec<PathBuf>> {
    let libskk_config = config_dir
        .parent()
        .context("設定ディレクトリの親がありません")?
        .join("libskk/rules");
    let mut paths = vec![config_dir.join("rules"), libskk_config];
    paths.extend(env!("SKK_RULES_PATH").split(':').map(PathBuf::from));
    Ok(paths)
}

fn load_dictionary_paths(config_dir: &Path) -> Result<Vec<PathBuf>> {
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

fn parse_plain_dictionary_list(contents: &str, path: &Path) -> Vec<PathBuf> {
    let base = path.parent().unwrap_or_else(|| Path::new(""));
    contents
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(PathBuf::from)
        .map(|entry| resolve_path(base, entry))
        .collect()
}

fn parse_fcitx_dictionary_list(contents: &str) -> Vec<PathBuf> {
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

fn resolve_path(base: &Path, path: PathBuf) -> PathBuf {
    if path.is_absolute() {
        path
    } else {
        base.join(path)
    }
}

const fn default_context_size() -> u32 {
    1024
}

const fn default_gpu_layers() -> u32 {
    999
}

fn default_rom_kana_rule() -> String {
    "default".to_owned()
}

fn default_huggingface_model() -> Option<String> {
    Some("Miwa-Keita/zenz-v3.2-small-gguf/ggml-model-Q5_K_M.gguf".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_config_defaults() {
        let file: ConfigFile = toml::from_str(
            r#"
            model_path = "models/model.gguf"
            "#,
        )
        .unwrap();
        assert_eq!(file.model_path, Some(PathBuf::from("models/model.gguf")));
        assert_eq!(file.huggingface, default_huggingface_model());
        assert_eq!(file.context_size, 1024);
        assert_eq!(file.gpu_layers, 999);
        assert_eq!(file.rom_kana_rule, "default");
    }

    #[test]
    fn parses_huggingface_reference() {
        assert_eq!(
            parse_huggingface_reference("Miwa-Keita/zenz-v3.2-small-gguf/ggml-model-Q5_K_M.gguf")
                .unwrap(),
            (
                "Miwa-Keita/zenz-v3.2-small-gguf".to_owned(),
                "ggml-model-Q5_K_M.gguf"
            )
        );
        assert_eq!(
            parse_huggingface_reference("owner/repository/models/model.gguf").unwrap(),
            ("owner/repository".to_owned(), "models/model.gguf")
        );
    }

    #[test]
    fn rejects_invalid_huggingface_reference() {
        assert!(parse_huggingface_reference("repository/model.gguf").is_err());
        assert!(parse_huggingface_reference("owner/repository/").is_err());
    }

    #[test]
    fn parses_plain_dictionary_paths_relative_to_list() {
        assert_eq!(
            parse_plain_dictionary_list(
                "# comment\nSKK-JISYO.L\n/opt/dict/SKK-JISYO.geo\n\n",
                Path::new("/config/dictionary_list")
            ),
            [
                PathBuf::from("/config/SKK-JISYO.L"),
                PathBuf::from("/opt/dict/SKK-JISYO.geo")
            ]
        );
    }

    #[test]
    fn fallback_uses_only_readonly_file_entries() {
        let contents = "\
encoding=UTF-8,file=$FCITX_CONFIG_DIR/skk/user.dict,mode=readwrite,type=file
file=/dict/SKK-JISYO.L,mode=readonly,type=file
file=/dict/server,mode=readonly,type=skkserv
";
        assert_eq!(
            parse_fcitx_dictionary_list(contents),
            [PathBuf::from("/dict/SKK-JISYO.L")]
        );
    }
}
