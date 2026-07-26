use std::fs;
use std::io::{self, BufRead};
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, ValueEnum};
use skk_zenz_backend::{Backend, BackendConfig, KanaMode, RomKanaRule};
#[cfg(test)]
use skk_zenz_backend::{Key, KeyEvent, Modifiers};

mod cli_script;

#[cfg(test)]
use cli_script::parse_key_event;
use cli_script::run_script;

#[derive(Debug, Parser)]
#[command(about = "skk-zenz backendのコア機能を操作列で試す")]
struct Args {
    /// backend設定ファイル
    #[arg(long)]
    config: Option<PathBuf>,

    /// 操作スクリプト。省略時は標準入力から読む
    #[arg(long)]
    script: Option<PathBuf>,

    /// モデルをロードせず、ローマ字かな変換だけを実行
    #[arg(long)]
    romaji: Option<String>,

    #[arg(long, value_enum, default_value_t = CliKanaMode::Hiragana)]
    kana_mode: CliKanaMode,
}

#[derive(Clone, Copy, Debug, Default, ValueEnum)]
enum CliKanaMode {
    #[default]
    Hiragana,
    Katakana,
    HankakuKatakana,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let config = match args.config {
        Some(path) => BackendConfig::from_toml_file(path)?,
        None => BackendConfig::from_xdg()?,
    };
    if let Some(romaji) = args.romaji {
        let rule = RomKanaRule::load(&config.rom_kana_rule, &config.rule_search_paths)?;
        let conversion = rule.convert(&romaji, args.kana_mode.into());
        println!(
            "output={:?} preedit={:?}",
            conversion.output, conversion.preedit
        );
        return Ok(());
    }
    let mut backend = Backend::load(config)?;
    let lines: Vec<String> = match args.script {
        Some(path) => fs::read_to_string(&path)
            .with_context(|| format!("スクリプトを読めません: {}", path.display()))?
            .lines()
            .map(str::to_owned)
            .collect(),
        None => io::stdin().lock().lines().collect::<io::Result<_>>()?,
    };
    run_script(&mut backend, &lines)
}

impl From<CliKanaMode> for KanaMode {
    fn from(mode: CliKanaMode) -> Self {
        match mode {
            CliKanaMode::Hiragana => Self::Hiragana,
            CliKanaMode::Katakana => Self::Katakana,
            CliKanaMode::HankakuKatakana => Self::HankakuKatakana,
        }
    }
}

#[cfg(test)]
mod input_tests {
    use super::*;

    const INPUT_CASES: &str = include_str!("../cases.input.script");

    #[test]
    fn parses_shifted_text_and_unmodified_base_separately() {
        assert_eq!(
            parse_key_event("! 1 shift").unwrap(),
            KeyEvent {
                text: Some("!".to_owned()),
                key: Key::Character('1'),
                modifiers: Modifiers::SHIFT
            }
        );
    }

    #[test]
    fn bundled_input_cases_have_valid_commands_and_keys() {
        for (index, line) in INPUT_CASES.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let (command, argument) = line.split_once(' ').unwrap_or((line, ""));
            match command {
                "key" => {
                    parse_key_event(argument)
                        .unwrap_or_else(|error| panic!("line {}: {error:#}", index + 1));
                }
                "context" | "reset" => {}
                "focus-out" => assert!(
                    matches!(argument, "commit" | "reset"),
                    "line {}: invalid focus-out argument",
                    index + 1
                ),
                other => panic!("line {}: unknown command {other:?}", index + 1),
            }
        }
    }
}
