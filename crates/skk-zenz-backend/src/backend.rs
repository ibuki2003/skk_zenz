use anyhow::Result;
use std::collections::{HashMap, HashSet};

use crate::candidate::{Candidate, CandidateSource, DictionaryCandidate};
use crate::config::BackendConfig;
use crate::dictionary::{Dictionary, SkkFileDictionary, UserDictionary, open_system_dictionaries};
use crate::input::{Composition, InputEvent, InputMode, InputState, InputUpdate, Phase};
use crate::keymap::{KeyEvent, Keymap, Modifiers};
use crate::rom_kana::{KanaConversion, KanaMode, RomKanaRule};
use crate::zenz::{Score, ZenzScorer};

pub struct Backend {
    scorer: ZenzScorer,
    system_dictionaries: Vec<SkkFileDictionary>,
    user_dictionary: UserDictionary,
    rom_kana_rule: RomKanaRule,
    keymaps: HashMap<InputMode, Keymap>,
    input_state: InputState,
    recency: HashMap<CandidateKey, u64>,
    sequence: u64,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
struct CandidateKey {
    input: String,
    okuri: Option<String>,
    text: String,
}

impl Backend {
    pub fn load(config: BackendConfig) -> Result<Self> {
        let keymaps = [
            (InputMode::Hiragana, "hiragana"),
            (InputMode::Katakana, "katakana"),
            (InputMode::HankakuKatakana, "hankaku-katakana"),
        ]
        .into_iter()
        .map(|(mode, name)| {
            Keymap::load(&config.rom_kana_rule, name, &config.rule_search_paths)
                .map(|keymap| (mode, keymap))
        })
        .collect::<Result<_>>()?;
        Ok(Self {
            scorer: ZenzScorer::load(&config.zenz)?,
            system_dictionaries: open_system_dictionaries(&config.system_dictionaries)?,
            user_dictionary: UserDictionary::open(&config.user_dictionary)?,
            rom_kana_rule: RomKanaRule::load(&config.rom_kana_rule, &config.rule_search_paths)?,
            keymaps,
            input_state: InputState::default(),
            recency: HashMap::new(),
            sequence: 0,
        })
    }

    pub fn clear_cache(&mut self) {
        self.scorer.clear();
    }

    pub fn prepare(&mut self, context: &str, input: &str) -> Result<Score> {
        self.scorer.prepare(context, input)
    }

    pub fn convert_romaji(&self, input: &str, mode: KanaMode) -> KanaConversion {
        self.rom_kana_rule.convert(input, mode)
    }

    pub fn handle_event(&mut self, event: InputEvent) -> Result<InputUpdate> {
        let (consumed, commit) = match event {
            InputEvent::SetContext(context) => {
                self.input_state.context = context;
                (false, None)
            }
            InputEvent::Reset => {
                self.input_state.reset();
                (true, None)
            }
            InputEvent::SelectCandidate(index) => self.select_candidate_at(index)?,
            InputEvent::FocusOut { commit } => {
                let output = if commit { self.finish_input()? } else { None };
                self.input_state.reset();
                if let Some(output) = &output {
                    self.input_state.context.push_str(output);
                }
                (true, output)
            }
            InputEvent::Key(event) => self.handle_key(event)?,
        };
        if commit.is_none()
            && let Some(input) = self.input_state.speculative_input()
        {
            let context = self.input_state.context.clone();
            self.prepare(&context, &input)?;
        }
        Ok(InputUpdate {
            consumed,
            preedit: self.input_state.preedit(),
            candidates: self.input_state.candidates(),
            commit,
            input_mode: self.input_state.mode,
        })
    }

