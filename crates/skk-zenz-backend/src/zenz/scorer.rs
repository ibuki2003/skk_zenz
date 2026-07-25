use std::ffi::{CStr, CString, c_char};
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::pin::{Pin, pin};
use std::sync::Once;
use std::time::Instant;

use anyhow::{Context, Result, bail};
use llama_cpp_2::context::LlamaContext;
use llama_cpp_2::context::params::LlamaContextParams;
use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::llama_batch::LlamaBatch;
use llama_cpp_2::model::params::LlamaModelParams;
use llama_cpp_2::model::params::kv_overrides::ParamOverrideValue;
use llama_cpp_2::model::{AddBos, LlamaModel};
use llama_cpp_2::token::LlamaToken;
use llama_cpp_2::{LogOptions, send_logs_to_tracing};
use self_cell::self_cell;

use super::prompt::{OUTPUT_TAG, prompt_tokens};

type ModelContext<'model> = LlamaContext<'model>;

self_cell!(
    struct ModelAndContext {
        owner: LlamaModel,

        #[covariant]
        dependent: ModelContext,
    }
);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ZenzConfig {
    pub model_path: PathBuf,
    pub context_size: u32,
    pub parallel_sequences: u32,
    pub threads: Option<i32>,
    pub gpu_layers: u32,
}

