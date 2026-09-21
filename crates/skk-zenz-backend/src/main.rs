use std::fs;
use std::io::{self, BufRead, IsTerminal, Write};
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, ValueEnum};
use skk_zenz_backend::zenz::GenerationOptions;
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

    /// Zenzで候補を直接生成する読み
    #[arg(long)]
    generate: Option<String>,

    /// --generateで使用する左文脈
    #[arg(long, default_value = "")]
    context: String,

    /// --generateで候補の先頭に固定する文字列
    #[arg(long, default_value = "")]
    prefix: String,

    /// --generateで1ページに出力する候補数
    #[arg(long, default_value_t = 5)]
    page_size: usize,

    /// --generateのbeam幅
    #[arg(long, default_value_t = 8)]
    beam_width: usize,

    /// --generateで追加生成する最大token数
    #[arg(long, default_value_t = 32)]
    max_tokens: usize,
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
    let load_started = std::time::Instant::now();
    let mut backend = Backend::load(config)?;
    if let Some(input) = args.generate {
        let load_elapsed = load_started.elapsed();
        let options = GenerationOptions {
            beam_width: args.beam_width,
            max_tokens: args.max_tokens,
        };
        println!("load: {:.2} ms", load_elapsed.as_secs_f64() * 1_000.0);
        let mut offset = 0;
        let mut first = true;
        loop {
            let generate_started = std::time::Instant::now();
            let result = if first {
                first = false;
                backend.start_generation(
                    &args.context,
                    &input,
                    &args.prefix,
                    options,
                    args.page_size,
                )?
            } else {
                backend.continue_generation(args.page_size)?
            };
            let generate_elapsed = generate_started.elapsed();
            println!(
                "generate: {:.2} ms candidates={} expanded={} reused={} decoded={} finished={}",
                generate_elapsed.as_secs_f64() * 1_000.0,
                result.candidates.len(),
                result.expanded_hypotheses,
                result.reused_tokens,
                result.decoded_tokens,
                result.finished,
            );
            for candidate in &result.candidates {
                println!(
                    "{offset:>3} loss={:.4} tokens={} {}",
                    candidate.loss, candidate.token_count, candidate.text
                );
                offset += 1;
            }
            if result.finished || !io::stdin().is_terminal() {
                break;
            }
            print!("Enter: 次の候補 / q: 終了 > ");
            io::stdout().flush()?;
            let mut line = String::new();
            if io::stdin().read_line(&mut line)? == 0 || line.trim() == "q" {
                backend.cancel_generation();
                break;
            }
        }
        return Ok(());
    }
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
