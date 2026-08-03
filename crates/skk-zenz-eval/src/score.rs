use std::collections::HashMap;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::Parser;
use skk_zenz_backend::zenz::{ModelSource, ZenzConfig, ZenzScorer};
use skk_zenz_backend::{
    BackendConfig, CandidateOrderConfig, CandidateSource, ConversionCore, MemoryUserDictionary,
};

use crate::history::read_history;
use crate::scored::{ScoredCandidate, ScoredGroup};

#[derive(Debug, Parser)]
pub struct Args {
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

pub fn run(args: Args) -> Result<()> {
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

        if let Some(chosen) = candidates.iter().find(|c| c.output == correct_output) {
            core.record_usage_at(chosen, now)?;
        }

        let group = ScoredGroup {
            context: entry.context.clone(),
            input: entry.input.clone(),
            okuri: entry.okuri.clone(),
            correct: correct_output,
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
