use std::path::PathBuf;

use anyhow::Result;
use serde::Deserialize;

#[derive(Clone, Copy, Debug, Deserialize, Hash, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CandidateOrder {
    Recency,
    UserFirstAiScore,
    RecencyThenAiScore,
    AiScore,
    Mixture,
}

impl CandidateOrder {
    pub(crate) const fn uses_ai_score(self) -> bool {
        !matches!(self, Self::Recency)
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CandidateOrderConfig {
    #[serde(default = "default_order_mixture")]
    pub okuri_ari: CandidateOrder,
    #[serde(default = "default_order_mixture")]
    pub okuri_nasi: CandidateOrder,
    #[serde(default = "default_order_mixture")]
    pub abbrev: CandidateOrder,
    #[serde(default = "default_order_mixture")]
    pub prefix_suffix: CandidateOrder,
    #[serde(default = "default_mixture_lambda")]
    pub mixture_lambda: f64,
}

impl Default for CandidateOrderConfig {
    fn default() -> Self {
        Self {
            okuri_ari: default_order_mixture(),
            okuri_nasi: default_order_mixture(),
            abbrev: default_order_mixture(),
            prefix_suffix: default_order_mixture(),
            mixture_lambda: default_mixture_lambda(),
        }
    }
}

impl CandidateOrderConfig {
    pub(super) fn validate(&self) -> Result<()> {
        if !self.mixture_lambda.is_finite() || !(0.0..=1.0).contains(&self.mixture_lambda) {
            anyhow::bail!("candidate_order.mixture_lambda は0以上1以下の有限値にしてください");
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

const fn default_order_mixture() -> CandidateOrder {
    CandidateOrder::Mixture
}

const fn default_mixture_lambda() -> f64 {
    0.2
}

fn default_rom_kana_rule() -> String {
    "default".to_owned()
}

pub(super) fn default_huggingface_model() -> Option<String> {
    Some("Miwa-Keita/zenz-v3.2-small-gguf/ggml-model-Q5_K_M.gguf".to_owned())
}
