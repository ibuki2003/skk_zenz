#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CandidateSource {
    User,
    System,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Candidate {
    pub input: String,
    pub okuri: Option<String>,
    pub text: String,
    pub output: String,
    pub annotation: Option<String>,
    pub source: CandidateSource,
    pub loss: f64,
    pub token_count: usize,
    pub recency: Option<f64>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct DictionaryCandidate {
    pub input: String,
    pub okuri: Option<String>,
    pub text: String,
    pub annotation: Option<String>,
    pub source: CandidateSource,
    pub recency: Option<f64>,
}
