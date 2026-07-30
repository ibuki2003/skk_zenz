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
/// Represents a dictionary entry before conversion-specific output expansion.
///
/// User-dictionary implementations exchange this type with the conversion core
/// so usage state can be attached without depending on interactive IME state.
pub struct DictionaryCandidate {
    pub input: String,
    pub okuri: Option<String>,
    pub text: String,
    pub annotation: Option<String>,
    pub source: CandidateSource,
    pub recency: Option<f64>,
}

impl From<&Candidate> for DictionaryCandidate {
    /// Retains the dictionary identity and learned metadata of a rendered candidate.
    fn from(candidate: &Candidate) -> Self {
        Self {
            input: candidate.input.clone(),
            okuri: candidate.okuri.clone(),
            text: candidate.text.clone(),
            annotation: candidate.annotation.clone(),
            source: candidate.source,
            recency: candidate.recency,
        }
    }
}
