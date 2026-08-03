use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use skk_zenz_backend::zenz::{ModelSource, ZenzConfig, ZenzScorer};
use skk_zenz_backend::{
    BackendConfig, CandidateOrderConfig, CandidateSource, ConversionCore, MemoryUserDictionary,
};

mod history;
mod scored;

use history::read_history;
use scored::{ScoredCandidate, ScoredGroup};

#[derive(Debug, Parser)]
#[command(about = "skk-zenz リランキング評価ツール")]
struct Args {
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Debug, Subcommand)]
enum Cmd {
    /// conversion_history から候補を生成し AI スコアを付けて JSONL に書く
    Score(ScoreArgs),
    /// scored JSONL を読み、各戦略で MRR / top-1 rate を計算する
    Analyze(AnalyzeArgs),
}

#[derive(Debug, Parser)]
struct ScoreArgs {
    /// config.toml のパス（省略時は XDG デフォルト）
    #[arg(long)]
    config: Option<PathBuf>,

    /// conversion_history.csv のパス（省略時は config と同じディレクトリ）
    #[arg(long)]
    history: Option<PathBuf>,

    /// 出力 JSONL ファイル
    #[arg(short, long, default_value = "scored.jsonl")]
    output: PathBuf,
}

#[derive(Debug, Parser)]
struct AnalyzeArgs {
    /// scored JSONL ファイル
    #[arg(long, default_value = "scored.jsonl")]
    input: PathBuf,

    /// 同じ入力・送り・正解候補の2回目以降だけを評価する
    #[arg(long)]
    repeated_only: bool,

    /// 出力 TSV ファイル
    #[arg(short, long, default_value = "reranking.tsv")]
    output: PathBuf,

    /// lambda の刻み幅
    #[arg(long, default_value_t = 0.01)]
    lambda_step: f64,

    /// 候補数 n に対する n / (n + k) の補正定数
    #[arg(long, default_value_t = 3.0)]
    correction_k: f64,

    /// sigmoid の傾き
    #[arg(long, default_value_t = 4.0)]
    sigmoid_a: f64,

    /// sigmoid の中心
    #[arg(long, default_value_t = 0.5)]
    sigmoid_b: f64,

    /// 平滑化で使う固定の user confidence
    #[arg(long, default_value_t = 0.5)]
    fixed_g: f64,
}

fn main() -> Result<()> {
    let args = Args::parse();
    match args.command {
        Cmd::Score(args) => cmd_score(args),
        Cmd::Analyze(args) => cmd_analyze(args),
    }
}

// ── score subcommand ──

fn resolve_zenz_config(config: &mut ZenzConfig) -> Result<()> {
    let ModelSource::HuggingFace(reference) = &config.model else {
        return Ok(());
    };
    let mut fields = reference.splitn(3, '/');
    let owner = fields.next().unwrap_or_default();
    let repository = fields.next().unwrap_or_default();
    let filename = fields.next().unwrap_or_default();
    let repo_id = format!("{owner}/{repository}");
    eprintln!("モデルを取得中: {reference}");
    let api = hf_hub::api::sync::ApiBuilder::new()
        .with_progress(true)
        .build()
        .context("Hugging Face API の初期化に失敗しました")?;
    let path = api
        .model(repo_id)
        .get(filename)
        .with_context(|| format!("{reference} の取得に失敗しました"))?;
    config.model = ModelSource::Local(path);
    Ok(())
}

