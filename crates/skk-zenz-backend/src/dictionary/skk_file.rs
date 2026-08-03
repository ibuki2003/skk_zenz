use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use encoding_rs::EUC_JP;
use iconv_native::decode;

use super::Dictionary;
use crate::candidate::{CandidateSource, DictionaryCandidate};
use crate::okuri::dictionary_key;

pub(crate) struct SkkFileDictionary {
    #[allow(dead_code)]
    path: PathBuf,
    okuri_ari: HashMap<String, Vec<Entry>>,
    okuri_nasi: HashMap<String, Vec<Entry>>,
}

#[derive(Clone)]
struct Entry {
    text: String,
    annotation: Option<String>,
}

impl SkkFileDictionary {
    pub(crate) fn load(path: &Path) -> Result<Self> {
        let bytes = fs::read(path)
            .with_context(|| format!("system辞書を読めません: {}", path.display()))?;
        let contents = decode_dictionary(&bytes).with_context(|| {
            format!("system辞書の文字コードを変換できません: {}", path.display())
        })?;
        Self::parse(path.to_owned(), &contents)
    }

    fn parse(path: PathBuf, contents: &str) -> Result<Self> {
        let mut dictionary = Self {
            path,
            okuri_ari: HashMap::new(),
            okuri_nasi: HashMap::new(),
        };
        let mut okuri_ari = None;
        for (index, line) in contents.lines().enumerate() {
            if line.starts_with(";; okuri-ari entries.") {
                okuri_ari = Some(true);
                continue;
            }
            if line.starts_with(";; okuri-nasi entries.") {
                okuri_ari = Some(false);
                continue;
            }
            if line.is_empty() || line.starts_with(';') {
                continue;
            }
            let Some(is_okuri_ari) = okuri_ari else {
                continue;
            };
            let Some((midasi, candidates)) = line.split_once(' ') else {
                bail!("system辞書の{}行目に見出し区切りがありません", index + 1);
            };
            let entries = parse_candidates(candidates)
                .with_context(|| format!("system辞書の{}行目を解釈できません", index + 1))?;
            if is_okuri_ari {
                dictionary.okuri_ari.insert(midasi.to_owned(), entries);
            } else {
                dictionary.okuri_nasi.insert(midasi.to_owned(), entries);
            }
        }
        Ok(dictionary)
    }
}

fn decode_dictionary(bytes: &[u8]) -> Result<String> {
    if let Ok(contents) = std::str::from_utf8(bytes) {
        return Ok(contents.to_owned());
    }
    if declares_euc_jis_2004(bytes) {
        return decode(bytes, "EUC-JISX0213")
            .map_err(|error| anyhow::anyhow!("EUC-JIS-2004として解釈できません: {error}"));
    }
    let (contents, _, malformed) = EUC_JP.decode(bytes);
    if malformed {
        bail!("UTF-8/EUC-JPとして解釈できません");
    }
    Ok(contents.into_owned())
}

fn declares_euc_jis_2004(bytes: &[u8]) -> bool {
    const DECLARATION: &[u8] = b"coding: euc-jis-2004";
    bytes.get(..bytes.len().min(1024)).is_some_and(|header| {
        header
            .windows(DECLARATION.len())
            .any(|part| part == DECLARATION)
    })
}

impl Dictionary for SkkFileDictionary {
    fn lookup(&self, input: &str, okuri: Option<&str>) -> Result<Vec<DictionaryCandidate>> {
        let key = dictionary_key(input, okuri);
        let entries = if okuri.is_some() {
            self.okuri_ari.get(&key)
        } else {
            self.okuri_nasi.get(&key)
        };
        Ok(entries
            .into_iter()
            .flatten()
            .map(|entry| DictionaryCandidate {
                input: input.to_owned(),
                okuri: okuri.map(str::to_owned),
                text: entry.text.clone(),
                annotation: entry.annotation.clone(),
                source: CandidateSource::System,
                recency: None,
            })
            .collect())
    }

    fn complete(&self, prefix: &str) -> Result<Vec<String>> {
        let mut completions: Vec<_> = self
            .okuri_nasi
            .iter()
            .filter(|(input, entries)| input.starts_with(prefix) && !entries.is_empty())
            .map(|(input, _)| input.clone())
            .collect();
        completions.sort();
        Ok(completions)
    }
}

fn parse_candidates(value: &str) -> Result<Vec<Entry>> {
    if !value.starts_with('/') || !value.ends_with('/') {
        bail!("候補列が / で囲まれていません");
    }
    Ok(value[1..value.len() - 1]
        .split('/')
        .filter(|candidate| !candidate.is_empty())
        .map(|candidate| {
            let (text, annotation) = candidate
                .split_once(';')
                .map_or((candidate, None), |(text, annotation)| {
                    (text, Some(annotation.to_owned()))
                });
            Entry {
                text: text.to_owned(),
                annotation,
            }
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_both_sections_and_annotations() {
        let dictionary = SkkFileDictionary::parse(
            PathBuf::from("test"),
            ";; okuri-ari entries.\nおくr /送/\n;; okuri-nasi entries.\nかな /仮名;注釈/かな/\n",
        )
        .unwrap();
        assert_eq!(dictionary.okuri_ari["おくr"][0].text, "送");
        assert_eq!(
            dictionary.okuri_nasi["かな"][0].annotation.as_deref(),
            Some("注釈")
        );
    }

    #[test]
    fn decodes_euc_jis_2004_dictionary() {
        let bytes = b";; -*- coding: euc-jis-2004 -*-\n\
            ;; okuri-nasi entries.\n\
            \xa4\xc4\xa4\xc5\xa4\xe9 /\xfb\xc9\xc0\xee/\n";
        let contents = decode_dictionary(bytes).unwrap();
        assert!(contents.contains("つづら /𧃴川/"));
    }
}
