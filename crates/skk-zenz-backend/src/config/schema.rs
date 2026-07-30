use std::collections::HashSet;
use std::path::PathBuf;

use anyhow::Result;
use serde::Deserialize;

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
    #[serde(default = "default_sort_order_recency")]
    pub prefix_suffix: Vec<CandidateSortKey>,
}

impl Default for CandidateOrderConfig {
    fn default() -> Self {
        Self {
            okuri_ari: default_sort_order_ai(),
            okuri_nasi: default_sort_order_recency(),
            abbrev: default_sort_order_recency(),
            prefix_suffix: default_sort_order_recency(),
        }
    }
}

impl CandidateOrderConfig {
    pub(super) fn validate(&self) -> Result<()> {
        for (name, order) in [
            ("okuri_ari", &self.okuri_ari),
            ("okuri_nasi", &self.okuri_nasi),
            ("abbrev", &self.abbrev),
            ("prefix_suffix", &self.prefix_suffix),
        ] {
            let mut seen = HashSet::new();
            if let Some(duplicate) = order.iter().find(|key| !seen.insert(**key)) {
                anyhow::bail!("candidate_order.{name} に重複があります: {duplicate:?}");
            }
        }
        Ok(())
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
pub(super) struct FrontendConfig {
    pub(super) fcitx5: Fcitx5Config,
}

#[derive(Debug, Deserialize)]
pub(super) struct ConfigFile {
    pub(super) model_path: Option<PathBuf>,
    #[serde(default = "default_huggingface_model")]
    pub(super) huggingface: Option<String>,
    #[serde(default = "default_rom_kana_rule")]
    pub(super) rom_kana_rule: String,
    #[serde(default = "default_context_size")]
    pub(super) context_size: u32,
    pub(super) threads: Option<i32>,
    #[serde(default = "default_gpu_layers")]
    pub(super) gpu_layers: u32,
    #[serde(default = "default_recency_half_life_days")]
    pub(super) recency_half_life_days: f64,
    #[serde(default)]
    pub(super) conversion_history: bool,
    #[serde(default)]
    pub(super) candidate_order: CandidateOrderConfig,
    #[serde(default)]
    pub(super) frontend: FrontendConfig,
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

pub(super) fn default_huggingface_model() -> Option<String> {
    Some("Miwa-Keita/zenz-v3.2-small-gguf/ggml-model-Q5_K_M.gguf".to_owned())
}
