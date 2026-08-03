use std::io::Write;
use std::path::PathBuf;

use anyhow::{Result, bail};
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
}

#[derive(Clone, Copy)]
struct Outcome {
    reciprocal_rank: f64,
    top1: bool,
}

fn evaluate(groups: &[ScoredGroup], normalization: LengthNormalization) -> Vec<Outcome> {
    groups
        .iter()
        .map(|group| {
            let mut ranked: Vec<_> = group.candidates.iter().collect();
            ranked.sort_by(|left, right| {
                length_normalized_loss(left, normalization)
                    .total_cmp(&length_normalized_loss(right, normalization))
            });
            let rank = ranked
                .iter()
                .position(|candidate| candidate.text == group.correct);
            Outcome {
                reciprocal_rank: rank.map_or(0.0, |rank| 1.0 / (rank + 1) as f64),
                top1: rank == Some(0),
            }
        })
        .collect()
}

fn summarize<'a>(outcomes: impl Iterator<Item = &'a Outcome>) -> EvalResult {
    let mut count = 0usize;
    let mut reciprocal_rank_sum = 0.0;
    let mut top1_count = 0usize;
    for outcome in outcomes {
        count += 1;
        reciprocal_rank_sum += outcome.reciprocal_rank;
        top1_count += usize::from(outcome.top1);
    }
    EvalResult {
        mrr: reciprocal_rank_sum / count as f64,
        top1: top1_count as f64 / count as f64,
    }
}

fn normalization_fields(normalization: LengthNormalization) -> (&'static str, f64) {
    match normalization {
        LengthNormalization::None => ("none", 0.0),
        LengthNormalization::TokenPower(exponent) => ("token", exponent),
        LengthNormalization::CharacterPower(exponent) => ("character", exponent),
    }
}

fn correct_length(group: &ScoredGroup, token_length: bool) -> Option<usize> {
    let candidate = group
        .candidates
        .iter()
        .find(|candidate| candidate.text == group.correct)?;
    Some(if token_length {
        candidate.token_count
    } else {
        candidate.text.chars().count()
    })
}

fn length_bucket(length: usize) -> &'static str {
    match length {
        0 | 1 => "1",
        2 => "2",
        _ => "3+",
    }
}

fn write_length_table(
    out: &mut impl Write,
    evaluations: &[(LengthNormalization, Vec<Outcome>)],
    groups: &[ScoredGroup],
    token_length: bool,
) -> Result<()> {
    let length_kind = if token_length { "token" } else { "character" };
    writeln!(out, "# by_{length_kind}_length")?;
    writeln!(
        out,
        "{:<13} {:>5} {:>6} {:>6} {:>8} {:>10}",
        "normalization", "alpha", "length", "count", "MRR", "top1_rate"
    )?;
    for (normalization, outcomes) in evaluations {
        let (name, alpha) = normalization_fields(*normalization);
        for bucket in ["1", "2", "3+"] {
            let selected: Vec<_> = outcomes
                .iter()
                .zip(groups)
                .filter(|(_, group)| {
                    correct_length(group, token_length)
                        .is_some_and(|length| length_bucket(length) == bucket)
                })
                .map(|(outcome, _)| outcome)
                .collect();
            if selected.is_empty() {
                continue;
            }
            let result = summarize(selected.iter().copied());
            writeln!(
                out,
                "{name:<13} {alpha:>5.2} {bucket:>6} {:>6} {:>8.6} {:>10.6}",
                selected.len(),
                result.mrr,
                result.top1
            )?;
        }
    }
    writeln!(out)?;
    Ok(())
}

pub fn run(args: Args) -> Result<()> {
    let mut groups = load_scored(&args.input)?;
    let total_count = groups.len();
    if args.repeated_only {
        retain_repeated_groups(&mut groups);
    }
    if groups.is_empty() {
        bail!("評価対象が空です");
    }
    eprintln!("グループ数: {}/{}", groups.len(), total_count);

    let normalizations = [
        LengthNormalization::None,
        LengthNormalization::TokenPower(0.25),
        LengthNormalization::TokenPower(0.5),
        LengthNormalization::TokenPower(0.75),
        LengthNormalization::TokenPower(1.0),
        LengthNormalization::CharacterPower(0.25),
        LengthNormalization::CharacterPower(0.5),
        LengthNormalization::CharacterPower(0.75),
        LengthNormalization::CharacterPower(1.0),
    ];
    let evaluations: Vec<_> = normalizations
        .into_iter()
        .map(|normalization| (normalization, evaluate(&groups, normalization)))
        .collect();
    let baseline = &evaluations[0].1;

    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    writeln!(out, "# overall")?;
    writeln!(
        out,
        "{:<13} {:>5} {:>6} {:>8} {:>10} {:>8} {:>8} {:>9}",
        "normalization", "alpha", "count", "MRR", "top1_rate", "improved", "worsened", "unchanged"
    )?;
    for (normalization, outcomes) in &evaluations {
        let result = summarize(outcomes.iter());
        let (name, alpha) = normalization_fields(*normalization);
        let mut improved = 0usize;
        let mut worsened = 0usize;
        let mut unchanged = 0usize;
        for (baseline, outcome) in baseline.iter().zip(outcomes) {
            match (baseline.top1, outcome.top1) {
                (false, true) => improved += 1,
                (true, false) => worsened += 1,
                _ => unchanged += 1,
            }
        }
        writeln!(
            out,
            "{name:<13} {alpha:>5.2} {:>6} {:>8.6} {:>10.6} {improved:>8} {worsened:>8} {unchanged:>9}",
            outcomes.len(),
            result.mrr,
            result.top1
        )?;
    }
    writeln!(out)?;
    write_length_table(&mut out, &evaluations, &groups, true)?;
    write_length_table(&mut out, &evaluations, &groups, false)?;
    out.flush()?;
    Ok(())
}
