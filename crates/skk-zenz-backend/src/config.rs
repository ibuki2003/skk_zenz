use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;

use crate::rom_kana::default_rule_search_paths;
use crate::zenz::ZenzConfig;

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
    model_path: PathBuf,
    system_dictionaries: Vec<PathBuf>,
    user_dictionary: PathBuf,
    #[serde(default = "default_rom_kana_rule")]
    rom_kana_rule: String,
    rule_search_paths: Option<Vec<PathBuf>>,
    #[serde(default = "default_context_size")]
    context_size: u32,
    threads: Option<i32>,
    #[serde(default = "default_gpu_layers")]
    gpu_layers: u32,
}

impl BackendConfig {
    pub fn from_toml_file(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let contents = fs::read_to_string(path)
            .with_context(|| format!("設定ファイルを読めません: {}", path.display()))?;
        let file: ConfigFile = toml::from_str(&contents)
            .with_context(|| format!("設定ファイルを解釈できません: {}", path.display()))?;
        let base = path.parent().unwrap_or_else(|| Path::new(""));
        let mut zenz = ZenzConfig::new(resolve_path(base, file.model_path));
        zenz.context_size = file.context_size;
        zenz.threads = file.threads;
        zenz.gpu_layers = file.gpu_layers;
        Ok(Self {
            zenz,
            system_dictionaries: file
                .system_dictionaries
                .into_iter()
                .map(|path| resolve_path(base, path))
                .collect(),
            user_dictionary: resolve_path(base, file.user_dictionary),
            rom_kana_rule: file.rom_kana_rule,
            rule_search_paths: file.rule_search_paths.map_or_else(
                default_rule_search_paths,
                |paths| {
                    paths
                        .into_iter()
                        .map(|path| resolve_path(base, path))
                        .collect()
                },
            ),
        })
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_defaults_and_resolves_relative_paths() {
        let file: ConfigFile = toml::from_str(
            r#"
            model_path = "models/model.gguf"
            system_dictionaries = ["dict/SKK-JISYO.L"]
            user_dictionary = "data/user.sqlite3"
            "#,
        )
        .unwrap();
        assert_eq!(file.context_size, 1024);
        assert_eq!(file.gpu_layers, 999);
        assert_eq!(file.rom_kana_rule, "default");
        assert_eq!(
            resolve_path(Path::new("/config"), file.model_path),
            Path::new("/config/models/model.gguf")
        );
    }
}