    pub fn lookup(
        &mut self,
        context: &str,
        input: &str,
        okuri: Option<&str>,
    ) -> Result<Vec<Candidate>> {
        let mut dictionary_candidates = self.user_dictionary.lookup(input, okuri)?;
        for dictionary in &self.system_dictionaries {
            dictionary_candidates.extend(dictionary.lookup(input, okuri)?);
        }
        let mut seen = HashSet::new();
        dictionary_candidates.retain(|candidate| {
            seen.insert(format!(
                "{}{}",
                candidate.text,
                candidate.okuri.as_deref().unwrap_or_default()
            ))
        });

        let mut candidates = Vec::with_capacity(dictionary_candidates.len());
        for candidate in dictionary_candidates {
            let output = format!(
                "{}{}",
                candidate.text,
                candidate.okuri.as_deref().unwrap_or_default()
            );
            let score = self.scorer.score(context, input, &output)?;
            let key = CandidateKey::from(&candidate);
            candidates.push(Candidate {
                input: candidate.input,
                okuri: candidate.okuri,
                text: candidate.text,
                output,
                annotation: candidate.annotation,
                source: candidate.source,
                loss: score.loss,
                token_count: score.token_count,
                recency: self.recency.get(&key).copied(),
                use_count: candidate.use_count,
            });
        }
        candidates.sort_by(|left, right| {
            source_order(left.source)
                .cmp(&source_order(right.source))
                .then_with(|| right.recency.cmp(&left.recency))
                .then_with(|| left.loss.total_cmp(&right.loss))
        });
        Ok(candidates)
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
        self.user_dictionary.select(&dictionary_candidate)?;
        self.sequence = self.sequence.wrapping_add(1);
        self.recency
            .insert(CandidateKey::from(&dictionary_candidate), self.sequence);
        Ok(())
    }

    pub fn purge(&mut self, candidate: &Candidate) -> Result<bool> {
        let removed = self.user_dictionary.purge(
            &candidate.input,
            candidate.okuri.as_deref(),
            &candidate.text,
        )?;
        self.recency.remove(&CandidateKey::from(candidate));
        Ok(removed)
    }

    fn handle_key(&mut self, event: KeyEvent) -> Result<(bool, Option<String>)> {
        let command = self
            .keymaps
            .get(&self.input_state.mode)
            .and_then(|keymap| keymap.lookup(&event))
            .map(str::to_owned);
        match std::mem::take(&mut self.input_state.phase) {
            Phase::Direct => self.handle_direct(event, command.as_deref()),
            Phase::Preedit { stem, okuri } => {
                self.handle_preedit(event, command.as_deref(), stem, okuri)
            }
            Phase::Selecting {
                input,
                okuri,
                candidates,
                cursor,
            } => self.handle_selecting(event, command.as_deref(), input, okuri, candidates, cursor),
        }
    }

    fn select_candidate_at(&mut self, index: usize) -> Result<(bool, Option<String>)> {
        let candidate = match &self.input_state.phase {
            Phase::Selecting { candidates, .. } => candidates.get(index).cloned(),
            _ => None,
        };
        let Some(candidate) = candidate else {
            return Ok((false, None));
        };
        self.select(&candidate)?;
        self.input_state.context.push_str(&candidate.output);
        self.input_state.phase = Phase::Direct;
        Ok((true, Some(candidate.output)))
    }

    fn handle_direct(
        &mut self,
        event: KeyEvent,
        command: Option<&str>,
    ) -> Result<(bool, Option<String>)> {
        self.input_state.phase = Phase::Direct;
        match command {
            Some("set-input-mode-hiragana") => {
                self.input_state.mode = InputMode::Hiragana;
                return Ok((true, None));
            }
            Some("set-input-mode-katakana") => {
                self.input_state.mode = InputMode::Katakana;
                return Ok((true, None));
            }
            Some("set-input-mode-hankaku-katakana") => {
                self.input_state.mode = InputMode::HankakuKatakana;
                return Ok((true, None));
            }
            Some("delete") => {
                return Ok((self.input_state.direct_pending.pop().is_some(), None));
            }
            Some("commit") | Some("commit-unhandled") => {
                let commit = self.finish_direct();
                if let Some(commit) = &commit {
                    self.input_state.context.push_str(commit);
                }
                return Ok((command == Some("commit"), commit));
            }
            Some("start-preedit") => {
                self.input_state.phase = Phase::Preedit {
                    stem: Composition::default(),
                    okuri: None,
                };
                return Ok((true, None));
            }
            _ => {}
        }
        let Some(text) = event.text.as_deref() else {
            return Ok((false, None));
        };
        if is_preedit_start(&event) {
            let mut stem = Composition::default();
            stem.push(
                &self.rom_kana_rule,
                &text.to_ascii_lowercase(),
                self.input_state.mode.into(),
            );
            self.input_state.phase = Phase::Preedit { stem, okuri: None };
            return Ok((true, None));
        }
        let input = format!("{}{text}", self.input_state.direct_pending);
        let conversion = self
            .rom_kana_rule
            .convert_partial(&input, self.input_state.mode.into());
        self.input_state.direct_pending = conversion.preedit;
        if conversion.output.is_empty() {
            Ok((true, None))
        } else {
            self.input_state.context.push_str(&conversion.output);
            Ok((true, Some(conversion.output)))
        }
    }