impl ZenzConfig {
    pub fn new(model_path: impl Into<PathBuf>) -> Self {
        Self {
            model_path: model_path.into(),
            context_size: 1024,
            parallel_sequences: 1,
            threads: None,
            gpu_layers: 999,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Score {
    pub loss: f64,
    pub token_count: usize,
    pub reused_tokens: usize,
    pub decoded_tokens: usize,
}

pub struct ZenzScorer {
    model_and_context: ModelAndContext,
    cached_tokens: Vec<LlamaToken>,
    parallel_sequences: u32,
    // model/contextより後にdropさせるため、フィールド順を末尾にする。
    #[allow(dead_code)]
    backend: LlamaBackend,
}

impl ZenzScorer {
    pub fn load(config: &ZenzConfig) -> Result<Self> {
        static LLAMA_LOG_INIT: Once = Once::new();
        LLAMA_LOG_INIT.call_once(|| {
            send_logs_to_tracing(LogOptions::default().with_logs_enabled(false));
        });
        let backend = LlamaBackend::init().context("llama.cpp backendの初期化に失敗しました")?;
        let model_and_context = load_model_and_context(&backend, config)?;
        Ok(Self {
            model_and_context,
            cached_tokens: Vec::new(),
            parallel_sequences: config.parallel_sequences,
            backend,
        })
    }

    pub(crate) fn reload(&mut self, config: &ZenzConfig) -> Result<()> {
        let model_and_context = load_model_and_context(&self.backend, config)?;
        self.model_and_context = model_and_context;
        self.cached_tokens.clear();
        self.parallel_sequences = config.parallel_sequences;
        Ok(())
    }

    pub fn clear(&mut self) {
        self.model_and_context
            .with_dependent_mut(|_, context| context.clear_kv_cache());
        self.cached_tokens.clear();
    }

    pub fn prepare(&mut self, context: &str, input: &str) -> Result<Score> {
        let target = prompt_tokens(self.model_and_context.borrow_owner(), context, input)?;
        let common = common_prefix_len(&self.cached_tokens, &target);
        self.model_and_context
            .with_dependent_mut(|_, llama_context| {
                remove_cache_suffix(llama_context, common)?;
                decode_suffix(llama_context, &target[common..], common)
            })?;
        let result = Score {
            reused_tokens: common,
            decoded_tokens: target.len() - common,
            ..Score::default()
        };
        self.cached_tokens = target;
        Ok(result)
    }

    pub fn score(&mut self, context: &str, input: &str, output: &str) -> Result<Score> {
        let prepared = self.prepare(context, input)?;
        let cached_len = self.cached_tokens.len();
        self.model_and_context
            .with_dependent_mut(|model, llama_context| {
                let output_tokens = model
                    .str_to_token(output, AddBos::Never)
                    .with_context(|| format!("outputのtoken化に失敗しました: {output}"))?;
                if output_tokens.is_empty() {
                    bail!("空のoutputは評価できません");
                }
                let mut evaluation_tokens = model
                    .str_to_token(&OUTPUT_TAG.to_string(), AddBos::Never)
                    .context("output tagのtoken化に失敗しました")?;
                let output_offset = evaluation_tokens.len();
                evaluation_tokens.extend_from_slice(&output_tokens);
                let sequence_context_size =
                    llama_context.n_ctx() as usize / self.parallel_sequences as usize;
                if cached_len + evaluation_tokens.len() > sequence_context_size {
                    bail!("評価token列がcontext sizeを超えます");
                }

                let mut batch = LlamaBatch::new(evaluation_tokens.len(), 1);
                for (offset, token) in evaluation_tokens.iter().copied().enumerate() {
                    let needs_logits =
                        offset + 1 >= output_offset && offset < evaluation_tokens.len() - 1;
                    batch.add(token, (cached_len + offset) as i32, &[0], needs_logits)?;
                }
                llama_context
                    .decode(&mut batch)
                    .context("候補のdecodeに失敗しました")?;

                let mut loss = 0.0;
                for (index, token) in output_tokens.iter().enumerate() {
                    let logits = llama_context.get_logits_ith((output_offset + index - 1) as i32);
                    loss += token_loss(logits, *token);
                }
                remove_cache_suffix(llama_context, cached_len)?;
                Ok(Score {
                    loss,
                    token_count: output_tokens.len(),
                    reused_tokens: prepared.reused_tokens,
                    decoded_tokens: prepared.decoded_tokens + evaluation_tokens.len(),
                })
            })
    }

    pub fn score_batch(
        &mut self,
        context: &str,
        input: &str,
        outputs: &[&str],
    ) -> Result<Vec<Score>> {
        if outputs.is_empty() {
            return Ok(Vec::new());
        }
        if outputs.len() + 1 > self.parallel_sequences as usize {
            bail!(
                "batch候補数{}に対してparallel_sequences={}が不足しています",
                outputs.len(),
                self.parallel_sequences
            );
        }
        let prepared = self.prepare(context, input)?;
        let cached_len = self.cached_tokens.len();
        self.model_and_context
            .with_dependent_mut(|model, llama_context| {
                let output_tag = model
                    .str_to_token(&OUTPUT_TAG.to_string(), AddBos::Never)
                    .context("output tagのtoken化に失敗しました")?;
                let mut sequences = Vec::with_capacity(outputs.len());
                let mut batch_size = 0;
                for output in outputs {
                    let output_tokens = model
                        .str_to_token(output, AddBos::Never)
                        .with_context(|| format!("outputのtoken化に失敗しました: {output}"))?;
                    if output_tokens.is_empty() {
                        bail!("空のoutputは評価できません");
                    }
                    let mut evaluation_tokens = output_tag.clone();
                    let output_offset = evaluation_tokens.len();
                    evaluation_tokens.extend_from_slice(&output_tokens);
                    batch_size += evaluation_tokens.len();
                    sequences.push((output_tokens, evaluation_tokens, output_offset));
                }
                let sequence_context_size =
                    llama_context.n_ctx() as usize / self.parallel_sequences as usize;
                if sequences
                    .iter()
                    .any(|(_, tokens, _)| cached_len + tokens.len() > sequence_context_size)
                {
                    bail!("batch評価token列がsequenceあたりのcontext sizeを超えます");
                }

                let mut batch = LlamaBatch::new(batch_size, 1);
                let mut logits_indices = Vec::with_capacity(sequences.len());
                for (sequence_index, (_, evaluation_tokens, output_offset)) in
                    sequences.iter().enumerate()
                {
                    let sequence_id = i32::try_from(sequence_index + 1)
                        .context("候補数がsequence IDの上限を超えました")?;
                    llama_context.copy_kv_cache_seq(0, sequence_id, None, None)?;
                    let mut indices =
                        Vec::with_capacity(evaluation_tokens.len().saturating_sub(*output_offset));
                    for (offset, token) in evaluation_tokens.iter().copied().enumerate() {
                        let needs_logits =
                            offset + 1 >= *output_offset && offset < evaluation_tokens.len() - 1;
                        let batch_index = batch.n_tokens();
                        batch.add(
                            token,
                            (cached_len + offset) as i32,
                            &[sequence_id],
                            needs_logits,
                        )?;
                        if needs_logits {
                            indices.push(batch_index);
                        }
                    }
                    logits_indices.push(indices);
                }
                llama_context
                    .decode(&mut batch)
                    .context("候補batchのdecodeに失敗しました")?;

                let mut scores = Vec::with_capacity(sequences.len());
                for (sequence_index, ((output_tokens, evaluation_tokens, _), indices)) in
                    sequences.iter().zip(&logits_indices).enumerate()
                {
                    let mut loss = 0.0;
                    for (token, index) in output_tokens.iter().zip(indices) {
                        loss += token_loss(llama_context.get_logits_ith(*index), *token);
                    }
                    scores.push(Score {
                        loss,
                        token_count: output_tokens.len(),
                        reused_tokens: prepared.reused_tokens,
                        decoded_tokens: prepared.decoded_tokens + evaluation_tokens.len(),
                    });
                    llama_context.kv_cache_seq_rm((sequence_index + 1) as i32, None, None)?;
                }
                Ok(scores)
            })
    }
}

fn load_model_and_context(backend: &LlamaBackend, config: &ZenzConfig) -> Result<ModelAndContext> {
    if config.parallel_sequences == 0 {
        bail!("parallel_sequencesは1以上必要です");
    }
    let load_started = Instant::now();
    let model = load_model(backend, &config.model_path, config.gpu_layers)?;
    eprintln!(
        "model loaded from {}, took {:.2} sec.",
        config.model_path.display(),
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
    #[cfg(any(feature = "cuda", feature = "vulkan"))]
    let model_params = LlamaModelParams::default().with_n_gpu_layers(gpu_layers);
    #[cfg(not(any(feature = "cuda", feature = "vulkan")))]
    let model_params = LlamaModelParams::default();
    #[cfg(not(any(feature = "cuda", feature = "vulkan")))]
    let _ = gpu_layers;

    let mut model_params = pin!(model_params);
    let pre_key = CString::new("tokenizer.ggml.pre")?;
    append_string_override(model_params.as_mut(), pre_key.as_c_str(), "gpt-2")?;
    LlamaModel::load_from_file(backend, path, &model_params)
        .with_context(|| format!("モデルを読み込めません: {}", path.display()))
}

fn decode_suffix(
    context: &mut LlamaContext<'_>,
    suffix: &[LlamaToken],
    position: usize,
) -> Result<()> {
    if suffix.is_empty() {
        return Ok(());
    }
    let mut batch = LlamaBatch::new(suffix.len(), 1);
    for (offset, token) in suffix.iter().copied().enumerate() {
        batch.add(token, (position + offset) as i32, &[0], false)?;
    }
    context
        .decode(&mut batch)
        .context("promptのdecodeに失敗しました")
}

fn remove_cache_suffix(context: &mut LlamaContext<'_>, position: usize) -> Result<()> {
    context
        .kv_cache_seq_rm(0, Some(position as u32), None)
        .context("KV cacheのsuffixを削除できません")
}

fn token_loss(logits: &[f32], token: LlamaToken) -> f64 {
    let max_logit = logits.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let sum_exp: f64 = logits
        .iter()
        .map(|logit| f64::from(*logit - max_logit).exp())
        .sum();
    f64::from(max_logit) + sum_exp.ln() - f64::from(logits[token.0 as usize])
}

fn common_prefix_len(left: &[LlamaToken], right: &[LlamaToken]) -> usize {
    left.iter()
        .zip(right)
        .take_while(|(left, right)| left == right)
        .count()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn common_prefix_is_counted() {
        assert_eq!(
            common_prefix_len(
                &[LlamaToken(1), LlamaToken(2), LlamaToken(3)],
                &[LlamaToken(1), LlamaToken(2), LlamaToken(4)]
            ),
            2
        );
    }
}
