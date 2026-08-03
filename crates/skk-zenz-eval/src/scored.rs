use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScoredGroup {
    pub context: String,
    pub input: String,
    pub okuri: String,
    pub correct: String,
    pub prior_uses: usize,
    pub candidates: Vec<ScoredCandidate>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScoredCandidate {
    pub text: String,
    pub loss: f64,
    pub token_count: usize,
    pub recency: Option<f64>,
    pub from_user: bool,
}
