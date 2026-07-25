use anyhow::Result;
use std::collections::{HashMap, HashSet};

use crate::candidate::{Candidate, CandidateSource, DictionaryCandidate};
use crate::config::BackendConfig;
use crate::dictionary::{Dictionary, SkkFileDictionary, UserDictionary, open_system_dictionaries};
use crate::zenz::{Score, ZenzScorer};

pub struct Backend {
    scorer: ZenzScorer,
    system_dictionaries: Vec<SkkFileDictionary>,
    user_dictionary: UserDictionary,
    recency: HashMap<CandidateKey, u64>,
    sequence: u64,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
struct CandidateKey {
    input: String,
    okuri: Option<String>,
    text: String,
}

impl Backend {
    pub fn load(config: BackendConfig) -> Result<Self> {
        Ok(Self {
            scorer: ZenzScorer::load(&config.zenz)?,
            system_dictionaries: open_system_dictionaries(&config.system_dictionaries)?,
            user_dictionary: UserDictionary::open(&config.user_dictionary)?,
            recency: HashMap::new(),
            sequence: 0,
        })
    }

    pub fn clear_cache(&mut self) {
        self.scorer.clear();
    }

    pub fn prepare(&mut self, context: &str, input: &str) -> Result<Score> {
        self.scorer.prepare(context, input)
    }

    pub fn lookup(
        &mut self,
        context: &str,
        input: &str,
        okuri: Option<&str>,
    ) -> Result<Vec<Candidate>> {
        let mut dictionary_candidates = self.user_dictionary.lookup(input, okuri)?;
        for dictionary in &self.system_dictionaries {
            dictionary_candidates.extend(dictionary.lookup(input, okuri)?);
        }
        let mut seen = HashSet::new();
        dictionary_candidates.retain(|candidate| {
            seen.insert(format!(
                "{}{}",
                candidate.text,
                candidate.okuri.as_deref().unwrap_or_default()
            ))
        });

        let mut candidates = Vec::with_capacity(dictionary_candidates.len());
        for candidate in dictionary_candidates {
            let output = format!(
                "{}{}",
                candidate.text,
                candidate.okuri.as_deref().unwrap_or_default()
            );
            let score = self.scorer.score(context, input, &output)?;
            let key = CandidateKey::from(&candidate);
            candidates.push(Candidate {
                input: candidate.input,
                okuri: candidate.okuri,
                text: candidate.text,
                output,
                annotation: candidate.annotation,
                source: candidate.source,
                loss: score.loss,
                token_count: score.token_count,
                recency: self.recency.get(&key).copied(),
                use_count: candidate.use_count,
            });
        }
        candidates.sort_by(|left, right| {
            source_order(left.source)
                .cmp(&source_order(right.source))
                .then_with(|| right.recency.cmp(&left.recency))
                .then_with(|| left.loss.total_cmp(&right.loss))
        });
        Ok(candidates)
    }

    pub fn register(
        &mut self,
        input: &str,
        okuri: Option<&str>,
        text: &str,
        annotation: Option<&str>,
    ) -> Result<()> {
        self.user_dictionary
            .register(input, okuri, text, annotation)
    }

    pub fn select(&mut self, candidate: &Candidate) -> Result<()> {
        let dictionary_candidate = DictionaryCandidate::from(candidate);
        self.user_dictionary.select(&dictionary_candidate)?;
        self.sequence = self.sequence.wrapping_add(1);
        self.recency
            .insert(CandidateKey::from(&dictionary_candidate), self.sequence);
        Ok(())
    }

    pub fn purge(&mut self, candidate: &Candidate) -> Result<bool> {
        let removed = self.user_dictionary.purge(
            &candidate.input,
            candidate.okuri.as_deref(),
            &candidate.text,
        )?;
        self.recency.remove(&CandidateKey::from(candidate));
        Ok(removed)
    }
}

fn source_order(source: CandidateSource) -> u8 {
    match source {
        CandidateSource::User => 0,
        CandidateSource::System => 1,
    }
}

impl From<&DictionaryCandidate> for CandidateKey {
    fn from(candidate: &DictionaryCandidate) -> Self {
        Self {
            input: candidate.input.clone(),
            okuri: candidate.okuri.clone(),
            text: candidate.text.clone(),
        }
    }
}

impl From<&Candidate> for CandidateKey {
    fn from(candidate: &Candidate) -> Self {
        Self {
            input: candidate.input.clone(),
            okuri: candidate.okuri.clone(),
            text: candidate.text.clone(),
        }
    }
}

impl From<&Candidate> for DictionaryCandidate {
    fn from(candidate: &Candidate) -> Self {
        Self {
            input: candidate.input.clone(),
            okuri: candidate.okuri.clone(),
            text: candidate.text.clone(),
            annotation: candidate.annotation.clone(),
            source: candidate.source,
            use_count: candidate.use_count,
        }
    }
}