    fn handle_preedit(
        &mut self,
        event: KeyEvent,
        command: Option<&str>,
        mut stem: Composition,
        mut okuri: Option<Composition>,
    ) -> Result<(bool, Option<String>)> {
        match command {
            Some("abort") => {
                self.input_state.phase = Phase::Direct;
                return Ok((true, None));
            }
            Some("delete") => {
                if okuri.as_mut().is_some_and(Composition::pop) {
                    if okuri
                        .as_ref()
                        .is_some_and(|value| value.display().is_empty())
                    {
                        okuri = None;
                    }
                } else if !stem.pop() {
                    self.input_state.phase = Phase::Direct;
                    return Ok((true, None));
                }
                self.input_state.phase = Phase::Preedit { stem, okuri };
                return Ok((true, None));
            }
            Some("next-candidate") => {
                return self.start_lookup(stem, okuri);
            }
            Some("commit") | Some("commit-unhandled") => {
                let output = composition_output(
                    &self.rom_kana_rule,
                    self.input_state.mode,
                    &stem,
                    okuri.as_ref(),
                );
                self.input_state.context.push_str(&output);
                self.input_state.phase = Phase::Direct;
                return Ok((command == Some("commit"), Some(output)));
            }
            _ => {}
        }
        let Some(text) = event.text.as_deref() else {
            self.input_state.phase = Phase::Preedit { stem, okuri };
            return Ok((false, None));
        };
        if okuri.is_none() && is_preedit_start(&event) && !stem.display().is_empty() {
            let mut value = Composition::default();
            value.push(
                &self.rom_kana_rule,
                &text.to_ascii_lowercase(),
                self.input_state.mode.into(),
            );
            let resolved = !value.display().is_empty() && value.has_output();
            okuri = Some(value);
            if resolved {
                return self.start_lookup(stem, okuri);
            }
        } else if let Some(okuri) = &mut okuri {
            okuri.push(&self.rom_kana_rule, text, self.input_state.mode.into());
        } else {
            stem.push(&self.rom_kana_rule, text, self.input_state.mode.into());
        }
        self.input_state.phase = Phase::Preedit { stem, okuri };
        Ok((true, None))
    }

    #[allow(clippy::too_many_arguments)]
    fn handle_selecting(
        &mut self,
        _event: KeyEvent,
        command: Option<&str>,
        input: String,
        okuri: Option<String>,
        mut candidates: Vec<Candidate>,
        mut cursor: usize,
    ) -> Result<(bool, Option<String>)> {
        match command {
            Some("next-candidate") => {
                if cursor + 1 < candidates.len() {
                    cursor += 1;
                }
            }
            Some("previous-candidate") => {
                cursor = cursor.saturating_sub(1);
            }
            Some("abort") => {
                self.input_state.phase = Phase::Preedit {
                    stem: Composition::from_output(input),
                    okuri: okuri.map(Composition::from_output),
                };
                return Ok((true, None));
            }
            Some("purge-candidate") => {
                if let Some(candidate) = candidates.get(cursor).cloned() {
                    self.purge(&candidate)?;
                    candidates.remove(cursor);
                    cursor = cursor.min(candidates.len().saturating_sub(1));
                }
            }
            Some("commit") | Some("commit-unhandled") | None => {
                if let Some(candidate) = candidates.get(cursor).cloned() {
                    self.select(&candidate)?;
                    self.input_state.context.push_str(&candidate.output);
                    self.input_state.phase = Phase::Direct;
                    return Ok((command == Some("commit"), Some(candidate.output)));
                }
            }
            _ => {}
        }
        self.input_state.phase = Phase::Selecting {
            input,
            okuri,
            candidates,
            cursor,
        };
        Ok((true, None))
    }

