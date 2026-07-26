use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use clap::{Parser, ValueEnum};
use hf_hub::api::sync::ApiBuilder;
use skk_zenz_backend::zenz::{ZenzConfig, ZenzScorer};
use unicode_width::UnicodeWidthStr;

mod zenz_bench {
    pub(super) mod script;
}

use zenz_bench::script::{Command, EvalGroup, Operation, eval_groups, parse_script};

const MODEL_FILE: &str = "ggml-model-Q5_K_M.gguf";
const DEFAULT_SCRIPT: &str = include_str!("zenz-bench-cases.script");

#[derive(Clone, Copy, Debug, ValueEnum)]
enum ModelSize {
    Xsmall,
    Small,
    Both,
}

impl ModelSize {
    fn repo_id(self) -> Option<&'static str> {
        match self {
            Self::Xsmall => Some("Miwa-Keita/zenz-v3.2-xsmall-gguf"),
            Self::Small => Some("Miwa-Keita/zenz-v3.2-small-gguf"),
            Self::Both => None,
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum Mode {
    Baseline,
    Speculative,
}

impl Mode {
    const ALL: [Self; 2] = [Self::Baseline, Self::Speculative];

    const fn name(self) -> &'static str {
        match self {
            Self::Baseline => "baseline",
            Self::Speculative => "speculative",
        }
    }
}

#[derive(Debug, Default)]
struct StepResult {
    elapsed: Duration,
    loss: Option<f64>,
    output_tokens: usize,
    reused_tokens: usize,
    decoded_tokens: usize,
}

#[derive(Debug)]
struct RunResult {
    steps: Vec<StepResult>,
    elapsed: Duration,
}

#[derive(Debug, Parser)]
#[command(about = "zenz v3.2 の baseline/speculative スコアリングを操作列で比較する")]
struct Args {
    /// Hugging Face から取得するモデルサイズ
    #[arg(long, value_enum, default_value_t = ModelSize::Both)]
    model: ModelSize,

    /// ローカル GGUF。指定時は --model を無視する
    #[arg(long)]
    model_path: Option<PathBuf>,

    /// 操作スクリプト。省略時は crate 同梱の cases.script を使う
    #[arg(long)]
    script: Option<PathBuf>,

    /// スクリプト全体の計測回数（各方式につき1回のウォームアップを別途実行）
    #[arg(long, default_value_t = 10)]
    runs: usize,

    /// 比較する llama.cpp の推論スレッド数（カンマ区切り、省略時は自動）
    #[arg(long, value_delimiter = ',')]
    threads: Vec<i32>,

    /// 比較するcontext size（カンマ区切り）
    #[arg(long, value_delimiter = ',', default_value = "1024")]
    context_sizes: Vec<u32>,

