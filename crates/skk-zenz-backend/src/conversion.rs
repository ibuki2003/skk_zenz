use std::collections::HashSet;
use std::path::Path;

use anyhow::Result;

use crate::candidate::{Candidate, DictionaryCandidate};
use crate::config::{CandidateOrder, CandidateOrderConfig};
use crate::dictionary::{
    Dictionary, SkkFileDictionary, UserDictionary, open_system_dictionaries, unix_timestamp,
};
use crate::numeric::NumericInput;
use crate::scoring::{self, ScoringStrategy};

/// Owns the reusable state and behavior required to produce and rank conversion candidates.
///
/// The interactive backend and offline simulations share this component while
/// supplying different user-dictionary implementations and AI scoring drivers.
pub struct ConversionCore<U> {
    system_dictionaries: Vec<SkkFileDictionary>,
    user_dictionary: U,
    candidate_order: CandidateOrderConfig,
}

impl<U: UserDictionary> ConversionCore<U> {
    /// Loads system dictionaries and constructs a conversion core around `user_dictionary`.
    pub fn load(
        system_dictionary_paths: &[impl AsRef<Path>],
        user_dictionary: U,
        candidate_order: CandidateOrderConfig,
    ) -> Result<Self> {
        Ok(Self {
            system_dictionaries: open_system_dictionaries(system_dictionary_paths)?,
            user_dictionary,
            candidate_order,
        })
    }

    /// Replaces system dictionaries after loading every new dictionary successfully.
    pub(crate) fn reload_system_dictionaries(&mut self, paths: &[impl AsRef<Path>]) -> Result<()> {
        let dictionaries = open_system_dictionaries(paths)?;
        self.system_dictionaries = dictionaries;
        Ok(())
    }

    /// Updates conversion ordering without changing already generated candidates.
    pub(crate) fn set_candidate_order(&mut self, candidate_order: CandidateOrderConfig) {
        self.candidate_order = candidate_order;
    }

    /// Updates the frequency half-life used by the owned user dictionary.
    pub(crate) fn set_half_life_days(&mut self, half_life_days: f64) {
        self.user_dictionary.set_half_life_days(half_life_days);
    }

    /// Returns the configured sort order for a standard conversion request.
    pub fn lookup_order(&self, input: &str, okuri: Option<&str>) -> CandidateOrder {
        if is_prefix_or_suffix(input) {
            self.candidate_order.prefix_suffix
        } else if okuri.is_some() {
            self.candidate_order.okuri_ari
        } else {
            self.candidate_order.okuri_nasi
        }
    }

    /// Returns the configured sort order for an abbreviation conversion.
    pub fn abbrev_order(&self) -> CandidateOrder {
        self.candidate_order.abbrev
    }

    /// Generates candidates using the current wall-clock time for frequency decay.
    pub fn lookup(&self, input: &str, okuri: Option<&str>) -> Result<Vec<Candidate>> {
        self.lookup_at(input, okuri, unix_timestamp()?)
    }

    /// Generates candidates with frequencies evaluated at an explicit timestamp.
    ///
    /// Offline simulations use this method to replay conversion history without
    /// depending on the process wall clock.
    pub fn lookup_at(&self, input: &str, okuri: Option<&str>, now: i64) -> Result<Vec<Candidate>> {
        self.generate_candidates_at(input, okuri, true, now)
    }

    /// Generates abbreviation candidates using the current wall-clock time.
    pub(crate) fn lookup_abbrev(&self, input: &str) -> Result<Vec<Candidate>> {
        self.generate_candidates_at(input, None, false, unix_timestamp()?)
    }

    /// Registers a user candidate and records its first use at the current time.
    pub fn register(
        &mut self,
        input: &str,
        okuri: Option<&str>,
        text: &str,
        annotation: Option<&str>,
    ) -> Result<()> {
        self.register_at(input, okuri, text, annotation, unix_timestamp()?)
    }

    /// Registers a user candidate and records its use at an explicit timestamp.
    pub fn register_at(
        &mut self,
        input: &str,
        okuri: Option<&str>,
        text: &str,
        annotation: Option<&str>,
        now: i64,
    ) -> Result<()> {
        self.user_dictionary
            .register_at(input, okuri, text, annotation, now)
    }

    /// Records one use of a generated candidate at the current wall-clock time.
    pub fn record_usage(&mut self, candidate: &Candidate) -> Result<()> {
        self.record_usage_at(candidate, unix_timestamp()?)
    }

    /// Records one use of a generated candidate at an explicit timestamp.
    ///
    /// This is the state transition used by simulations after choosing a
    /// candidate from the result of [`Self::lookup_at`].
    pub fn record_usage_at(&mut self, candidate: &Candidate, now: i64) -> Result<()> {
        self.user_dictionary
            .record_usage(&DictionaryCandidate::from(candidate), now)
    }

    /// Removes a generated candidate from the owned user dictionary.
    pub fn purge(&mut self, candidate: &Candidate) -> Result<bool> {
        self.user_dictionary.purge(
            &candidate.input,
            candidate.okuri.as_deref(),
            &candidate.text,
        )
    }

    /// Sorts candidates according to the supplied configured key priority.
    pub fn rerank_candidates(&self, candidates: &mut [Candidate], order: CandidateOrder) {
        scoring::rerank_by_order(candidates, order, self.candidate_order.mixture_lambda);
    }

    /// Sorts candidates using a caller-supplied scoring strategy.
    pub fn rerank_candidates_with(candidates: &mut [Candidate], strategy: &impl ScoringStrategy) {
        scoring::rerank_candidates(candidates, strategy);
    }

