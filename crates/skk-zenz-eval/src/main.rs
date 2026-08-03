use anyhow::Result;
use clap::{Parser, Subcommand};

mod analyze;
mod history;
mod score;
mod scored;

#[derive(Debug, Parser)]
#[command(about = "skk-zenz リランキング評価ツール")]
struct Args {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// conversion_history から候補を生成し AI スコアを付けて JSONL に書く
    Score(score::Args),
    /// scored JSONL を読み、各戦略で MRR / top-1 rate を計算する
    Analyze(analyze::Args),
}

fn main() -> Result<()> {
    match Args::parse().command {
        Command::Score(args) => score::run(args),
        Command::Analyze(args) => analyze::run(args),
    }
}
