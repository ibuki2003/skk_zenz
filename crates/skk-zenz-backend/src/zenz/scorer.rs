use std::collections::{HashSet, VecDeque};
use std::sync::Once;

use anyhow::{Context, Result, bail};
use llama_cpp_2::context::LlamaContext;
use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::llama_batch::LlamaBatch;
use llama_cpp_2::model::AddBos;
use llama_cpp_2::token::LlamaToken;
use llama_cpp_2::{LogOptions, send_logs_to_tracing};

use super::ZenzConfig;
use super::model::{ModelAndContext, load_model_and_context};
use super::prompt::{OUTPUT_TAG, prompt_tokens};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GenerationOptions {
    pub beam_width: usize,
    pub max_tokens: usize,
}

impl Default for GenerationOptions {
    fn default() -> Self {
        Self {
            beam_width: 8,
            max_tokens: 32,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct GeneratedCandidate {
    pub text: String,
    pub loss: f64,
    pub token_count: usize,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct GenerationResult {
    pub candidates: Vec<GeneratedCandidate>,
    pub reused_tokens: usize,
    pub decoded_tokens: usize,
    pub expanded_hypotheses: usize,
    pub finished: bool,
}

#[derive(Clone, Debug)]
struct Hypothesis {
    tokens: Vec<LlamaToken>,
    loss: f64,
}

pub struct GenerationSession {
    base_tokens: Vec<LlamaToken>,
    prefix_tokens: Vec<LlamaToken>,
    active: Vec<Hypothesis>,
    completed: Vec<GeneratedCandidate>,
    ready: VecDeque<GeneratedCandidate>,
    emitted_texts: HashSet<String>,
    options: GenerationOptions,
    depth: usize,
    reused_tokens: usize,
    decoded_tokens: usize,
    expanded_hypotheses: usize,
    finished: bool,
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
                let mut scored_tokens = output_tokens.clone();
                scored_tokens.push(model.token_eos());
                let mut evaluation_tokens = model
                    .str_to_token(&OUTPUT_TAG.to_string(), AddBos::Never)
                    .context("output tagのtoken化に失敗しました")?;
                let output_offset = evaluation_tokens.len();
                evaluation_tokens.extend_from_slice(&scored_tokens);
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
                for (index, token) in scored_tokens.iter().enumerate() {
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
                    let mut scored_tokens = output_tokens.clone();
                    scored_tokens.push(model.token_eos());
                    let mut evaluation_tokens = output_tag.clone();
                    let output_offset = evaluation_tokens.len();
                    evaluation_tokens.extend_from_slice(&scored_tokens);
                    batch_size += evaluation_tokens.len();
                    sequences.push((
                        output_tokens,
                        scored_tokens,
                        evaluation_tokens,
                        output_offset,
                    ));
                }
                let sequence_context_size =
                    llama_context.n_ctx() as usize / self.parallel_sequences as usize;
                if sequences
                    .iter()
                    .any(|(_, _, tokens, _)| cached_len + tokens.len() > sequence_context_size)
                {
                    bail!("batch評価token列がsequenceあたりのcontext sizeを超えます");
                }

                let mut batch = LlamaBatch::new(batch_size, 1);
                let mut logits_indices = Vec::with_capacity(sequences.len());
                for (sequence_index, (_, _, evaluation_tokens, output_offset)) in
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
                for (
                    sequence_index,
                    ((output_tokens, scored_tokens, evaluation_tokens, _), indices),
                ) in sequences.iter().zip(&logits_indices).enumerate()
                {
                    let mut loss = 0.0;
                    for (token, index) in scored_tokens.iter().zip(indices) {
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

    pub fn start_generation(
        &mut self,
        context: &str,
        input: &str,
        output_prefix: &str,
        options: GenerationOptions,
    ) -> Result<GenerationSession> {
        if options.beam_width == 0 {
            bail!("beam_widthは1以上必要です");
        }
        if options.max_tokens == 0 {
            bail!("max_tokensは1以上必要です");
        }

        let prepared = self.prepare(context, input)?;
        let parallel_sequences = self.parallel_sequences;
        let (base_tokens, prefix_tokens) =
            self.model_and_context
                .with_dependent_mut(|model, llama_context| {
                    let mut base_tokens = self.cached_tokens.clone();
                    base_tokens.extend(
                        model
                            .str_to_token(&OUTPUT_TAG.to_string(), AddBos::Never)
                            .context("output tagのtoken化に失敗しました")?,
                    );
                    let prefix_tokens = model
                        .str_to_token(output_prefix, AddBos::Never)
                        .with_context(|| {
                            format!("output prefixのtoken化に失敗しました: {output_prefix}")
                        })?;
                    base_tokens.extend_from_slice(&prefix_tokens);

                    let sequence_context_size =
                        llama_context.n_ctx() as usize / parallel_sequences as usize;
                    if base_tokens.len() + options.max_tokens > sequence_context_size {
                        bail!("生成token列がcontext sizeを超えます");
                    }
                    Ok((base_tokens, prefix_tokens))
                })?;
        Ok(GenerationSession {
            base_tokens,
            prefix_tokens,
            active: vec![Hypothesis {
                tokens: Vec::new(),
                loss: 0.0,
            }],
            completed: Vec::new(),
            ready: VecDeque::new(),
            emitted_texts: HashSet::new(),
            options,
            depth: 0,
            reused_tokens: prepared.reused_tokens,
            decoded_tokens: prepared.decoded_tokens,
            expanded_hypotheses: 0,
            finished: false,
        })
    }

    pub fn continue_generation(
        &mut self,
        session: &mut GenerationSession,
        candidate_count: usize,
    ) -> Result<GenerationResult> {
        if candidate_count == 0 {
            bail!("candidate_countは1以上必要です");
        }
        while session.ready.len() < candidate_count && !session.finished {
            self.advance_generation(session)?;
        }
        let candidates = (0..candidate_count)
            .map_while(|_| session.ready.pop_front())
            .collect();
        Ok(GenerationResult {
            candidates,
            reused_tokens: session.reused_tokens,
            decoded_tokens: session.decoded_tokens,
            expanded_hypotheses: session.expanded_hypotheses,
            finished: session.finished && session.ready.is_empty(),
        })
    }

    fn advance_generation(&mut self, session: &mut GenerationSession) -> Result<()> {
        if session.finished {
            return Ok(());
        }
        let mut cache_tokens = self.cached_tokens.clone();
        self.model_and_context
            .with_dependent_mut(|model, llama_context| {
                let mut continuations = Vec::new();
                for hypothesis in &session.active {
                    session.expanded_hypotheses += 1;
                    let mut target = session.base_tokens.clone();
                    target.extend_from_slice(&hypothesis.tokens);
                    let (token_losses, decoded) = next_token_losses(
                        llama_context,
                        &target,
                        &mut cache_tokens,
                        session.options.beam_width,
                    )?;
                    session.decoded_tokens += decoded;

                    for (token, token_loss) in token_losses {
                        let loss = hypothesis.loss + token_loss;
                        if model.is_eog_token(token) {
                            if session.prefix_tokens.is_empty() && hypothesis.tokens.is_empty() {
                                continue;
                            }
                            let mut output_tokens = session.prefix_tokens.clone();
                            output_tokens.extend_from_slice(&hypothesis.tokens);
                            let Some(text) = tokens_to_string(model, &output_tokens)? else {
                                continue;
                            };
                            if !text.is_empty() && !session.emitted_texts.contains(&text) {
                                session.completed.push(GeneratedCandidate {
                                    text,
                                    loss,
                                    token_count: output_tokens.len(),
                                });
                            }
                        } else {
                            let mut tokens = hypothesis.tokens.clone();
                            tokens.push(token);
                            continuations.push(Hypothesis { tokens, loss });
                        }
                    }
                }

                continuations.sort_by(|left, right| left.loss.total_cmp(&right.loss));
                continuations.truncate(session.options.beam_width);
                session
                    .completed
                    .sort_by(|left, right| left.loss.total_cmp(&right.loss));
                deduplicate_generated(&mut session.completed);
                session.depth += 1;
                if session.depth >= session.options.max_tokens {
                    continuations.clear();
                }
                session.active = continuations;

                let stable_loss_limit = session
                    .active
                    .first()
                    .map_or(f64::INFINITY, |hypothesis| hypothesis.loss);
                let stable_count = stable_candidate_count(&session.completed, stable_loss_limit);
                for candidate in session.completed.drain(..stable_count) {
                    session.emitted_texts.insert(candidate.text.clone());
                    session.ready.push_back(candidate);
                }
                session.finished = session.active.is_empty();
                Ok::<(), anyhow::Error>(())
            })?;
        self.cached_tokens = cache_tokens;
        Ok(())
    }
}

fn next_token_losses(
    context: &mut LlamaContext<'_>,
    target: &[LlamaToken],
    cached_tokens: &mut Vec<LlamaToken>,
    count: usize,
) -> Result<(Vec<(LlamaToken, f64)>, usize)> {
    if target.is_empty() {
        bail!("空のtoken列からは生成できません");
    }
    // logitsを確実に得るため、共通prefixの末尾tokenも毎回decodeし直す。
    let common = common_prefix_len(cached_tokens, target).min(target.len() - 1);
    remove_cache_suffix(context, common)?;
    let suffix = &target[common..];
    let mut batch = LlamaBatch::new(suffix.len(), 1);
    for (offset, token) in suffix.iter().copied().enumerate() {
        batch.add(
            token,
            (common + offset) as i32,
            &[0],
            offset + 1 == suffix.len(),
        )?;
    }
    context
        .decode(&mut batch)
        .context("生成prefixのdecodeに失敗しました")?;
    cached_tokens.clear();
    cached_tokens.extend_from_slice(target);

    let logits = context.get_logits_ith((suffix.len() - 1) as i32);
    let max_logit = logits.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let log_sum_exp = f64::from(max_logit)
        + logits
            .iter()
            .map(|logit| f64::from(*logit - max_logit).exp())
            .sum::<f64>()
            .ln();
    let mut ranked: Vec<_> = logits
        .iter()
        .enumerate()
        .map(|(token, logit)| (LlamaToken(token as i32), log_sum_exp - f64::from(*logit)))
        .collect();
    ranked.sort_unstable_by(|left, right| left.1.total_cmp(&right.1));
    ranked.truncate(count);
    Ok((ranked, suffix.len()))
}

fn tokens_to_string(
    model: &llama_cpp_2::model::LlamaModel,
    tokens: &[LlamaToken],
) -> Result<Option<String>> {
    let mut bytes = Vec::new();
    for token in tokens {
        bytes.extend(model.token_to_piece_bytes(*token, 8, false, None)?);
    }
    Ok(String::from_utf8(bytes).ok())
}

fn deduplicate_generated(candidates: &mut Vec<GeneratedCandidate>) {
    let mut seen = std::collections::HashSet::new();
    candidates.retain(|candidate| seen.insert(candidate.text.clone()));
}

fn stable_candidate_count(candidates: &[GeneratedCandidate], active_loss: f64) -> usize {
    candidates
        .iter()
        .take_while(|candidate| candidate.loss < active_loss)
        .count()
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

    #[test]
    fn generated_candidates_are_deduplicated_without_reordering() {
        let mut candidates = vec![
            GeneratedCandidate {
                text: "変換".to_owned(),
                loss: 1.0,
                token_count: 2,
            },
            GeneratedCandidate {
                text: "返還".to_owned(),
                loss: 2.0,
                token_count: 2,
            },
            GeneratedCandidate {
                text: "変換".to_owned(),
                loss: 3.0,
                token_count: 2,
            },
        ];
        deduplicate_generated(&mut candidates);
        assert_eq!(
            candidates
                .iter()
                .map(|candidate| candidate.text.as_str())
                .collect::<Vec<_>>(),
            ["変換", "返還"]
        );
    }

    #[test]
    fn only_candidates_ahead_of_the_active_frontier_are_stable() {
        let candidates = [
            GeneratedCandidate {
                text: "第一".to_owned(),
                loss: 1.0,
                token_count: 2,
            },
            GeneratedCandidate {
                text: "第二".to_owned(),
                loss: 2.0,
                token_count: 2,
            },
            GeneratedCandidate {
                text: "第三".to_owned(),
                loss: 3.0,
                token_count: 2,
            },
        ];
        assert_eq!(stable_candidate_count(&candidates, 2.5), 2);
        assert_eq!(stable_candidate_count(&candidates, 2.0), 1);
        assert_eq!(stable_candidate_count(&candidates, f64::INFINITY), 3);
    }
}