    /// Removes candidates that produce the same committed output.
    pub(crate) fn deduplicate_candidate_outputs(&self, candidates: &mut Vec<Candidate>) {
        let mut seen = HashSet::new();
        candidates.retain(|candidate| seen.insert(candidate.output.clone()));
    }

    /// Generates candidates through the shared dictionary and expansion pipeline.
    fn generate_candidates_at(
        &self,
        input: &str,
        okuri: Option<&str>,
        append_okuri: bool,
        now: i64,
    ) -> Result<Vec<Candidate>> {
        let numeric_input = NumericInput::extract(input);
        let dictionary_candidates =
            self.lookup_dictionaries_at(input, okuri, numeric_input.as_ref(), now)?;
        let mut candidates = Vec::with_capacity(dictionary_candidates.len());

        for candidate in dictionary_candidates {
            for text in self.expand_numeric_candidate_at(&candidate, numeric_input.as_ref(), now)? {
                let output = if append_okuri {
                    format!("{text}{}", candidate.okuri.as_deref().unwrap_or_default())
                } else {
                    text
                };
                candidates.push(Candidate {
                    input: candidate.input.clone(),
                    okuri: candidate.okuri.clone(),
                    text: candidate.text.clone(),
                    output,
                    annotation: candidate.annotation.clone(),
                    source: candidate.source,
                    loss: 0.0,
                    token_count: 0,
                    recency: candidate.recency,
                });
            }
        }
        Ok(candidates)
    }

    /// Looks up literal and normalized numeric keys and removes duplicate entries.
    fn lookup_dictionaries_at(
        &self,
        input: &str,
        okuri: Option<&str>,
        numeric_input: Option<&NumericInput>,
        now: i64,
    ) -> Result<Vec<DictionaryCandidate>> {
        let mut candidates = self.lookup_dictionary_key_at(input, okuri, now)?;
        if let Some(numeric_input) = numeric_input
            && numeric_input.key != input
        {
            candidates.extend(self.lookup_dictionary_key_at(&numeric_input.key, okuri, now)?);
        }
        let mut seen = HashSet::new();
        candidates.retain(|candidate| {
            seen.insert(format!(
                "{}{}",
                candidate.text,
                candidate.okuri.as_deref().unwrap_or_default()
            ))
        });
        Ok(candidates)
    }

    /// Collects candidates for one dictionary key with user entries taking precedence.
    fn lookup_dictionary_key_at(
        &self,
        input: &str,
        okuri: Option<&str>,
        now: i64,
    ) -> Result<Vec<DictionaryCandidate>> {
        let mut candidates = self.user_dictionary.lookup_at(input, okuri, now)?;
        for dictionary in &self.system_dictionaries {
            candidates.extend(dictionary.lookup(input, okuri)?);
        }
        Ok(candidates)
    }

    /// Expands numeric placeholders, including dictionary-backed reconversion placeholders.
    fn expand_numeric_candidate_at(
        &self,
        candidate: &DictionaryCandidate,
        numeric_input: Option<&NumericInput>,
        now: i64,
    ) -> Result<Vec<String>> {
        let Some(numeric_input) = numeric_input.filter(|numeric| candidate.input == numeric.key)
        else {
            return Ok(vec![candidate.text.clone()]);
        };
        numeric_input.expand(&candidate.text, |value| {
            self.lookup_dictionary_key_at(value, None, now)
                .map(|candidates| {
                    candidates
                        .into_iter()
                        .map(|candidate| candidate.text)
                        .collect()
                })
        })
    }
}

/// Reports whether a dictionary key denotes prefix or suffix conversion.
fn is_prefix_or_suffix(input: &str) -> bool {
    input.starts_with('>') || input.ends_with('>')
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::candidate::CandidateSource;
    use crate::dictionary::MemoryUserDictionary;

    #[test]
    fn recognizes_prefix_and_suffix_midasi() {
        assert!(is_prefix_or_suffix(">接頭"));
        assert!(is_prefix_or_suffix("接尾>"));
        assert!(!is_prefix_or_suffix("通常"));
        assert!(!is_prefix_or_suffix("途>中"));
    }

    #[test]
    fn candidate_order_follows_configured_key_priority() {
        let candidate = |recency, loss| Candidate {
            input: String::new(),
            okuri: None,
            text: String::new(),
            output: String::new(),
            annotation: None,
            source: CandidateSource::System,
            loss,
            token_count: 0,
            recency,
        };
        let core = ConversionCore::load(
            &[] as &[&Path],
            MemoryUserDictionary::new(1.0),
            CandidateOrderConfig::default(),
        )
        .unwrap();
        let frequent = candidate(Some(2.0), 2.0);
        let accurate = candidate(Some(1.0), 1.0);
        let mut candidates = vec![accurate.clone(), frequent.clone()];

        core.rerank_candidates(&mut candidates, CandidateOrder::RecencyThenAiScore);
        assert_eq!(candidates, [frequent.clone(), accurate.clone()]);

        core.rerank_candidates(&mut candidates, CandidateOrder::AiScore);
        assert_eq!(candidates, [accurate, frequent]);
    }

    #[test]
    fn memory_user_dictionary_supports_timestamped_conversion_replay() {
        let mut core = ConversionCore::load(
            &[] as &[&Path],
            MemoryUserDictionary::new(1.0),
            CandidateOrderConfig::default(),
        )
        .unwrap();
        core.register_at("かな", None, "仮名", None, 0).unwrap();

        let candidates = core.lookup_at("かな", None, 86_400).unwrap();
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].recency, Some(0.5));

        core.record_usage_at(&candidates[0], 86_400).unwrap();
        let candidates = core.lookup_at("かな", None, 86_400).unwrap();
        assert_eq!(candidates[0].recency, Some(1.5));
    }
}
