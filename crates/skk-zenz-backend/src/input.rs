use crate::candidate::Candidate;
use crate::keymap::KeyEvent;
use crate::rom_kana::{KanaMode, RomKanaRule};

#[derive(Clone, Copy, Debug, Default, Hash, PartialEq, Eq)]
pub enum InputMode {
    #[default]
    Hiragana,
    Katakana,
    HankakuKatakana,
    Latin,
    WideLatin,
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
    pub selected_candidate: Option<usize>,
    pub commit: Option<String>,
    pub input_mode: InputMode,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct InputState {
    pub(crate) mode: InputMode,
    pub(crate) context: String,
    pub(crate) phase: Phase,
    pub(crate) direct_pending: String,
    pub(crate) registrations: Vec<Registration>,
}

#[derive(Clone, Debug, Default)]
pub(crate) enum Phase {
    #[default]
    Direct,
    Preedit {
        stem: Composition,
        okuri: Option<Composition>,
    },
    Abbrev {
        input: String,
    },
    Selecting {
        input: String,
        okuri: Option<String>,
        candidates: Vec<Candidate>,
        cursor: usize,
        origin: SelectionOrigin,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SelectionOrigin {
    Preedit,
    Abbrev,
}

#[derive(Clone, Debug)]
pub(crate) struct Registration {
    pub(crate) input: String,
    pub(crate) okuri: Option<String>,
    pub(crate) source_phase: Phase,
    pub(crate) text: String,
    pub(crate) mode: InputMode,
    pub(crate) context_before: String,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct Composition {
    output: String,
    pending: String,
}

impl InputState {
    pub(crate) fn reset(&mut self) {
        if let Some(registration) = self.registrations.first() {
            self.mode = registration.mode;
            self.context.clone_from(&registration.context_before);
        }
        self.phase = Phase::Direct;
        self.direct_pending.clear();
        self.registrations.clear();
    }

    pub(crate) fn preedit(&self) -> String {
        let mut value = match &self.phase {
            Phase::Direct => self.direct_pending.clone(),
            Phase::Preedit { stem, okuri } => {
                let mut value = format!("▽{}", stem.display());
                if let Some(okuri) = okuri {
                    value.push('*');
                    value.push_str(&okuri.display());
                }
                value
            }
            Phase::Abbrev { input } => format!("▽{input}"),
            Phase::Selecting {
                candidates, cursor, ..
            } => candidates
                .get(*cursor)
                .map(|candidate| format!("▼{}", candidate.output))
                .unwrap_or_default(),
        };
        for registration in self.registrations.iter().rev() {
            let mut header = registration.input.clone();
            if let Some(okuri) = &registration.okuri {
                header.push('*');
                header.push_str(okuri);
            }
            value = format!("▼{header}【{}{value}】", registration.text);
        }
        value
    }

    pub(crate) fn candidates(&self) -> Vec<Candidate> {
        match &self.phase {
            Phase::Selecting { candidates, .. } => candidates.clone(),
            _ => Vec::new(),
        }
    }

    pub(crate) fn selected_candidate(&self) -> Option<usize> {
        match &self.phase {
            Phase::Selecting { cursor, .. } => Some(*cursor),
            _ => None,
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

    pub(crate) fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }
}

impl From<InputMode> for KanaMode {
    fn from(mode: InputMode) -> Self {
        match mode {
            InputMode::Hiragana => Self::Hiragana,
            InputMode::Katakana => Self::Katakana,
            InputMode::HankakuKatakana => Self::HankakuKatakana,
            InputMode::Latin | InputMode::WideLatin => {
                unreachable!("Latin mode does not use rom-kana conversion")
            }
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
        assert!(!composition.has_output());
        assert!(composition.has_pending());
        composition.push(&rule, "a", KanaMode::Hiragana);
        assert_eq!(composition.display(), "か");
        assert!(composition.has_output());
        assert!(!composition.has_pending());
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
            }],
            cursor: 0,
            origin: SelectionOrigin::Preedit,
        };
        assert_eq!(state.preedit(), "▼仮名");
    }

    #[test]
    fn abbrev_preedit_keeps_ascii_input() {
        let state = InputState {
            phase: Phase::Abbrev {
                input: "skk-server".to_owned(),
            },
            ..InputState::default()
        };
        assert_eq!(state.preedit(), "▽skk-server");
        assert_eq!(state.speculative_input(), None);
    }

    #[test]
    fn registration_preedit_wraps_nested_input() {
        let source_phase = Phase::Preedit {
            stem: Composition::from_output("かぱ".to_owned()),
            okuri: None,
        };
        let mut state = InputState {
            phase: Phase::Preedit {
                stem: Composition::from_output("か".to_owned()),
                okuri: None,
            },
            registrations: vec![Registration {
                input: "かぱ".to_owned(),
                okuri: None,
                source_phase: source_phase.clone(),
                text: "下".to_owned(),
                mode: InputMode::Hiragana,
                context_before: String::new(),
            }],
            ..InputState::default()
        };
        assert_eq!(state.preedit(), "▼かぱ【下▽か】");

        state.registrations.push(Registration {
            input: "か".to_owned(),
            okuri: Some("え".to_owned()),
            source_phase,
            text: String::new(),
            mode: InputMode::Hiragana,
            context_before: "下".to_owned(),
        });
        state.phase = Phase::Direct;
        assert_eq!(state.preedit(), "▼かぱ【下▼か*え【】】");
    }
}