fn cmd_score(args: ScoreArgs) -> Result<()> {
    let backend_config = match &args.config {
        Some(path) => BackendConfig::from_toml_file(path)?,
        None => BackendConfig::from_xdg()?,
    };

    let history_path = args.history.unwrap_or_else(|| {
        backend_config
            .config_path
            .parent()
            .unwrap_or(Path::new(""))
            .join("conversion_history.csv")
    });
    let entries = read_history(&history_path)?;
    eprintln!("履歴エントリ数: {}", entries.len());

    let dict_paths: Vec<&Path> = backend_config
        .system_dictionaries
        .iter()
        .map(PathBuf::as_path)
        .collect();
    let user_dict = MemoryUserDictionary::new(backend_config.recency_half_life_days);
    let mut core = ConversionCore::load(&dict_paths, user_dict, CandidateOrderConfig::default())?;

    let mut zenz_config = backend_config.zenz;
    resolve_zenz_config(&mut zenz_config)?;
    eprintln!("モデルをロード中...");
    let mut scorer = ZenzScorer::load(&zenz_config)?;

    let out_file =
        File::create(&args.output).with_context(|| format!("出力先: {}", args.output.display()))?;
    let mut writer = BufWriter::new(out_file);

    let mut scored_count = 0u64;
    let mut registered_count = 0u64;

    let mut evaluated_candidates = 0u64;
    let mut last_time = std::time::Instant::now();
    let mut usage_counts = HashMap::new();

    for (i, entry) in entries.iter().enumerate() {
        let okuri = if entry.okuri.is_empty() {
            None
        } else {
            Some(entry.okuri.as_str())
        };
        let now = (entry.timestamp_ms / 1000) as i64;
        let correct_output = format!("{}{}", entry.text, entry.okuri);
        let usage_key = (
            entry.input.clone(),
            entry.okuri.clone(),
            correct_output.clone(),
        );
        let usage_count = usage_counts.entry(usage_key).or_insert(0usize);
        let prior_uses = *usage_count;
        *usage_count += 1;

        let mut candidates = core.lookup_at(&entry.input, okuri, now)?;

        // 正解が候補にない場合はユーザ辞書に登録して再 lookup
        if !candidates.iter().any(|c| c.output == correct_output) {
            core.register_at(&entry.input, okuri, &entry.text, None, now)?;
            candidates = core.lookup_at(&entry.input, okuri, now)?;
            registered_count += 1;
            if !candidates.iter().any(|c| c.output == correct_output) {
                eprintln!(
                    "スキップ: 入力={} 正解={} (登録後も候補に含まれない)",
                    entry.input, correct_output,
                );
                continue;
            }
        }

        scorer.clear();
        scorer.prepare(&entry.context, &entry.input)?;

        evaluated_candidates += candidates.len() as u64;
        let scored_candidates: Vec<ScoredCandidate> = candidates
            .iter()
            .map(|c| {
                let score = scorer.score(&entry.context, &entry.input, &c.output)?;
                Ok(ScoredCandidate {
                    text: c.output.clone(),
                    loss: score.loss,
                    token_count: score.token_count,
                    recency: c.recency,
                    from_user: c.source == CandidateSource::User,
                })
            })
            .collect::<Result<_>>()?;

        // 正解候補の使用を記録（recency の再現）
        if let Some(chosen) = candidates.iter().find(|c| c.output == correct_output) {
            core.record_usage_at(chosen, now)?;
        }

        let group = ScoredGroup {
            context: entry.context.clone(),
            input: entry.input.clone(),
            okuri: entry.okuri.clone(),
            correct: correct_output.clone(),
            prior_uses,
            candidates: scored_candidates,
        };

        serde_json::to_writer(&mut writer, &group)?;
        writeln!(writer)?;
        scored_count += 1;

        if (i + 1) % 50 == 0 {
            let eval_rate = evaluated_candidates as f64 / last_time.elapsed().as_secs_f64();
            last_time = std::time::Instant::now();
            evaluated_candidates = 0;

            eprintln!(
                "  {}/{} (scored={scored_count}, registered={registered_count}, speed={:.1} candidates/s)",
                i + 1,
                entries.len(),
                eval_rate,
            );
        }
    }
    writer.flush()?;
    eprintln!(
        "完了: scored={scored_count}, registered={registered_count}, 出力={}",
        args.output.display()
    );
    Ok(())
}

// ── analyze subcommand ──