    /// GPU へoffloadするレイヤー数（vulkan feature時のみ有効）
    #[arg(long, default_value_t = 999)]
    gpu_layers: u32,
}

fn main() -> Result<()> {
    let args = Args::parse();
    if args.runs == 0 {
        bail!("--runs は1以上にしてください");
    }
    if args.context_sizes.contains(&0) {
        bail!("--context-sizes は1以上にしてください");
    }
    if args.threads.iter().any(|threads| *threads <= 0) {
        bail!("--threads は1以上にしてください");
    }
    let operations = load_script(args.script.as_deref())?;

    for (label, path) in resolve_models(&args)? {
        benchmark_model(&args, &operations, label, &path)?;
    }
    Ok(())
}

fn load_script(path: Option<&Path>) -> Result<Vec<Operation>> {
    let contents = match path {
        Some(path) => fs::read_to_string(path)
            .with_context(|| format!("スクリプトを読めません: {}", path.display()))?,
        None => DEFAULT_SCRIPT.to_owned(),
    };
    parse_script(&contents)
}

fn resolve_models(args: &Args) -> Result<Vec<(&'static str, PathBuf)>> {
    if let Some(path) = &args.model_path {
        return Ok(vec![("local", path.clone())]);
    }
    let sizes: &[ModelSize] = match args.model {
        ModelSize::Xsmall => &[ModelSize::Xsmall],
        ModelSize::Small => &[ModelSize::Small],
        ModelSize::Both => &[ModelSize::Xsmall, ModelSize::Small],
    };
    let api = ApiBuilder::new()
        .with_progress(true)
        .build()
        .context("Hugging Face API の初期化に失敗しました")?;
    sizes
        .iter()
        .map(|size| {
            let repo = size.repo_id().expect("Both is expanded above");
            let path = api
                .model(repo.to_owned())
                .get(MODEL_FILE)
                .with_context(|| format!("{repo}/{MODEL_FILE} の取得に失敗しました"))?;
            let label = match size {
                ModelSize::Xsmall => "xsmall",
                ModelSize::Small => "small",
                ModelSize::Both => unreachable!(),
            };
            Ok((label, path))
        })
        .collect()
}

fn benchmark_model(args: &Args, operations: &[Operation], label: &str, path: &Path) -> Result<()> {
    println!("\n== zenz-v3.2-{label} ==");
    println!("model: {}", path.display());

    let thread_counts: Vec<Option<i32>> = if args.threads.is_empty() {
        vec![None]
    } else {
        args.threads.iter().copied().map(Some).collect()
    };
    for context_size in &args.context_sizes {
        for threads in &thread_counts {
            benchmark_configuration(args, operations, path, *context_size, *threads)?;
        }
    }
    Ok(())
}

fn benchmark_configuration(
    args: &Args,
    operations: &[Operation],
    path: &Path,
    context_size: u32,
    threads: Option<i32>,
) -> Result<()> {
    println!(
        "\n## context_size={context_size} threads={}",
        threads.map_or_else(|| "auto".to_owned(), |value| value.to_string())
    );
    let mut config = ZenzConfig::new(path);
    config.context_size = context_size;
    config.parallel_sequences = eval_groups(operations)
        .iter()
        .map(|group| group.outputs.len() as u32 + 1)
        .max()
        .unwrap_or(1);
    config.threads = threads;
    config.gpu_layers = args.gpu_layers;
    let load_started = Instant::now();
    let mut scorer = ZenzScorer::load(&config)?;
    let rss = resident_memory_mib()
        .map(|mib| format!(" rss={mib:.1} MiB"))
        .unwrap_or_default();
    println!("load: {:.2} ms{rss}", milliseconds(load_started.elapsed()));

    for mode in Mode::ALL {
        let _ = run_script(&mut scorer, operations, mode)?;
        let mut runs = Vec::with_capacity(args.runs);
        for _ in 0..args.runs {
            runs.push(run_script(&mut scorer, operations, mode)?);
        }
        print_statistics(mode, operations, &runs);
    }
    benchmark_candidate_batches(&mut scorer, operations, args.runs)?;
    Ok(())
}

fn run_script(scorer: &mut ZenzScorer, operations: &[Operation], mode: Mode) -> Result<RunResult> {
    scorer.clear();
    let run_started = Instant::now();
    let mut state = ScriptState::default();
    let mut steps = Vec::with_capacity(operations.len());

    for operation in operations {
        let started = Instant::now();
        let mut result = match &operation.command {
            Command::Context(value) => {
                scorer.clear();
                state.context.clone_from(value);
                state.input.clear();
                if matches!(mode, Mode::Speculative) {
                    prepare(scorer, &state)?
                } else {
                    StepResult::default()
                }
            }
            Command::Input(value) => {
                state.input.clone_from(value);
                if matches!(mode, Mode::Speculative) {
                    prepare(scorer, &state)?
                } else {
                    StepResult::default()
                }
            }
            Command::Eval(output) => score_output(scorer, &state, output)?,
        };
        result.elapsed = started.elapsed();
        steps.push(result);
    }
    Ok(RunResult {
        steps,
        elapsed: run_started.elapsed(),
    })
}

#[derive(Default)]
struct ScriptState {
    context: String,
    input: String,
}

fn prepare(scorer: &mut ZenzScorer, state: &ScriptState) -> Result<StepResult> {
    let score = scorer.prepare(&state.context, &state.input)?;
    Ok(StepResult {
        reused_tokens: score.reused_tokens,
        decoded_tokens: score.decoded_tokens,
        ..StepResult::default()
    })
}

fn score_output(scorer: &mut ZenzScorer, state: &ScriptState, output: &str) -> Result<StepResult> {
    let score = scorer.score(&state.context, &state.input, output)?;
    Ok(StepResult {
        loss: Some(score.loss),
        output_tokens: score.token_count,
        reused_tokens: score.reused_tokens,
        decoded_tokens: score.decoded_tokens,
        ..StepResult::default()
    })
}

fn benchmark_candidate_batches(
    scorer: &mut ZenzScorer,
    operations: &[Operation],
    runs: usize,
) -> Result<()> {
    let groups = eval_groups(operations);
    println!("\n-- candidate sequential/batch --");
    for group in groups {
        let sequential_warmup = evaluate_sequential(scorer, &group)?;
        let batch_warmup = evaluate_batch(scorer, &group)?;
        let max_loss_delta = sequential_warmup
            .iter()
            .zip(&batch_warmup)
            .map(|(left, right)| (left.loss - right.loss).abs())
            .fold(0.0, f64::max);

        let mut sequential_samples = Vec::with_capacity(runs);
        let mut batch_samples = Vec::with_capacity(runs);
        for _ in 0..runs {
            let started = Instant::now();
            evaluate_sequential(scorer, &group)?;
            sequential_samples.push(milliseconds(started.elapsed()));

            let started = Instant::now();
            evaluate_batch(scorer, &group)?;
            batch_samples.push(milliseconds(started.elapsed()));
        }
        let sequential_mean = mean(&sequential_samples);
        let batch_mean = mean(&batch_samples);
        println!(
            "line {:>2} candidates={} sequential={:>7.2} ms batch={:>7.2} ms speedup={:>5.2}x loss_delta={:.2e}",
            group.line,
            group.outputs.len(),
            sequential_mean,
            batch_mean,
            sequential_mean / batch_mean,
            max_loss_delta,
        );
    }
    Ok(())
}

fn evaluate_sequential(
    scorer: &mut ZenzScorer,
    group: &EvalGroup,
) -> Result<Vec<skk_zenz_backend::zenz::Score>> {
    scorer.clear();
    scorer.prepare(&group.context, &group.input)?;
    group
        .outputs
        .iter()
        .map(|output| scorer.score(&group.context, &group.input, output))
        .collect()
}

fn evaluate_batch(
    scorer: &mut ZenzScorer,
    group: &EvalGroup,
) -> Result<Vec<skk_zenz_backend::zenz::Score>> {
    scorer.clear();
    scorer.prepare(&group.context, &group.input)?;
    let outputs: Vec<_> = group.outputs.iter().map(String::as_str).collect();
    scorer.score_batch(&group.context, &group.input, &outputs)
}

fn print_statistics(mode: Mode, operations: &[Operation], runs: &[RunResult]) {
    println!("\n-- {} --", mode.name());
    for (index, operation) in operations.iter().enumerate() {
        let samples: Vec<f64> = runs
            .iter()
            .map(|run| milliseconds(run.steps[index].elapsed))
            .collect();
        let step = &runs[0].steps[index];
        let loss = step
            .loss
            .map(|loss| {
                format!(
                    " loss={loss:.4} mean_nll={:.4}",
                    loss / step.output_tokens as f64
                )
            })
            .unwrap_or_default();
        println!(
            "line {:>2} {} mean={:>7.2} ms p50={:>7.2} p95={:>7.2} min={:>7.2} max={:>7.2} reused={} decoded={}{}",
            operation.line,
            pad_right(&operation.label(), 38),
            mean(&samples),
            percentile(&samples, 0.50),
            percentile(&samples, 0.95),
            samples.iter().copied().fold(f64::INFINITY, f64::min),
            samples.iter().copied().fold(f64::NEG_INFINITY, f64::max),
            step.reused_tokens,
            step.decoded_tokens,
            loss,
        );
    }
    let script_samples: Vec<f64> = runs.iter().map(|run| milliseconds(run.elapsed)).collect();
    println!(
        "script total: mean={:.2} ms p50={:.2} p95={:.2} min={:.2} max={:.2}",
        mean(&script_samples),
        percentile(&script_samples, 0.50),
        percentile(&script_samples, 0.95),
        script_samples.iter().copied().fold(f64::INFINITY, f64::min),
        script_samples
            .iter()
            .copied()
            .fold(f64::NEG_INFINITY, f64::max),
    );
}

fn pad_right(value: &str, width: usize) -> String {
    let padding = width.saturating_sub(value.width());
    format!("{value}{}", " ".repeat(padding))
}

fn mean(values: &[f64]) -> f64 {
    values.iter().sum::<f64>() / values.len() as f64
}

fn percentile(values: &[f64], percentile: f64) -> f64 {
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let index = ((sorted.len() - 1) as f64 * percentile).ceil() as usize;
    sorted[index]
}

fn milliseconds(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1_000.0
}

fn resident_memory_mib() -> Option<f64> {
    let status = fs::read_to_string("/proc/self/status").ok()?;
    let line = status.lines().find(|line| line.starts_with("VmRSS:"))?;
    let kib = line.split_whitespace().nth(1)?.parse::<f64>().ok()?;
    Some(kib / 1024.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_script_is_valid() {
        let operations = parse_script(DEFAULT_SCRIPT).unwrap();
        assert!(operations.len() > 1);
    }

    #[test]
    fn argument_keeps_spaces_after_first_separator() {
        let operations = parse_script("context 前  後\ninput かな\neval 仮名").unwrap();
        assert!(matches!(
            &operations[0].command,
            Command::Context(value) if value == "前  後"
        ));
    }

    #[test]
    fn padding_uses_terminal_cell_width() {
        assert_eq!(pad_right("input かな", 12), "input かな  ");
        assert_eq!(
            UnicodeWidthStr::width(pad_right("input かな", 12).as_str()),
            12
        );
    }

    #[test]
    fn parses_comparison_axes() {
        let args = Args::try_parse_from([
            "skk-zenz-bench",
            "--threads",
            "1,2,4",
            "--context-sizes",
            "256,512",
        ])
        .unwrap();
        assert_eq!(args.threads, [1, 2, 4]);
        assert_eq!(args.context_sizes, [256, 512]);
    }

    #[test]
    fn groups_consecutive_candidates_for_batch_evaluation() {
        let operations =
            parse_script("context 前文\ninput かな\neval 仮名\neval 加奈\ninput つぎ\neval 次")
                .unwrap();
        let groups = eval_groups(&operations);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].context, "前文");
        assert_eq!(groups[0].input, "かな");
        assert_eq!(groups[0].outputs, ["仮名", "加奈"]);
        assert_eq!(groups[1].outputs, ["次"]);
    }
}
