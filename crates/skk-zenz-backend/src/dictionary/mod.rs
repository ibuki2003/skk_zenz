mod skk_file;
mod user;

use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::{Context, Result};

use crate::candidate::DictionaryCandidate;

pub(crate) use skk_file::SkkFileDictionary;
pub use user::{MemoryUserDictionary, UserDictionary};
pub(crate) use user::{SqliteUserDictionary, unix_timestamp};

pub(crate) trait Dictionary {
    fn lookup(&self, input: &str, okuri: Option<&str>) -> Result<Vec<DictionaryCandidate>>;
    fn complete(&self, prefix: &str) -> Result<Vec<String>>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DictionaryFingerprint {
    path: PathBuf,
    size: u64,
    modified: SystemTime,
}

pub(crate) fn system_dictionary_fingerprints(
    paths: &[impl AsRef<Path>],
) -> Result<Vec<DictionaryFingerprint>> {
    paths
        .iter()
        .map(|path| {
            let path = path.as_ref();
            let metadata = fs::metadata(path)
                .with_context(|| format!("system辞書の情報を読めません: {}", path.display()))?;
            let modified = metadata
                .modified()
                .with_context(|| format!("system辞書の更新日時を読めません: {}", path.display()))?;
            Ok(DictionaryFingerprint {
                path: path.to_owned(),
                size: metadata.len(),
                modified,
            })
        })
        .collect()
}

pub(crate) fn open_system_dictionaries(
    paths: &[impl AsRef<Path>],
) -> Result<Vec<SkkFileDictionary>> {
    paths
        .iter()
        .map(|path| SkkFileDictionary::load(path.as_ref()))
        .collect()
}
