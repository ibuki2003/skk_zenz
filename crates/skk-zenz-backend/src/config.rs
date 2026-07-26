use std::collections::HashSet;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;

use crate::zenz::ZenzConfig;

const CONFIG_FILE: &str = "config.toml";
const DICTIONARY_LIST: &str = "dictionary_list";
const USER_DICTIONARY: &str = "user.sqlite3";

#[derive(Clone, Debug)]
pub struct BackendConfig {
    pub config_path: PathBuf,
    pub zenz: ZenzConfig,
    pub system_dictionaries: Vec<PathBuf>,
    pub user_dictionary: PathBuf,
    pub recency_half_life_days: f64,
    pub candidate_order: CandidateOrderConfig,
    pub rom_kana_rule: String,
    pub rule_search_paths: Vec<PathBuf>,
    pub fcitx5: Fcitx5Config,
}

#[derive(Clone, Copy, Debug, Deserialize, Hash, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CandidateSortKey {
    FromUser,
    Recency,
    AiScore,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CandidateOrderConfig {
    #[serde(default = "default_sort_order_ai")]
    pub okuri_ari: Vec<CandidateSortKey>,
    #[serde(default = "default_sort_order_recency")]
    pub okuri_nasi: Vec<CandidateSortKey>,
    #[serde(default = "default_sort_order_recency")]
    pub abbrev: Vec<CandidateSortKey>,
}

impl Default for CandidateOrderConfig {
    fn default() -> Self {
        Self {
            okuri_ari: default_sort_order_ai(),
            okuri_nasi: default_sort_order_recency(),
            abbrev: default_sort_order_recency(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct Fcitx5Config {
    pub page_size: usize,
    pub candidate_window_threshold: usize,
    pub show_annotations: bool,
    pub candidate_key: String,
}

impl Default for Fcitx5Config {
    fn default() -> Self {
        Self {
            page_size: 9,
            candidate_window_threshold: 3,
            show_annotations: true,
            candidate_key: "digit".to_owned(),
        }
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct FrontendConfig {
    fcitx5: Fcitx5Config,
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
    #[serde(default = "default_recency_half_life_days")]
    recency_half_life_days: f64,
    #[serde(default)]
    candidate_order: CandidateOrderConfig,
    #[serde(default)]
    frontend: FrontendConfig,
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
        if !file.recency_half_life_days.is_finite()
            || file.recency_half_life_days <= 0.0
            || !(file.recency_half_life_days * 86_400.0).is_finite()
        {
            anyhow::bail!("recency_half_life_days は正の有限値にしてください");
        }
        file.candidate_order.validate()?;
        if !(1..=9).contains(&file.frontend.fcitx5.page_size) {
            anyhow::bail!("frontend.fcitx5.page_size は1以上9以下にしてください");
        }
        if file.frontend.fcitx5.candidate_key != "digit" {
            anyhow::bail!("frontend.fcitx5.candidate_key は現在 digit のみ指定できます");
        }
        let base = path.parent().unwrap_or_else(|| Path::new(""));
        let config_dir = config_dir()?;
        let mut zenz = match file.model_path {
            Some(path) => ZenzConfig::new(resolve_path(base, path)),
            None => {
                let reference = file
                    .huggingface
                    .context("model_path または huggingface を指定してください")?;
                parse_huggingface_reference(&reference)?;
                ZenzConfig::from_huggingface(reference)
            }
        };
        zenz.context_size = file.context_size;
        zenz.threads = file.threads;
        zenz.gpu_layers = file.gpu_layers;
        Ok(Self {
            config_path: path.to_owned(),
            zenz,
            system_dictionaries: load_dictionary_paths(&config_dir)?,
            user_dictionary: config_dir.join(USER_DICTIONARY),
            recency_half_life_days: file.recency_half_life_days,
            candidate_order: file.candidate_order,
            rom_kana_rule: file.rom_kana_rule,
            rule_search_paths: rule_search_paths(&config_dir)?,
            fcitx5: file.frontend.fcitx5,
        })
    }
}

impl CandidateOrderConfig {
    fn validate(&self) -> Result<()> {
        for (name, order) in [
            ("okuri_ari", &self.okuri_ari),
            ("okuri_nasi", &self.okuri_nasi),
            ("abbrev", &self.abbrev),
        ] {
            let mut seen = HashSet::new();
            if let Some(duplicate) = order.iter().find(|key| !seen.insert(**key)) {
                anyhow::bail!("candidate_order.{name} に重複があります: {duplicate:?}");
            }
        }
        Ok(())
    }
}

pub(crate) fn parse_huggingface_reference(reference: &str) -> Result<(String, &str)> {
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

const fn default_recency_half_life_days() -> f64 {
    14.0
}

fn default_sort_order_ai() -> Vec<CandidateSortKey> {
    vec![CandidateSortKey::FromUser, CandidateSortKey::AiScore]
}

fn default_sort_order_recency() -> Vec<CandidateSortKey> {
    vec![CandidateSortKey::FromUser, CandidateSortKey::Recency]
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
        assert_eq!(file.recency_half_life_days, 14.0);
        assert_eq!(file.candidate_order, CandidateOrderConfig::default());
        assert_eq!(file.rom_kana_rule, "default");
        assert_eq!(file.frontend.fcitx5, Fcitx5Config::default());
    }

    #[test]
    fn parses_candidate_order() {
        let file: ConfigFile = toml::from_str(
            r#"
            model_path = "model.gguf"

            [candidate_order]
            okuri_ari = ["ai_score", "recency"]
            okuri_nasi = ["from_user"]
            abbrev = []
            "#,
        )
        .unwrap();
        assert_eq!(
            file.candidate_order.okuri_ari,
            [CandidateSortKey::AiScore, CandidateSortKey::Recency]
        );
        assert_eq!(
            file.candidate_order.okuri_nasi,
            [CandidateSortKey::FromUser]
        );
        assert!(file.candidate_order.abbrev.is_empty());
        file.candidate_order.validate().unwrap();
    }

    #[test]
    fn parses_fcitx5_candidate_ui() {
        let file: ConfigFile = toml::from_str(
            r#"
            model_path = "model.gguf"

            [frontend.fcitx5]
            page_size = 8
            candidate_window_threshold = 2
            show_annotations = false
            candidate_key = "digit"
            "#,
        )
        .unwrap();
        assert_eq!(file.frontend.fcitx5.page_size, 8);
        assert_eq!(file.frontend.fcitx5.candidate_window_threshold, 2);
        assert!(!file.frontend.fcitx5.show_annotations);
    }

    #[test]
    fn rejects_duplicate_candidate_order_keys() {
        let order = CandidateOrderConfig {
            okuri_ari: vec![CandidateSortKey::Recency, CandidateSortKey::Recency],
            ..CandidateOrderConfig::default()
        };
        assert!(order.validate().is_err());
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
