use super::helpers::{compare_candidates, deduplicate_candidate_outputs};
use super::*;

impl Backend {
    pub(super) fn current_order_uses_ai_score(&self) -> bool {
        let order = match &self.input_state.phase {
            Phase::Preedit { okuri: Some(_), .. } => &self.candidate_order.okuri_ari,
            Phase::Preedit { okuri: None, .. } => &self.candidate_order.okuri_nasi,
            Phase::Abbrev { .. } => &self.candidate_order.abbrev,
            Phase::Direct | Phase::Selecting { .. } => return false,
        };
        order.contains(&CandidateSortKey::AiScore)
    }

    pub fn lookup(
        &mut self,
        context: &str,
        input: &str,
        okuri: Option<&str>,
    ) -> Result<Vec<Candidate>> {
        eprintln!("[BACKEND] lookup context={context:?} input={input:?} okuri={okuri:?}");
        let order = if okuri.is_some() {
            self.candidate_order.okuri_ari.clone()
        } else {
            self.candidate_order.okuri_nasi.clone()
        };
        eprintln!("[BACKEND] lookup order={order:?}");
        let numeric_input = NumericInput::extract(input);
        let dictionary_candidates =
            self.lookup_dictionaries(input, okuri, numeric_input.as_ref())?;

        let mut candidates = Vec::with_capacity(dictionary_candidates.len());
        for candidate in dictionary_candidates {
            for text in self.expand_numeric_candidate(&candidate, numeric_input.as_ref())? {
                let output = format!("{text}{}", candidate.okuri.as_deref().unwrap_or_default());
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
        let candidates = self.rerank_candidates(context, input, candidates, &order)?;
        eprint!("[BACKEND] lookup -> ");
        for candidate in &candidates {
            eprint!(
                "{:?}({}, {:?}) ",
                candidate.output, candidate.loss, candidate.recency
            );
        }
        Ok(candidates)
    }

    pub(super) fn lookup_abbrev(&mut self, input: &str) -> Result<Vec<Candidate>> {
        let order = self.candidate_order.abbrev.clone();
        eprintln!("[BACKEND] abbr lookup order={order:?}");
        let context = self.input_state.context.clone();
        let numeric_input = NumericInput::extract(input);
        let dictionary_candidates =
            self.lookup_dictionaries(input, None, numeric_input.as_ref())?;
        let mut candidates = Vec::with_capacity(dictionary_candidates.len());
        for candidate in dictionary_candidates {
            for output in self.expand_numeric_candidate(&candidate, numeric_input.as_ref())? {
                candidates.push(Candidate {
                    input: candidate.input.clone(),
                    okuri: None,
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
        self.rerank_candidates(&context, input, candidates, &order)
    }

    pub(super) fn rerank_candidates(
        &mut self,
        context: &str,
        input: &str,
        mut candidates: Vec<Candidate>,
        order: &[CandidateSortKey],
    ) -> Result<Vec<Candidate>> {
        let initial_order: Vec<_> = order
            .iter()
            .copied()
            .filter(|key| *key != CandidateSortKey::AiScore)
            .collect();
        candidates.sort_by(|left, right| compare_candidates(left, right, &initial_order));
        deduplicate_candidate_outputs(&mut candidates);
        if candidates.is_empty() {
            self.active_rerank = None;
            return Ok(candidates);
        }
        if !order.contains(&CandidateSortKey::AiScore) {
            self.active_rerank = None;
            return Ok(candidates);
        }

        let total = candidates.len();
        let (generation, mut initial) = self.scorer.rerank(context, input, candidates)?;
        let evaluated = initial.len();
        initial.sort_by(|left, right| compare_candidates(left, right, order));
        self.active_rerank = Some(ActiveRerank {
            generation,
            order: order.to_vec(),
            selection_touched: false,
            evaluated,
            total,
        });
        Ok(initial)
    }

    pub(super) fn lookup_dictionaries(
        &self,
        input: &str,
        okuri: Option<&str>,
        numeric_input: Option<&NumericInput>,
    ) -> Result<Vec<DictionaryCandidate>> {
        let mut candidates = self.lookup_dictionary_key(input, okuri)?;
        if let Some(numeric_input) = numeric_input
            && numeric_input.key != input
        {
            candidates.extend(self.lookup_dictionary_key(&numeric_input.key, okuri)?);
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

    pub(super) fn lookup_dictionary_key(
        &self,
        input: &str,
        okuri: Option<&str>,
    ) -> Result<Vec<DictionaryCandidate>> {
        let mut candidates = self.user_dictionary.lookup(input, okuri)?;
        for dictionary in &self.system_dictionaries {
            candidates.extend(dictionary.lookup(input, okuri)?);
        }
        Ok(candidates)
    }

    pub(super) fn expand_numeric_candidate(
        &self,
        candidate: &DictionaryCandidate,
        numeric_input: Option<&NumericInput>,
    ) -> Result<Vec<String>> {
        let Some(numeric_input) = numeric_input.filter(|numeric| candidate.input == numeric.key)
        else {
            return Ok(vec![candidate.text.clone()]);
        };
        numeric_input.expand(&candidate.text, |value| {
            self.lookup_dictionary_key(value, None).map(|candidates| {
                candidates
                    .into_iter()
                    .map(|candidate| candidate.text)
                    .collect()
            })
        })
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
        self.user_dictionary.select(&dictionary_candidate)
    }

    pub fn purge(&mut self, candidate: &Candidate) -> Result<bool> {
        self.user_dictionary.purge(
            &candidate.input,
            candidate.okuri.as_deref(),
            &candidate.text,
        )
    }
}
