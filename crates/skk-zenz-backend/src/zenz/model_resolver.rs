use anyhow::{Context, Result};
use hf_hub::api::sync::ApiBuilder;
use hf_hub::{Cache, Repo};

use super::worker::{ModelDownloadEvent, ModelDownloadEventHandler};
use super::{ModelSource, ZenzConfig};

pub(crate) fn parse_huggingface_reference(reference: &str) -> Result<(String, &str)> {
    let mut fields = reference.splitn(3, '/');
    let owner = fields.next().unwrap_or_default();
    let repository = fields.next().unwrap_or_default();
    let filename = fields.next().unwrap_or_default();
    if owner.is_empty() || repository.is_empty() || filename.is_empty() {
        anyhow::bail!("huggingface は owner/repository/path 形式で指定してください: {reference}");
    }
    Ok((format!("{owner}/{repository}"), filename))
}

pub(super) fn resolve_model(
    mut config: ZenzConfig,
    model_events: &ModelDownloadEventHandler,
) -> Result<ZenzConfig> {
    let ModelSource::HuggingFace(reference) = &config.model else {
        return Ok(config);
    };
    let reference = reference.clone();
    let (repo_id, filename) = parse_huggingface_reference(&reference)?;
    let repo = Repo::model(repo_id);
    let cache = Cache::default();
    if let Some(path) = cache.repo(repo.clone()).get(filename) {
        config.model = ModelSource::Local(path);
        return Ok(config);
    }

    model_events(ModelDownloadEvent::Started {
        reference: reference.clone(),
    });
    let result = ApiBuilder::from_cache(cache)
        .with_progress(false)
        .build()
        .context("Hugging Face API の初期化に失敗しました")
        .and_then(|api| {
            api.repo(repo)
                .download(filename)
                .with_context(|| format!("{reference} の取得に失敗しました"))
        });
    match result {
        Ok(path) => {
            model_events(ModelDownloadEvent::Completed { reference });
            config.model = ModelSource::Local(path);
            Ok(config)
        }
        Err(error) => {
            model_events(ModelDownloadEvent::Failed {
                reference,
                error: format!("{error:#}"),
            });
            Err(error)
        }
    }
}
