use std::path::{Path, PathBuf};

use anyhow::{Result, bail};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ModelSource {
    Local(PathBuf),
    HuggingFace(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ZenzConfig {
    pub model: ModelSource,
    pub context_size: u32,
    pub parallel_sequences: u32,
    pub threads: Option<i32>,
    pub gpu_layers: u32,
}

impl ZenzConfig {
    pub fn new(model_path: impl Into<PathBuf>) -> Self {
        Self {
            model: ModelSource::Local(model_path.into()),
            context_size: 1024,
            parallel_sequences: 1,
            threads: None,
            gpu_layers: 999,
        }
    }

    pub fn from_huggingface(reference: impl Into<String>) -> Self {
        Self {
            model: ModelSource::HuggingFace(reference.into()),
            context_size: 1024,
            parallel_sequences: 1,
            threads: None,
            gpu_layers: 999,
        }
    }

    pub(super) fn model_path(&self) -> Result<&Path> {
        match &self.model {
            ModelSource::Local(path) => Ok(path),
            ModelSource::HuggingFace(reference) => {
                bail!("Hugging Faceモデルが未解決です: {reference}")
            }
        }
    }
}
