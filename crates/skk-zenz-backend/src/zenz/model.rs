use std::ffi::{CStr, CString, c_char};
use std::num::NonZeroU32;
use std::path::Path;
use std::pin::{Pin, pin};
#[cfg(debug_assertions)]
use std::time::Instant;

use anyhow::{Context, Result, bail};
use llama_cpp_2::context::LlamaContext;
use llama_cpp_2::context::params::LlamaContextParams;
use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::model::LlamaModel;
use llama_cpp_2::model::params::LlamaModelParams;
use llama_cpp_2::model::params::kv_overrides::ParamOverrideValue;
use self_cell::self_cell;

use super::ZenzConfig;

type ModelContext<'model> = LlamaContext<'model>;

self_cell!(
    pub(super) struct ModelAndContext {
        owner: LlamaModel,

        #[covariant]
        dependent: ModelContext,
    }
);

pub(super) fn load_model_and_context(
    backend: &LlamaBackend,
    config: &ZenzConfig,
) -> Result<ModelAndContext> {
    if config.parallel_sequences == 0 {
        bail!("parallel_sequencesは1以上必要です");
    }
    #[cfg(debug_assertions)]
    let load_started = Instant::now();
    let model_path = config.model_path()?;
    let model = load_model(backend, model_path, config.gpu_layers)?;
    #[cfg(debug_assertions)]
    eprintln!(
        "model loaded from {}, took {:.2} sec.",
        model_path.display(),
        load_started.elapsed().as_secs_f64()
    );
    let mut context_params = LlamaContextParams::default()
        .with_n_ctx(Some(
            NonZeroU32::new(config.context_size).context("context_sizeは1以上必要です")?,
        ))
        .with_n_seq_max(config.parallel_sequences);
    if let Some(threads) = config.threads {
        context_params = context_params
            .with_n_threads(threads)
            .with_n_threads_batch(threads);
    }
    ModelAndContext::try_new(model, |model| {
        model
            .new_context(backend, context_params)
            .context("推論contextの作成に失敗しました")
    })
}

fn load_model(backend: &LlamaBackend, path: &Path, gpu_layers: u32) -> Result<LlamaModel> {
    #[cfg(feature = "vulkan")]
    let model_params = LlamaModelParams::default().with_n_gpu_layers(gpu_layers);
    #[cfg(not(feature = "vulkan"))]
    let model_params = LlamaModelParams::default();
    #[cfg(not(feature = "vulkan"))]
    let _ = gpu_layers;

    let mut model_params = pin!(model_params);
    let pre_key = CString::new("tokenizer.ggml.pre")?;
    append_string_override(model_params.as_mut(), pre_key.as_c_str(), "gpt-2")?;
    LlamaModel::load_from_file(backend, path, &model_params)
        .with_context(|| format!("モデルを読み込めません: {}", path.display()))
}

fn append_string_override(
    params: Pin<&mut LlamaModelParams>,
    key: &CStr,
    value: &str,
) -> Result<()> {
    let mut buffer = [0 as c_char; 128];
    if value.len() >= buffer.len() {
        bail!("metadata overrideが長すぎます: {key:?}");
    }
    for (target, source) in buffer.iter_mut().zip(value.bytes()) {
        *target = source as c_char;
    }
    params.append_kv_override(key, ParamOverrideValue::Str(buffer));
    Ok(())
}
