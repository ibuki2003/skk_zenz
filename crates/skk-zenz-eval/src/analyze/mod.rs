use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

use crate::scored::{ScoredCandidate, ScoredGroup};

mod length;
mod mixture;

#[derive(Debug, Parser)]
pub struct Args {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// AIスコアとuserスコアを混合し、lambdaを変えて比較する
    Mixture(mixture::Args),
    /// AIスコア単独でlength normalizationを比較する
    Length(length::Args),
}

pub fn run(args: Args) -> Result<()> {
    match args.command {
        Command::Mixture(args) => mixture::run(args),
        Command::Length(args) => length::run(args),
    }
}

fn load_scored(path: &Path) -> Result<Vec<ScoredGroup>> {
    let file = File::open(path).with_context(|| format!("読めません: {}", path.display()))?;
    let reader = BufReader::new(file);
    let mut groups = Vec::new();
    for (i, line) in reader.lines().enumerate() {
        let line = line?;
        if line.is_empty() {
            continue;
        }
        let group =
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
enum LengthNormalization {
    None,
    TokenPower(f64),
    CharacterPower(f64),
}

fn length_normalized_loss(candidate: &ScoredCandidate, normalization: LengthNormalization) -> f64 {
    let length = match normalization {
        LengthNormalization::None => 1.0,
        LengthNormalization::TokenPower(exponent) => {
            (candidate.token_count.max(1) as f64).powf(exponent)
        }
        LengthNormalization::CharacterPower(exponent) => {
            (candidate.text.chars().count().max(1) as f64).powf(exponent)
        }
    };
    candidate.loss / length
}
