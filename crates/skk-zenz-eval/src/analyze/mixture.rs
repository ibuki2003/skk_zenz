use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::Parser;

use super::{
    EvalResult, LengthNormalization, length_normalized_loss, load_scored, retain_repeated_groups,
};
use crate::scored::ScoredGroup;

#[derive(Debug, Parser)]
pub struct Args {
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

struct Strategy {
    label: &'static str,
    term: Term,
    user_treatment: UserTreatment,
}

#[rustfmt::skip]
const STRATEGIES: &[Strategy] = &[
    Strategy { label: "prob_none",               term: Term::Probability,    user_treatment: UserTreatment::Existing(UserAdjustment::None) },
    Strategy { label: "prob_correction",         term: Term::Probability,    user_treatment: UserTreatment::Existing(UserAdjustment::Correction) },
    Strategy { label: "prob_sigmoid",            term: Term::Probability,    user_treatment: UserTreatment::Existing(UserAdjustment::Sigmoid) },
    Strategy { label: "prob_entropy",            term: Term::Probability,    user_treatment: UserTreatment::Existing(UserAdjustment::Entropy) },
    Strategy { label: "prob_sigmoid_correction", term: Term::Probability,    user_treatment: UserTreatment::Existing(UserAdjustment::SigmoidCorrection) },
    Strategy { label: "prob_entropy_correction", term: Term::Probability,    user_treatment: UserTreatment::Existing(UserAdjustment::EntropyCorrection) },
    Strategy { label: "log_smooth_correction",   term: Term::LogProbability, user_treatment: UserTreatment::Smoothed(Smoothing::Correction) },
    Strategy { label: "log_smooth_fixed",        term: Term::LogProbability, user_treatment: UserTreatment::Smoothed(Smoothing::Fixed) },
    Strategy { label: "log_smooth_sigmoid",      term: Term::LogProbability, user_treatment: UserTreatment::Smoothed(Smoothing::Sigmoid) },
    Strategy { label: "log_smooth_entropy",      term: Term::LogProbability, user_treatment: UserTreatment::Smoothed(Smoothing::Entropy) },
];

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

fn evaluate(groups: &[ScoredGroup], strategy: &Strategy, lambda: f64, args: &Args) -> EvalResult {
    let mut reciprocal_rank_sum = 0.0;
    let mut top1_count = 0usize;
    for group in groups {
        let count = group.candidates.len();
        if count == 0 {
            continue;
        }
        let losses: Vec<_> = group
            .candidates
            .iter()
            .map(|candidate| length_normalized_loss(candidate, LengthNormalization::None))
            .collect();
        let min_loss = losses.iter().copied().fold(f64::INFINITY, f64::min);
        let ai = normalized(losses.iter().map(|loss| (-(loss - min_loss)).exp()), count);
        let user = normalized(
            group
                .candidates
                .iter()
                .map(|candidate| candidate.recency.unwrap_or_default().max(0.0)),
            count,
        );
        let (sigmoid, entropy) = confidence_factors(&user, args.sigmoid_a, args.sigmoid_b);
        let correction = count as f64 / (count as f64 + args.correction_k);
        let mut ranked: Vec<_> = group.candidates.iter().enumerate().collect();
        ranked.sort_by(|(left, _), (right, _)| {
            let score = |index: usize| {
                let term = |probability: f64| match strategy.term {
                    Term::Probability => probability,
                    Term::LogProbability => probability.max(f64::MIN_POSITIVE).ln(),
                };
                let (user_probability, factor) = match strategy.user_treatment {
                    UserTreatment::Existing(adjustment) => {
                        let factor = match adjustment {
                            UserAdjustment::None => 1.0,
                            UserAdjustment::Correction => correction,
                            UserAdjustment::Sigmoid => sigmoid,
                            UserAdjustment::Entropy => entropy,
                            UserAdjustment::SigmoidCorrection => sigmoid * correction,
                            UserAdjustment::EntropyCorrection => entropy * correction,
                        };
                        (user[index], factor)
                    }
                    UserTreatment::Smoothed(smoothing) => {
                        let confidence = match smoothing {
                            Smoothing::Correction => correction,
                            Smoothing::Fixed => args.fixed_g,
                            Smoothing::Sigmoid => sigmoid,
                            Smoothing::Entropy => entropy,
                        };
                        (
                            confidence * user[index] + (1.0 - confidence) / count as f64,
                            1.0,
                        )
                    }
                };
                lambda * term(ai[index]) + (1.0 - lambda) * term(user_probability) * factor
            };
            score(*right).total_cmp(&score(*left))
        });
        if let Some(rank) = ranked
            .iter()
            .position(|(_, candidate)| candidate.text == group.correct)
        {
            reciprocal_rank_sum += 1.0 / (rank + 1) as f64;
            top1_count += usize::from(rank == 0);
        }
    }
    EvalResult {
        mrr: reciprocal_rank_sum / groups.len() as f64,
        top1: top1_count as f64 / groups.len() as f64,
    }
}

pub fn run(args: Args) -> Result<()> {
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
    let file =
        File::create(&args.output).with_context(|| format!("出力先: {}", args.output.display()))?;
    let mut out = BufWriter::new(file);
    write!(out, "lambda")?;
    for strategy in STRATEGIES {
        write!(out, "\t{}_MRR\t{}_top1", strategy.label, strategy.label)?;
    }
    writeln!(out)?;
    let steps = (1.0 / args.lambda_step).ceil() as usize;
    for step in 0..=steps {
        let lambda = (step as f64 * args.lambda_step).min(1.0);
        write!(out, "{lambda:.6}")?;
        for strategy in STRATEGIES {
            let result = evaluate(&groups, strategy, lambda, &args);
            write!(out, "\t{:.6}\t{:.6}", result.mrr, result.top1)?;
        }
        writeln!(out)?;
    }
    out.flush()?;
    eprintln!("完了: データ={}", args.output.display());
    Ok(())
}
