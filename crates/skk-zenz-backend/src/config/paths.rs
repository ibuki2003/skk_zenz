use std::env;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

pub(super) fn config_dir() -> Result<PathBuf> {
    if let Some(path) = env::var_os("XDG_CONFIG_HOME") {
        return Ok(PathBuf::from(path).join("skk-zenz"));
    }
    Ok(home_dir()?.join(".config/skk-zenz"))
}

pub(super) fn data_home() -> Result<PathBuf> {
    if let Some(path) = env::var_os("XDG_DATA_HOME") {
        return Ok(PathBuf::from(path));
    }
    Ok(home_dir()?.join(".local/share"))
}

fn home_dir() -> Result<PathBuf> {
    env::var_os("HOME")
        .map(PathBuf::from)
        .context("HOMEが設定されていません")
}

pub(super) fn rule_search_paths(config_dir: &Path) -> Result<Vec<PathBuf>> {
    let libskk_config = config_dir
        .parent()
        .context("設定ディレクトリの親がありません")?
        .join("libskk/rules");
    let mut paths = vec![config_dir.join("rules"), libskk_config];
    paths.extend(env!("SKK_RULES_PATH").split(':').map(PathBuf::from));
    Ok(paths)
}

pub(super) fn resolve_path(base: &Path, path: PathBuf) -> PathBuf {
    if path.is_absolute() {
        path
    } else {
        base.join(path)
    }
}
