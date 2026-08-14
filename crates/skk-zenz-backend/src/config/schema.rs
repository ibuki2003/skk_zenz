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
#[serde(default, deny_unknown_fields)]
pub struct CandidateOrderConfig {
    pub okuri_ari: CandidateOrder,
    pub okuri_nasi: CandidateOrder,
    pub abbrev: CandidateOrder,
    pub prefix_suffix: CandidateOrder,
    pub mixture_lambda: f64,
}

impl Default for CandidateOrderConfig {
    fn default() -> Self {
        Self {
            okuri_ari: CandidateOrder::Mixture,
            okuri_nasi: CandidateOrder::Mixture,
            abbrev: CandidateOrder::Recency,
            prefix_suffix: CandidateOrder::Mixture,
            mixture_lambda: 0.2,
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

#[derive(Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub(super) struct FrontendConfig {
    pub(super) fcitx5: Fcitx5Config,
}

#[derive(Debug, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub(super) struct ConfigFile {
    pub(super) model_path: Option<PathBuf>,
    pub(super) huggingface: Option<String>,
    pub(super) rom_kana_rule: String,
    pub(super) context_size: u32,
    pub(super) threads: Option<i32>,
    pub(super) gpu_layers: u32,
    pub(super) recency_half_life_days: f64,
    pub(super) conversion_history: bool,
    pub(super) candidate_order: CandidateOrderConfig,
    pub(super) frontend: FrontendConfig,
}

impl Default for ConfigFile {
    fn default() -> Self {
        Self {
            model_path: None,
            huggingface: Some("Miwa-Keita/zenz-v3.2-small-gguf/ggml-model-Q5_K_M.gguf".to_owned()),
            rom_kana_rule: "default".to_owned(),
            context_size: 1024,
            threads: None,
            gpu_layers: 999,
            recency_half_life_days: 14.0,
            conversion_history: false,
            candidate_order: CandidateOrderConfig::default(),
            frontend: FrontendConfig::default(),
        }
    }
}
