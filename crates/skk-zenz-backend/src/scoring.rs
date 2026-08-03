use crate::candidate::{Candidate, CandidateSource};
use crate::config::CandidateOrder;

pub trait ScoringStrategy {
    fn rerank(&self, candidates: &mut [Candidate]);
}

pub trait PointwiseScoringStrategy {
    type Score: Ord;

    fn score(&self, candidate: &Candidate) -> Self::Score;
}

impl<T: PointwiseScoringStrategy> ScoringStrategy for T {
    fn rerank(&self, candidates: &mut [Candidate]) {
        // `sort_by_key` does not cache keys, but these pointwise scores are deliberately cheap.
        // A strategy with expensive or collection-dependent scoring should implement
        // `ScoringStrategy` directly and calculate its scores before sorting.
        candidates.sort_by_key(|candidate| self.score(candidate));
    }
}

#[derive(Clone, Copy, Debug)]
pub struct TotalF64(pub f64);

impl PartialEq for TotalF64 {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other).is_eq()
    }
}

impl Eq for TotalF64 {}

impl Ord for TotalF64 {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.0.total_cmp(&other.0)
    }
}

impl PartialOrd for TotalF64 {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

struct Recency;

impl PointwiseScoringStrategy for Recency {
    type Score = TotalF64;

    fn score(&self, candidate: &Candidate) -> Self::Score {
        TotalF64(-candidate.recency.unwrap_or_default())
    }
}

struct AiScore;

impl PointwiseScoringStrategy for AiScore {
    type Score = TotalF64;

    fn score(&self, candidate: &Candidate) -> Self::Score {
        TotalF64(candidate.loss)
    }
}

struct UserFirstAiScore;

impl PointwiseScoringStrategy for UserFirstAiScore {
    type Score = (u8, TotalF64);

    fn score(&self, candidate: &Candidate) -> Self::Score {
        (source_order(candidate.source), TotalF64(candidate.loss))
    }
}

struct RecencyThenAiScore;

impl PointwiseScoringStrategy for RecencyThenAiScore {
    type Score = (TotalF64, TotalF64);

    fn score(&self, candidate: &Candidate) -> Self::Score {
        (
            TotalF64(-candidate.recency.unwrap_or_default()),
            TotalF64(candidate.loss),
        )
    }
}

struct Mixture {
    lambda: f64,
}

impl ScoringStrategy for Mixture {
    fn rerank(&self, candidates: &mut [Candidate]) {
        if candidates.is_empty() {
            return;
        }
        let count = candidates.len() as f64;
        let min_loss = candidates
            .iter()
            .map(|candidate| candidate.loss)
            .fold(f64::INFINITY, f64::min);
        let ai_sum: f64 = candidates
            .iter()
            .map(|candidate| (-(candidate.loss - min_loss)).exp())
            .sum();
        let recency_sum: f64 = candidates
            .iter()
            .map(|candidate| candidate.recency.unwrap_or_default().max(0.0))
            .sum();
        let recency_probability = |candidate: &Candidate| {
            if recency_sum > 0.0 && recency_sum.is_finite() {
                candidate.recency.unwrap_or_default().max(0.0) / recency_sum
            } else {
                1.0 / count
            }
        };
        let entropy = -candidates
            .iter()
            .map(recency_probability)
            .filter(|probability| *probability > 0.0)
            .map(|probability| probability * probability.ln())
            .sum::<f64>();
        let entropy_confidence = if candidates.len() <= 1 {
            1.0
        } else {
            (1.0 - entropy / count.ln()).clamp(0.0, 1.0)
        };
        let score = |candidate: &Candidate| {
            let ai = if ai_sum > 0.0 && ai_sum.is_finite() {
                (-(candidate.loss - min_loss)).exp() / ai_sum
            } else {
                1.0 / count
            };
            let recency = entropy_confidence * recency_probability(candidate)
                + (1.0 - entropy_confidence) / count;
            self.lambda * ai.max(f64::MIN_POSITIVE).ln()
                + (1.0 - self.lambda) * recency.max(f64::MIN_POSITIVE).ln()
        };
        candidates.sort_by(|left, right| score(right).total_cmp(&score(left)));
    }
}

const fn source_order(source: CandidateSource) -> u8 {
    match source {
        CandidateSource::User => 0,
        CandidateSource::System => 1,
    }
}

pub fn rerank_candidates(candidates: &mut [Candidate], strategy: &impl ScoringStrategy) {
    strategy.rerank(candidates);
}

pub fn rerank_by_order(candidates: &mut [Candidate], order: CandidateOrder, mixture_lambda: f64) {
    match order {
        CandidateOrder::Recency => Recency.rerank(candidates),
        CandidateOrder::UserFirstAiScore => UserFirstAiScore.rerank(candidates),
        CandidateOrder::RecencyThenAiScore => RecencyThenAiScore.rerank(candidates),
        CandidateOrder::AiScore => AiScore.rerank(candidates),
        CandidateOrder::Mixture => Mixture {
            lambda: mixture_lambda,
        }
        .rerank(candidates),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(text: &str, loss: f64, recency: Option<f64>) -> Candidate {
        Candidate {
            input: String::new(),
            okuri: None,
            text: text.to_owned(),
            output: text.to_owned(),
            annotation: None,
            source: CandidateSource::System,
            loss,
            token_count: 1,
            recency,
        }
    }

    #[test]
    fn mixture_uses_log_probabilities_smoothed_by_entropy_confidence() {
        let mut candidates = vec![
            candidate("ai", 0.0, None),
            candidate("recency", 1.0, Some(3.0)),
            candidate("third", 2.0, Some(1.0)),
        ];

        rerank_by_order(&mut candidates, CandidateOrder::Mixture, 0.5);

        assert_eq!(
            candidates
                .iter()
                .map(|candidate| candidate.text.as_str())
                .collect::<Vec<_>>(),
            ["recency", "ai", "third"]
        );
    }
}