fn load_scored(path: &Path) -> Result<Vec<ScoredGroup>> {
    let file = File::open(path).with_context(|| format!("読めません: {}", path.display()))?;
    let reader = BufReader::new(file);
    let mut groups = Vec::new();
    for (i, line) in reader.lines().enumerate() {
        let line = line?;
        if line.is_empty() {
            continue;
        }
        let group: ScoredGroup =
            serde_json::from_str(&line).with_context(|| format!("{}行目のパースに失敗", i + 1))?;
        groups.push(group);
    }
    Ok(groups)
}

fn retain_repeated_groups(groups: &mut Vec<ScoredGroup>) {
    groups.retain(|group| group.prior_uses > 0);
}

struct EvalResult {
    mrr: f64,
    top1: f64,
}

#[derive(Clone, Copy)]
enum Term {
    Probability,
    LogProbability,
}

#[derive(Clone, Copy)]
enum UserAdjustment {
    None,
    Correction,
    Sigmoid,
    Entropy,
    SigmoidCorrection,
    EntropyCorrection,
}

#[derive(Clone, Copy)]
enum Smoothing {
    Correction,
    Fixed,
    Sigmoid,
    Entropy,
}

#[derive(Clone, Copy)]
enum UserTreatment {
    Existing(UserAdjustment),
    Smoothed(Smoothing),
}

#[derive(Clone, Copy)]
enum LengthNormalization {
    None,
    TokenPower(f64),
    CharacterPower(f64),
}

#[derive(Clone)]
struct PlotStrategy {
    label: String,
    term: Term,
    user_treatment: UserTreatment,
    length_normalization: LengthNormalization,
}

fn plot_strategies() -> Vec<PlotStrategy> {
    plot_strategies_length_normalization()
}

fn plot_strategies_methods() -> Vec<PlotStrategy> {
    vec![
        // linear probability
        PlotStrategy {
            label: "prob_none".to_string(),
            term: Term::Probability,
            user_treatment: UserTreatment::Existing(UserAdjustment::None),
            length_normalization: LengthNormalization::None,
        },
        PlotStrategy {
            label: "prob_correction".to_string(),
            term: Term::Probability,
            user_treatment: UserTreatment::Existing(UserAdjustment::Correction),
            length_normalization: LengthNormalization::None,
        },
        PlotStrategy {
            label: "prob_sigmoid".to_string(),
            term: Term::Probability,
            user_treatment: UserTreatment::Existing(UserAdjustment::Sigmoid),
            length_normalization: LengthNormalization::None,
        },
        PlotStrategy {
            label: "prob_entropy".to_string(),
            term: Term::Probability,
            user_treatment: UserTreatment::Existing(UserAdjustment::Entropy),
            length_normalization: LengthNormalization::None,
        },
        PlotStrategy {
            label: "prob_sigmoid_correction".to_string(),
            term: Term::Probability,
            user_treatment: UserTreatment::Existing(UserAdjustment::SigmoidCorrection),
            length_normalization: LengthNormalization::None,
        },
        PlotStrategy {
            label: "prob_entropy_correction".to_string(),
            term: Term::Probability,
            user_treatment: UserTreatment::Existing(UserAdjustment::EntropyCorrection),
            length_normalization: LengthNormalization::None,
        },
        // log probability
        PlotStrategy {
            label: "log_smooth_correction".to_string(),
            term: Term::LogProbability,
            user_treatment: UserTreatment::Smoothed(Smoothing::Correction),
            length_normalization: LengthNormalization::None,
        },
        PlotStrategy {
            label: "log_smooth_fixed".to_string(),
            term: Term::LogProbability,
            user_treatment: UserTreatment::Smoothed(Smoothing::Fixed),
            length_normalization: LengthNormalization::None,
        },
        PlotStrategy {
            label: "log_smooth_sigmoid".to_string(),
            term: Term::LogProbability,
            user_treatment: UserTreatment::Smoothed(Smoothing::Sigmoid),
            length_normalization: LengthNormalization::None,
        },
        PlotStrategy {
            label: "log_smooth_entropy".to_string(),
            term: Term::LogProbability,
            user_treatment: UserTreatment::Smoothed(Smoothing::Entropy),
            length_normalization: LengthNormalization::None,
        },
    ]
}

