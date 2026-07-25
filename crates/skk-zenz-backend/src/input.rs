use crate::candidate::Candidate;
use crate::keymap::KeyEvent;
use crate::rom_kana::{KanaMode, RomKanaRule};

#[derive(Clone, Copy, Debug, Default, Hash, PartialEq, Eq)]
pub enum InputMode {
    #[default]
    Hiragana,
    Katakana,
    HankakuKatakana,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InputEvent {
    Key(KeyEvent),
    SetContext(String),
    FocusOut { commit: bool },
    SelectCandidate(usize),
    Reset,
}

#[derive(Clone, Debug, PartialEq)]
pub struct InputUpdate {
    pub consumed: bool,
    pub preedit: String,
    pub candidates: Vec<Candidate>,
    pub commit: Option<String>,
    pub input_mode: InputMode,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct InputState {
    pub(crate) mode: InputMode,
    pub(crate) context: String,
    pub(crate) phase: Phase,
    pub(crate) direct_pending: String,
}

#[derive(Clone, Debug, Default)]
pub(crate) enum Phase {
    #[default]
    Direct,
    Preedit {
        stem: Composition,
        okuri: Option<Composition>,
    },
    Selecting {
        input: String,
        okuri: Option<String>,
        candidates: Vec<Candidate>,
        cursor: usize,
    },
}

#[derive(Clone, Debug, Default)]
pub(crate) struct Composition {
    output: String,
    pending: String,
}

impl InputState {
    pub(crate) fn reset(&mut self) {
        self.phase = Phase::Direct;
        self.direct_pending.clear();
    }

    pub(crate) fn preedit(&self) -> String {
        match &self.phase {
            Phase::Direct => self.direct_pending.clone(),
            Phase::Preedit { stem, okuri } => {
                let mut value = format!("▽{}", stem.display());
                if let Some(okuri) = okuri {
                    value.push('*');
                    value.push_str(&okuri.display());
                }
                value
            }
            Phase::Selecting {
                candidates, cursor, ..
            } => candidates
                .get(*cursor)
                .map(|candidate| format!("▼{}", candidate.output))
                .unwrap_or_default(),
        }
    }

    pub(crate) fn candidates(&self) -> Vec<Candidate> {
        match &self.phase {
            Phase::Selecting { candidates, .. } => candidates.clone(),
            _ => Vec::new(),
        }
    }

    pub(crate) fn speculative_input(&self) -> Option<String> {
        match &self.phase {
            Phase::Preedit { stem, .. } if !stem.output.is_empty() => Some(stem.output.clone()),
            _ => None,
        }
    }
}

impl Composition {
    pub(crate) fn from_output(output: String) -> Self {
        Self {
            output,
            pending: String::new(),
        }
    }

    pub(crate) fn push(&mut self, rule: &RomKanaRule, text: &str, mode: KanaMode) {
        let input = format!("{}{text}", self.pending);
        let conversion = rule.convert_partial(&input, mode);
        self.output.push_str(&conversion.output);
        self.pending = conversion.preedit;
    }

    pub(crate) fn pop(&mut self) -> bool {
        if self.pending.pop().is_some() {
            return true;
        }
        self.output.pop().is_some()
    }

    pub(crate) fn finish(&self, rule: &RomKanaRule, mode: KanaMode) -> String {
        let mut value = self.output.clone();
        let conversion = rule.convert(&self.pending, mode);
        value.push_str(&conversion.output);
        value.push_str(&conversion.preedit);
        value
    }

    pub(crate) fn display(&self) -> String {
        format!("{}{}", self.output, self.pending)
    }

    pub(crate) fn has_output(&self) -> bool {
        !self.output.is_empty()
    }
}

impl From<InputMode> for KanaMode {
    fn from(mode: InputMode) -> Self {
        match mode {
            InputMode::Hiragana => Self::Hiragana,
            InputMode::Katakana => Self::Katakana,
            InputMode::HankakuKatakana => Self::HankakuKatakana,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    fn rule() -> RomKanaRule {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("test-data/rules");
        RomKanaRule::load("base", &[root]).unwrap()
    }

    #[test]
    fn composition_keeps_pending_romaji_separate() {
        let rule = rule();
        let mut composition = Composition::default();
        composition.push(&rule, "k", KanaMode::Hiragana);
        assert_eq!(composition.display(), "k");
        composition.push(&rule, "a", KanaMode::Hiragana);
        assert_eq!(composition.display(), "か");
    }

    #[test]
    fn preedit_and_selection_have_skk_markers() {
        let mut state = InputState {
            phase: Phase::Preedit {
                stem: Composition::from_output("かな".to_owned()),
                okuri: None,
            },
            ..InputState::default()
        };
        assert_eq!(state.preedit(), "▽かな");
        state.phase = Phase::Selecting {
            input: "かな".to_owned(),
            okuri: None,
            candidates: vec![Candidate {
                input: "かな".to_owned(),
                okuri: None,
                text: "仮名".to_owned(),
                output: "仮名".to_owned(),
                annotation: None,
                source: crate::CandidateSource::System,
                loss: 0.0,
                token_count: 1,
                recency: None,
                use_count: 0,
            }],
            cursor: 0,
        };
        assert_eq!(state.preedit(), "▼仮名");
    }
}
