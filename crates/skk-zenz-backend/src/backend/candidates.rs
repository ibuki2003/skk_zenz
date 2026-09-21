use super::*;
use crate::candidate::CandidateSource;

impl Backend {
    pub(super) fn complete(&self, prefix: &str) -> Result<Vec<String>> {
        self.conversion.complete(prefix)
    }

    pub(super) fn current_order_uses_ai_score(&self) -> bool {
        let order = match &self.input_state.phase {
            Phase::Preedit { stem, okuri } => self
                .conversion
                .lookup_order(&stem.display(), okuri.as_ref().map(|_| "")),
            Phase::Abbrev { .. } => self.conversion.abbrev_order(),
            Phase::Direct | Phase::Selecting(_) => return false,
        };
        order.uses_ai_score()
    }

    pub fn lookup(
        &mut self,
        context: &str,
        input: &str,
        okuri: Option<&str>,
    ) -> Result<Vec<Candidate>> {
        #[cfg(debug_assertions)]
        eprintln!("[BACKEND] lookup context={context:?} input={input:?} okuri={okuri:?}");
        let order = self.conversion.lookup_order(input, okuri);
        #[cfg(debug_assertions)]
        eprintln!("[BACKEND] lookup order={order:?}");
        let candidates = self.conversion.lookup(input, okuri)?;
        let candidates = self.rerank_candidates(context, input, candidates, order)?;
        #[cfg(debug_assertions)]
        {
            eprint!("[BACKEND] lookup -> ");
            for candidate in &candidates {
                eprint!(
                    "{:?}({}, {:?}) ",
                    candidate.output, candidate.loss, candidate.recency
                );
            }
        }
        Ok(candidates)
    }

    pub(super) fn lookup_abbrev(&mut self, input: &str) -> Result<Vec<Candidate>> {
        let order = self.conversion.abbrev_order();
        #[cfg(debug_assertions)]
        eprintln!("[BACKEND] abbr lookup order={order:?}");
        let context = self.input_state.context.clone();
        let candidates = self.conversion.lookup_abbrev(input)?;
        self.rerank_candidates(&context, input, candidates, order)
    }

    pub(super) fn rerank_candidates(
        &mut self,
        context: &str,
        input: &str,
        mut candidates: Vec<Candidate>,
        order: CandidateOrder,
    ) -> Result<Vec<Candidate>> {
        match order {
            CandidateOrder::UserFirstAiScore => {
                candidates.sort_by_key(|candidate| candidate.source != CandidateSource::User);
            }
            CandidateOrder::RecencyThenAiScore | CandidateOrder::Mixture => {
                self.conversion
                    .rerank_candidates(&mut candidates, CandidateOrder::Recency);
            }
            CandidateOrder::Recency | CandidateOrder::AiScore => {}
        }
        self.conversion
            .deduplicate_candidate_outputs(&mut candidates);
        if candidates.is_empty() {
            self.active_rerank = None;
            return Ok(candidates);
        }
        if !order.uses_ai_score() {
            self.active_rerank = None;
            return Ok(candidates);
        }

        let total = candidates.len();
        let (generation, mut initial) = self.scorer.rerank(context, input, candidates)?;
        let evaluated = initial.len();
        self.conversion.rerank_candidates(&mut initial, order);
        self.active_rerank = Some(ActiveRerank {
            generation,
            order,
            selection_touched: false,
            evaluated,
            total,
        });
        Ok(initial)
    }

    pub fn register(
        &mut self,
        input: &str,
        okuri: Option<&str>,
        text: &str,
        annotation: Option<&str>,
    ) -> Result<()> {
        self.conversion.register(input, okuri, text, annotation)
    }

    /// Records one use of a candidate in the conversion core's user dictionary.
    pub fn record_usage(&mut self, candidate: &Candidate) -> Result<()> {
        self.conversion.record_usage(candidate)
    }

    pub(super) fn confirm_candidate(
        &mut self,
        input: &str,
        okuri: Option<&str>,
        candidate: &Candidate,
    ) -> Result<()> {
        self.record_usage(candidate)?;

        if self.conversion_history_enabled {
            let text = candidate
                .output
                .strip_suffix(okuri.unwrap_or_default())
                .unwrap_or(&candidate.output);
            if let Err(error) = conversion_history::append(
                &self.conversion_history_path,
                &self.input_state.context,
                input,
                okuri,
                text,
            ) {
                eprintln!("[skk-zenz] {error:#}");
            }
        }
        Ok(())
    }

    pub fn purge(&mut self, candidate: &Candidate) -> Result<bool> {
        self.conversion.purge(candidate)
    }
}