    fn start_lookup(
        &mut self,
        stem: Composition,
        okuri: Option<Composition>,
    ) -> Result<(bool, Option<String>)> {
        let input = stem.finish(&self.rom_kana_rule, self.input_state.mode.into());
        let okuri_text = okuri
            .as_ref()
            .map(|value| value.finish(&self.rom_kana_rule, self.input_state.mode.into()));
        let context = self.input_state.context.clone();
        let candidates = self.lookup(&context, &input, okuri_text.as_deref())?;
        if candidates.is_empty() {
            self.input_state.phase = Phase::Preedit { stem, okuri };
        } else {
            self.input_state.phase = Phase::Selecting {
                input,
                okuri: okuri_text,
                candidates,
                cursor: 0,
            };
        }
        Ok((true, None))
    }

    fn finish_direct(&mut self) -> Option<String> {
        if self.input_state.direct_pending.is_empty() {
            return None;
        }
        let output = self.rom_kana_rule.convert(
            &self.input_state.direct_pending,
            self.input_state.mode.into(),
        );
        self.input_state.direct_pending.clear();
        let value = format!("{}{}", output.output, output.preedit);
        Some(value)
    }

    fn finish_input(&mut self) -> Result<Option<String>> {
        match &self.input_state.phase {
            Phase::Direct => Ok(self.finish_direct()),
            Phase::Preedit { stem, okuri } => Ok(Some(composition_output(
                &self.rom_kana_rule,
                self.input_state.mode,
                stem,
                okuri.as_ref(),
            ))),
            Phase::Selecting {
                candidates, cursor, ..
            } => {
                let candidate = candidates.get(*cursor).cloned();
                if let Some(candidate) = &candidate {
                    self.select(candidate)?;
                }
                Ok(candidate.map(|candidate| candidate.output))
            }
        }
    }
}

fn is_preedit_start(event: &KeyEvent) -> bool {
    event.modifiers == Modifiers::SHIFT
        && event
            .text
            .as_deref()
            .is_some_and(|text| text.len() == 1 && text.as_bytes()[0].is_ascii_uppercase())
}

fn composition_output(
    rule: &RomKanaRule,
    mode: InputMode,
    stem: &Composition,
    okuri: Option<&Composition>,
) -> String {
    let mut output = stem.finish(rule, mode.into());
    if let Some(okuri) = okuri {
        output.push_str(&okuri.finish(rule, mode.into()));
    }
    output
}

fn source_order(source: CandidateSource) -> u8 {
    match source {
        CandidateSource::User => 0,
        CandidateSource::System => 1,
    }
}

impl From<&DictionaryCandidate> for CandidateKey {
    fn from(candidate: &DictionaryCandidate) -> Self {
        Self {
            input: candidate.input.clone(),
            okuri: candidate.okuri.clone(),
            text: candidate.text.clone(),
        }
    }
}

impl From<&Candidate> for CandidateKey {
    fn from(candidate: &Candidate) -> Self {
        Self {
            input: candidate.input.clone(),
            okuri: candidate.okuri.clone(),
            text: candidate.text.clone(),
        }
    }
}

impl From<&Candidate> for DictionaryCandidate {
    fn from(candidate: &Candidate) -> Self {
        Self {
            input: candidate.input.clone(),
            okuri: candidate.okuri.clone(),
            text: candidate.text.clone(),
            annotation: candidate.annotation.clone(),
            source: candidate.source,
            use_count: candidate.use_count,
        }
    }
}