fn plot_strategies_length_normalization() -> Vec<PlotStrategy> {
    // baseline; log probability + smoothed entropy
    let baseline = PlotStrategy {
        label: "baseline".to_string(),
        term: Term::LogProbability,
        user_treatment: UserTreatment::Smoothed(Smoothing::Entropy),
        length_normalization: LengthNormalization::None,
    };

    let mut strategies = vec![baseline.clone()];

    // token
    for exponent in [0.25, 0.5, 0.75, 1.0] {
        strategies.push(PlotStrategy {
            label: format!("token_{:03}", (exponent * 100.0) as usize),
            length_normalization: LengthNormalization::TokenPower(exponent),
            ..baseline.clone()
        });
    }
    // character
    for exponent in [0.25, 0.5, 0.75, 1.0] {
        strategies.push(PlotStrategy {
            label: format!("character_{:03}", (exponent * 100.0) as usize),
            length_normalization: LengthNormalization::CharacterPower(exponent),
            ..baseline.clone()
        });
    }
    strategies
}

fn normalized(values: impl Iterator<Item = f64>, count: usize) -> Vec<f64> {
    let values: Vec<_> = values.collect();
    let sum: f64 = values.iter().sum();
    if sum > 0.0 && sum.is_finite() {
        values.into_iter().map(|value| value / sum).collect()
    } else {
        vec![1.0 / count as f64; count]
    }
}

fn confidence_factors(probabilities: &[f64], a: f64, b: f64) -> (f64, f64) {
    if probabilities.len() <= 1 {
        return (1.0, 1.0);
    }

    let mut descending = probabilities.to_vec();
    descending.sort_by(|left, right| right.total_cmp(left));
    let ratio = descending[0] / descending[1].max(f64::MIN_POSITIVE);
    let sigmoid = 1.0 / (1.0 + (-a * (ratio.ln() - b)).exp());

    let entropy = -probabilities
        .iter()
        .filter(|probability| **probability > 0.0)
        .map(|probability| probability * probability.ln())
        .sum::<f64>();
    let entropy_confidence = (1.0 - entropy / (probabilities.len() as f64).ln()).clamp(0.0, 1.0);
    (sigmoid, entropy_confidence)
}

