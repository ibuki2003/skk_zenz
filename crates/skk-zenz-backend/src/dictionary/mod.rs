mod skk_file;
mod user;

use std::path::Path;

use anyhow::Result;

use crate::candidate::DictionaryCandidate;

pub(crate) use skk_file::SkkFileDictionary;
pub(crate) use user::UserDictionary;

pub(crate) trait Dictionary {
    fn lookup(&self, input: &str, okuri: Option<&str>) -> Result<Vec<DictionaryCandidate>>;
}

pub(crate) fn open_system_dictionaries(
    paths: &[impl AsRef<Path>],
) -> Result<Vec<SkkFileDictionary>> {
    paths
        .iter()
        .map(|path| SkkFileDictionary::load(path.as_ref()))
        .collect()
}
