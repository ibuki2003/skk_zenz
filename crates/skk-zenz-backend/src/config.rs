use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::zenz::{ZenzConfig, parse_huggingface_reference};

mod dictionary_list;
mod paths;
mod schema;

use dictionary_list::load_dictionary_paths;
use paths::{config_dir, resolve_path, rule_search_paths};
use schema::ConfigFile;
pub use schema::{CandidateOrder, CandidateOrderConfig, Fcitx5Config};

const CONFIG_FILE: &str = "config.toml";
const USER_DICTIONARY: &str = "user.sqlite3";

#[derive(Clone, Debug)]
pub struct BackendConfig {
    pub config_path: PathBuf,
    pub zenz: ZenzConfig,
    pub system_dictionaries: Vec<PathBuf>,
    pub user_dictionary: PathBuf,
    pub recency_half_life_days: f64,
    pub conversion_history: bool,
    pub candidate_order: CandidateOrderConfig,
    pub rom_kana_rule: String,
    pub rule_search_paths: Vec<PathBuf>,
    pub fcitx5: Fcitx5Config,
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
            conversion_history: file.conversion_history,
            candidate_order: file.candidate_order,
            rom_kana_rule: file.rom_kana_rule,
            rule_search_paths: rule_search_paths(&config_dir)?,
            fcitx5: file.frontend.fcitx5,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dictionary_list::{parse_fcitx_dictionary_list, parse_plain_dictionary_list};

    #[test]
    fn sample_config_matches_defaults() {
        let sample: ConfigFile =
            toml::from_str(include_str!("../../../config.sample.toml")).unwrap();

        assert_eq!(sample, ConfigFile::default());
    }

    #[test]
    fn parses_config_defaults() {
        // just test that the default config can be parsed without error
        let file: ConfigFile = toml::from_str(
            r#"
            model_path = "models/model.gguf"
            "#,
        )
        .unwrap();
        assert_eq!(file.huggingface, ConfigFile::default().huggingface);
        assert!(!file.conversion_history);
    }

    #[test]
    fn enables_conversion_history() {
        let file: ConfigFile = toml::from_str(
            r#"
            model_path = "models/model.gguf"
            conversion_history = true
            "#,
        )
        .unwrap();
        assert!(file.conversion_history);
    }

    #[test]
    fn parses_candidate_order() {
        let file: ConfigFile = toml::from_str(
            r#"
            model_path = "model.gguf"

            [candidate_order]
            okuri_ari = "recency_then_ai_score"
            okuri_nasi = "user_first_ai_score"
            abbrev = "ai_score"
            prefix_suffix = "mixture"
            mixture_lambda = 0.25
            "#,
        )
        .unwrap();
        assert_eq!(
            file.candidate_order.okuri_ari,
            CandidateOrder::RecencyThenAiScore
        );
        assert_eq!(
            file.candidate_order.okuri_nasi,
            CandidateOrder::UserFirstAiScore
        );
        assert_eq!(file.candidate_order.abbrev, CandidateOrder::AiScore);
        assert_eq!(file.candidate_order.prefix_suffix, CandidateOrder::Mixture);
        assert_eq!(file.candidate_order.mixture_lambda, 0.25);
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
    fn rejects_invalid_mixture_lambda() {
        let order = CandidateOrderConfig {
            mixture_lambda: f64::NAN,
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
