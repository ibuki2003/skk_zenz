use std::fs;
use std::io::{self, BufRead};
use std::path::PathBuf;
use std::time::Instant;

use anyhow::{Context, Result, bail};
use clap::{Parser, ValueEnum};
use skk_zenz_backend::{
    Backend, BackendConfig, Candidate, InputEvent, KanaMode, Key, KeyEvent, Modifiers, RomKanaRule,
};

#[derive(Debug, Parser)]
#[command(about = "skk-zenz backendのコア機能を操作列で試す")]
struct Args {
    /// backend設定ファイル
    #[arg(long, default_value = "skk-zenz.toml")]
    config: PathBuf,

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
    let config = BackendConfig::from_toml_file(args.config)?;
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

fn run_script(backend: &mut Backend, lines: &[String]) -> Result<()> {
    let mut context = String::new();
    let mut input = String::new();
    let mut okuri: Option<String> = None;
    let mut candidates: Vec<Candidate> = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (command, argument) = line.split_once(' ').unwrap_or((line, ""));
        match command {
            "context" => {
                backend.clear_cache();
                context = argument.to_owned();
                backend.handle_event(InputEvent::SetContext(context.clone()))?;
                input.clear();
                okuri = None;
                candidates.clear();
            }
            "input" => input = argument.to_owned(),
            "prepare" => {
                let started = Instant::now();
                let score = backend.prepare(&context, &input)?;
                println!(
                    "prepare {:.2} ms reused={} decoded={}",
                    started.elapsed().as_secs_f64() * 1_000.0,
                    score.reused_tokens,
                    score.decoded_tokens
                );
            }
            "lookup" => {
                let started = Instant::now();
                okuri = (!argument.is_empty()).then(|| argument.to_owned());
                candidates = backend.lookup(&context, &input, okuri.as_deref())?;
                println!(
                    "lookup {:.2} ms candidates={}",
                    started.elapsed().as_secs_f64() * 1_000.0,
                    candidates.len()
                );
                for (candidate_index, candidate) in candidates.iter().enumerate() {
                    println!(
                        "{candidate_index:>3} {:?} loss={:.4} tokens={} recent={:?} uses={} {}{}",
                        candidate.source,
                        candidate.loss,
                        candidate.token_count,
                        candidate.recency,
                        candidate.use_count,
                        candidate.output,
                        candidate
                            .annotation
                            .as_deref()
                            .map(|annotation| format!(";{annotation}"))
                            .unwrap_or_default()
                    );
                }
            }
            "select" => {
                let candidate_index: usize =
                    argument.parse().context("selectの引数は候補番号です")?;
                let candidate = candidates
                    .get(candidate_index)
                    .context("selectの候補番号が範囲外です")?;
                backend.select(candidate)?;
            }
            "purge" => {
                let candidate_index: usize =
                    argument.parse().context("purgeの引数は候補番号です")?;
                let candidate = candidates
                    .get(candidate_index)
                    .context("purgeの候補番号が範囲外です")?;
                println!("purged={}", backend.purge(candidate)?);
            }
            "register" => backend.register(&input, okuri.as_deref(), argument, None)?,
            "key" => {
                let event = parse_key_event(argument)
                    .with_context(|| format!("{}行目のkeyを解釈できません", index + 1))?;
                let update = backend.handle_event(InputEvent::Key(event))?;
                println!(
                    "consumed={} preedit={:?} commit={:?} candidates={}",
                    update.consumed,
                    update.preedit,
                    update.commit,
                    update.candidates.len()
                );
            }
            "focus-out" => {
                let commit = match argument {
                    "commit" => true,
                    "reset" => false,
                    _ => bail!("{}行目: focus-outはcommitまたはresetです", index + 1),
                };
                let update = backend.handle_event(InputEvent::FocusOut { commit })?;
                println!("preedit={:?} commit={:?}", update.preedit, update.commit);
            }
            "reset" => {
                backend.handle_event(InputEvent::Reset)?;
            }
            "kana" | "katakana" | "hankaku-katakana" => {
                let mode = match command {
                    "kana" => KanaMode::Hiragana,
                    "katakana" => KanaMode::Katakana,
                    "hankaku-katakana" => KanaMode::HankakuKatakana,
                    _ => unreachable!(),
                };
                let conversion = backend.convert_romaji(argument, mode);
                println!(
                    "output={:?} preedit={:?}",
                    conversion.output, conversion.preedit
                );
            }
            _ => bail!("{}行目: 未知のコマンドです: {command}", index + 1),
        }
    }
    Ok(())
}

fn parse_key_event(argument: &str) -> Result<KeyEvent> {
    let mut parts = argument.split_whitespace();
    let text = parts.next().context("textがありません")?;
    let base = parts.next().context("base keyがありません")?;
    let modifiers = parts.next().unwrap_or("-");
    if parts.next().is_some() {
        bail!("keyの引数が多すぎます");
    }
    let text = match text {
        "-" => None,
        "SPC" => Some(" ".to_owned()),
        _ => Some(text.to_owned()),
    };
    let key = if base.chars().count() == 1 {
        Key::Character(base.chars().next().expect("長さを確認済み"))
    } else {
        let mut event = KeyEvent::named(base, parse_modifiers(modifiers)?);
        event.text = text;
        return Ok(event);
    };
    Ok(KeyEvent {
        text,
        key,
        modifiers: parse_modifiers(modifiers)?,
    })
}

fn parse_modifiers(value: &str) -> Result<Modifiers> {
    if value == "-" {
        return Ok(Modifiers::empty());
    }
    value
        .split(',')
        .try_fold(Modifiers::empty(), |mut result, modifier| {
            result |= match modifier {
                "shift" => Modifiers::SHIFT,
                "control" => Modifiers::CONTROL,
                "alt" => Modifiers::ALT,
                "meta" => Modifiers::META,
                "super" => Modifiers::SUPER,
                "hyper" => Modifiers::HYPER,
                "mod5" => Modifiers::MOD5,
                _ => bail!("未知のmodifierです: {modifier}"),
            };
            Ok(result)
        })
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