fn evaluate_strategy(
    groups: &[ScoredGroup],
    strategy: &PlotStrategy,
    lambda: f64,
    args: &AnalyzeArgs,
) -> EvalResult {
    let mut reciprocal_rank_sum = 0.0;
    let mut top1_count = 0usize;

    for group in groups {
        let count = group.candidates.len();
        if count == 0 {
            continue;
        }
        let normalized_losses: Vec<_> = group
            .candidates
            .iter()
            .map(|candidate| {
                let length = match strategy.length_normalization {
                    LengthNormalization::None => 1.0,
                    LengthNormalization::TokenPower(exponent) => {
                        (candidate.token_count.max(1) as f64).powf(exponent)
                    }
                    LengthNormalization::CharacterPower(exponent) => {
                        (candidate.text.chars().count().max(1) as f64).powf(exponent)
                    }
                };
                candidate.loss / length
            })
            .collect();
        let min_loss = normalized_losses
            .iter()
            .copied()
            .fold(f64::INFINITY, f64::min);
        let ai_probabilities = normalized(
            normalized_losses
                .iter()
                .map(|loss| (-(loss - min_loss)).exp()),
            count,
        );
        let user_probabilities = normalized(
            group
                .candidates
                .iter()
                .map(|candidate| candidate.recency.unwrap_or_default().max(0.0)),
            count,
        );
        let (sigmoid, entropy) =
            confidence_factors(&user_probabilities, args.sigmoid_a, args.sigmoid_b);
        let correction = count as f64 / (count as f64 + args.correction_k);

        let mut ranked: Vec<_> = group.candidates.iter().enumerate().collect();
        ranked.sort_by(|(left_index, _), (right_index, _)| {
            let score = |index: usize| {
                let probability_term = |probability: f64| match strategy.term {
                    Term::Probability => probability,
                    Term::LogProbability => probability.max(f64::MIN_POSITIVE).ln(),
                };
                let (user_probability, user_factor) = match strategy.user_treatment {
                    UserTreatment::Existing(adjustment) => {
                        let factor = match adjustment {
                            UserAdjustment::None => 1.0,
                            UserAdjustment::Correction => correction,
                            UserAdjustment::Sigmoid => sigmoid,
                            UserAdjustment::Entropy => entropy,
                            UserAdjustment::SigmoidCorrection => sigmoid * correction,
                            UserAdjustment::EntropyCorrection => entropy * correction,
                        };
                        (user_probabilities[index], factor)
                    }
                    UserTreatment::Smoothed(smoothing) => {
                        let confidence = match smoothing {
                            Smoothing::Correction => correction,
                            Smoothing::Fixed => args.fixed_g,
                            Smoothing::Sigmoid => sigmoid,
                            Smoothing::Entropy => entropy,
                        };
                        let probability = confidence * user_probabilities[index]
                            + (1.0 - confidence) / count as f64;
                        (probability, 1.0)
                    }
                };
                lambda * probability_term(ai_probabilities[index])
                    + (1.0 - lambda) * probability_term(user_probability) * user_factor
            };
            score(*right_index).total_cmp(&score(*left_index))
        });

        if let Some(rank) = ranked
            .iter()
            .position(|(_, candidate)| candidate.text == group.correct)
        {
            let rank1 = rank + 1;
            reciprocal_rank_sum += 1.0 / rank1 as f64;
            if rank == 0 {
                top1_count += 1;
            }
        }
    }

    EvalResult {
        mrr: reciprocal_rank_sum / groups.len() as f64,
        top1: top1_count as f64 / groups.len() as f64,
    }
}

fn cmd_analyze(args: AnalyzeArgs) -> Result<()> {
    if !(args.lambda_step > 0.0 && args.lambda_step <= 1.0) {
        bail!("--lambda-step は 0 より大きく 1 以下にしてください");
    }
    if args.correction_k <= 0.0 {
        bail!("--correction-k は 0 より大きくしてください");
    }
    if !(0.0..=1.0).contains(&args.fixed_g) {
        bail!("--fixed-g は 0 以上 1 以下にしてください");
    }
    let mut groups = load_scored(&args.input)?;
    let total_count = groups.len();
    if args.repeated_only {
        retain_repeated_groups(&mut groups);
    }
    if groups.is_empty() {
        bail!("評価対象が空です");
    }
    eprintln!("グループ数: {}/{}", groups.len(), total_count);

    let plot_strategies = plot_strategies();

    let file =
        File::create(&args.output).with_context(|| format!("出力先: {}", args.output.display()))?;
    let mut out = BufWriter::new(file);
    write!(out, "lambda")?;
    for strategy in &plot_strategies {
        write!(out, "\t{}_MRR\t{}_top1", strategy.label, strategy.label)?;
    }
    writeln!(out)?;

    let steps = (1.0 / args.lambda_step).ceil() as usize;
    for step in 0..=steps {
        let lambda = (step as f64 * args.lambda_step).min(1.0);
        write!(out, "{lambda:.6}")?;
        for strategy in &plot_strategies {
            let result = evaluate_strategy(&groups, strategy, lambda, &args);
            write!(out, "\t{:.6}\t{:.6}", result.mrr, result.top1)?;
        }
        writeln!(out)?;
    }
    out.flush()?;

    eprintln!("完了: データ={}", args.output.display());

    Ok(())
}
