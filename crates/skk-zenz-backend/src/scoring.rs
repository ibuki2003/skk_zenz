use crate::candidate::{Candidate, CandidateSource};
use crate::config::CandidateSortKey;

pub trait ScoringStrategy {
    fn compare(&self, a: &Candidate, b: &Candidate) -> std::cmp::Ordering;
}

pub struct DefaultStrategy<'a> {
    pub order: &'a [CandidateSortKey],
}

impl ScoringStrategy for DefaultStrategy<'_> {
    fn compare(&self, left: &Candidate, right: &Candidate) -> std::cmp::Ordering {
        self.order
            .iter()
            .map(|key| match key {
                CandidateSortKey::FromUser => {
                    source_order(left.source).cmp(&source_order(right.source))
                }
                CandidateSortKey::Recency | CandidateSortKey::AiScore => {
                    let left_score =
                        ranking_score(left.recency.unwrap_or_default(), left.loss, *key);
                    let right_score =
                        ranking_score(right.recency.unwrap_or_default(), right.loss, *key);
                    left_score.total_cmp(&right_score)
                }
            })
            .find(|ordering| !ordering.is_eq())
            .unwrap_or(std::cmp::Ordering::Equal)
    }
}

pub fn ranking_score(frequency: f64, ai_loss: f64, sort_key: CandidateSortKey) -> f64 {
    match sort_key {
        CandidateSortKey::Recency => -frequency,
        CandidateSortKey::AiScore => ai_loss,
        CandidateSortKey::FromUser => 0.0,
    }
}

fn source_order(source: CandidateSource) -> u8 {
    match source {
        CandidateSource::User => 0,
        CandidateSource::System => 1,
    }
}

pub fn rerank_candidates(candidates: &mut [Candidate], strategy: &impl ScoringStrategy) {
    candidates.sort_by(|a, b| strategy.compare(a, b));
}
