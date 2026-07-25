use anyhow::Result;
use std::collections::{HashMap, HashSet};

use crate::candidate::{Candidate, CandidateSource, DictionaryCandidate};
use crate::config::BackendConfig;
use crate::dictionary::{Dictionary, SkkFileDictionary, UserDictionary, open_system_dictionaries};
use crate::input::{
    Composition, InputEvent, InputMode, InputState, InputUpdate, Phase, SelectionOrigin,
};
use crate::keymap::{KeyEvent, Keymap, Modifiers};
use crate::rom_kana::{KanaConversion, KanaMode, RomKanaRule, convert_kana_script};
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
            (InputMode::Latin, "latin"),
            (InputMode::WideLatin, "wide-latin"),
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
        eprintln!("[BACKEND] prepare context={context:?} input={input:?}");
        let result = self.scorer.prepare(context, input);
        eprintln!("[BACKEND] prepare -> {result:?}");
        result
    }

    pub fn convert_romaji(&self, input: &str, mode: KanaMode) -> KanaConversion {
        self.rom_kana_rule.convert(input, mode)
    }

    pub fn handle_event(&mut self, event: InputEvent) -> Result<InputUpdate> {
        let prepare_speculatively = matches!(&event, InputEvent::Key(_));
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
        if prepare_speculatively
            && commit.is_none()
            && let Some(input) = self.input_state.speculative_input()
        {
            let context = self.input_state.context.clone();
            self.prepare(&context, &input)?;
        }
        let update = InputUpdate {
            consumed,
            preedit: self.input_state.preedit(),
            candidates: self.input_state.candidates(),
            selected_candidate: self.input_state.selected_candidate(),
            commit,
            input_mode: self.input_state.mode,
        };
        Ok(update)
    }

    pub fn lookup(
        &mut self,
        context: &str,
        input: &str,
        okuri: Option<&str>,
    ) -> Result<Vec<Candidate>> {
        eprintln!("[BACKEND] lookup context={context:?} input={input:?} okuri={okuri:?}");
        let dictionary_candidates = self.lookup_dictionaries(input, okuri)?;

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
        eprint!("[BACKEND] lookup -> ");
        for candidate in &candidates {
            eprint!(
                "{:?}({}, {:?}, {}) ",
                candidate.output, candidate.loss, candidate.recency, candidate.use_count
            );
        }
        Ok(candidates)
    }

    fn lookup_abbrev(&self, input: &str) -> Result<Vec<Candidate>> {
        self.lookup_dictionaries(input, None)?
            .into_iter()
            .map(|candidate| {
                let key = CandidateKey::from(&candidate);
                Ok(Candidate {
                    input: candidate.input,
                    okuri: None,
                    output: candidate.text.clone(),
                    text: candidate.text,
                    annotation: candidate.annotation,
                    source: candidate.source,
                    loss: 0.0,
                    token_count: 0,
                    recency: self.recency.get(&key).copied(),
                    use_count: candidate.use_count,
                })
            })
            .collect()
    }

    fn lookup_dictionaries(
        &self,
        input: &str,
        okuri: Option<&str>,
    ) -> Result<Vec<DictionaryCandidate>> {
        let mut candidates = self.user_dictionary.lookup(input, okuri)?;
        for dictionary in &self.system_dictionaries {
            candidates.extend(dictionary.lookup(input, okuri)?);
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
            Phase::Abbrev { input } => self.handle_abbrev(event, command.as_deref(), input),
            Phase::Selecting {
                input,
                okuri,
                candidates,
                cursor,
                origin,
            } => self.handle_selecting(
                event,
                command.as_deref(),
                input,
                okuri,
                candidates,
                cursor,
                origin,
            ),
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
        if let Some(mode) = direct_mode_transition(command, &self.input_state.direct_pending) {
            return Ok(self.set_input_mode(mode));
        }
        match command {
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
            Some("abbrev") if supports_abbrev(self.input_state.mode) => {
                let commit = self.finish_direct();
                if let Some(commit) = &commit {
                    self.input_state.context.push_str(commit);
                }
                self.input_state.phase = Phase::Abbrev {
                    input: String::new(),
                };
                return Ok((true, commit));
            }
            _ => {}
        }
        let Some(text) = event.text.as_deref() else {
            return Ok((false, None));
        };
        if matches!(
            self.input_state.mode,
            InputMode::Latin | InputMode::WideLatin
        ) {
            let output = match self.input_state.mode {
                InputMode::Latin => text.to_owned(),
                InputMode::WideLatin => to_wide_latin(text),
                _ => unreachable!("mode checked above"),
            };
            self.input_state.context.push_str(&output);
            return Ok((true, Some(output)));
        }
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
        if let Some(
            mode @ (InputMode::Hiragana | InputMode::Katakana | InputMode::HankakuKatakana),
        ) = command.and_then(input_mode_for_command)
        {
            let output = composition_output(
                &self.rom_kana_rule,
                self.input_state.mode,
                &stem,
                okuri.as_ref(),
            );
            let output = convert_kana_script(&output, mode.into());
            self.input_state.context.push_str(&output);
            self.input_state.phase = Phase::Direct;
            return Ok((true, Some(output)));
        }
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
        if okuri.is_none() && should_start_okuri(&event, &stem) {
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
            let text = if is_preedit_start(&event) {
                text.to_ascii_lowercase()
            } else {
                text.to_owned()
            };
            okuri.push(&self.rom_kana_rule, &text, self.input_state.mode.into());
            if okuri.has_output() {
                return self.start_lookup(stem, Some(okuri.clone()));
            }
        } else {
            let text = if is_preedit_start(&event) {
                text.to_ascii_lowercase()
            } else {
                text.to_owned()
            };
            stem.push(&self.rom_kana_rule, &text, self.input_state.mode.into());
        }
        self.input_state.phase = Phase::Preedit { stem, okuri };
        Ok((true, None))
    }

    fn handle_abbrev(
        &mut self,
        event: KeyEvent,
        command: Option<&str>,
        mut input: String,
    ) -> Result<(bool, Option<String>)> {
        match command {
            Some("abort") => {
                self.input_state.phase = Phase::Direct;
                return Ok((true, None));
            }
            Some("delete") => {
                if input.pop().is_some() {
                    self.input_state.phase = Phase::Abbrev { input };
                } else {
                    self.input_state.phase = Phase::Direct;
                }
                return Ok((true, None));
            }
            Some("next-candidate") => {
                if input.is_empty() {
                    self.input_state.phase = Phase::Abbrev { input };
                    return Ok((true, None));
                }
                let candidates = self.lookup_abbrev(&input)?;
                if candidates.is_empty() {
                    self.input_state.phase = Phase::Abbrev { input };
                } else {
                    self.input_state.phase = Phase::Selecting {
                        input,
                        okuri: None,
                        candidates,
                        cursor: 0,
                        origin: SelectionOrigin::Abbrev,
                    };
                }
                return Ok((true, None));
            }
            Some("commit") | Some("commit-unhandled") => {
                self.input_state.context.push_str(&input);
                self.input_state.phase = Phase::Direct;
                return Ok((command == Some("commit"), Some(input)));
            }
            _ => {}
        }
        let Some(text) = event.text else {
            self.input_state.phase = Phase::Abbrev { input };
            return Ok((false, None));
        };
        input.push_str(&text);
        self.input_state.phase = Phase::Abbrev { input };
        Ok((true, None))
    }

    #[allow(clippy::too_many_arguments)]
    fn handle_selecting(
        &mut self,
        event: KeyEvent,
        command: Option<&str>,
        input: String,
        okuri: Option<String>,
        mut candidates: Vec<Candidate>,
        mut cursor: usize,
        origin: SelectionOrigin,
    ) -> Result<(bool, Option<String>)> {
        if should_restore_preedit(command, cursor) {
            self.input_state.phase = match origin {
                SelectionOrigin::Preedit => Phase::Preedit {
                    stem: Composition::from_output(input),
                    okuri: okuri.map(Composition::from_output),
                },
                SelectionOrigin::Abbrev => Phase::Abbrev { input },
            };
            return Ok((true, None));
        }
        match command {
            Some("next-candidate") => {
                if cursor + 1 < candidates.len() {
                    cursor += 1;
                }
            }
            Some("previous-candidate") => {
                cursor -= 1;
            }
            Some("purge-candidate") => {
                if let Some(candidate) = candidates.get(cursor).cloned() {
                    self.purge(&candidate)?;
                    candidates.remove(cursor);
                    cursor = cursor.min(candidates.len().saturating_sub(1));
                }
            }
            Some("commit") | Some("commit-unhandled") => {
                if let Some(candidate) = candidates.get(cursor).cloned() {
                    self.select(&candidate)?;
                    self.input_state.context.push_str(&candidate.output);
                    self.input_state.phase = Phase::Direct;
                    return Ok((command == Some("commit"), Some(candidate.output)));
                }
            }
            None if should_commit_selection_for_event(&event) => {
                if let Some(candidate) = candidates.get(cursor).cloned() {
                    self.select(&candidate)?;
                    self.input_state.context.push_str(&candidate.output);
                    self.input_state.phase = Phase::Direct;
                    let (_, following_commit) = self.handle_direct(event, None)?;
                    let mut commit = candidate.output;
                    if let Some(following_commit) = following_commit {
                        commit.push_str(&following_commit);
                    }
                    return Ok((true, Some(commit)));
                }
            }
            None => {
                self.input_state.phase = Phase::Selecting {
                    input,
                    okuri,
                    candidates,
                    cursor,
                    origin,
                };
                return Ok((false, None));
            }
            _ => {}
        }
        self.input_state.phase = Phase::Selecting {
            input,
            okuri,
            candidates,
            cursor,
            origin,
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
                origin: SelectionOrigin::Preedit,
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

    fn set_input_mode(&mut self, mode: InputMode) -> (bool, Option<String>) {
        let commit = self.finish_direct();
        if let Some(commit) = &commit {
            self.input_state.context.push_str(commit);
        }
        self.input_state.mode = mode;
        (true, commit)
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
            Phase::Abbrev { input } => Ok(Some(input.clone())),
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

fn should_start_okuri(event: &KeyEvent, stem: &Composition) -> bool {
    is_preedit_start(event) && stem.has_output() && !stem.has_pending()
}

fn should_restore_preedit(command: Option<&str>, cursor: usize) -> bool {
    matches!(command, Some("abort" | "delete"))
        || command == Some("previous-candidate") && cursor == 0
}

fn should_commit_selection_for_event(event: &KeyEvent) -> bool {
    event.text.is_some()
}

fn supports_abbrev(mode: InputMode) -> bool {
    matches!(
        mode,
        InputMode::Hiragana | InputMode::Katakana | InputMode::HankakuKatakana
    )
}

fn input_mode_for_command(command: &str) -> Option<InputMode> {
    match command {
        "set-input-mode-hiragana" => Some(InputMode::Hiragana),
        "set-input-mode-katakana" => Some(InputMode::Katakana),
        "set-input-mode-hankaku-katakana" => Some(InputMode::HankakuKatakana),
        "set-input-mode-latin" => Some(InputMode::Latin),
        "set-input-mode-wide-latin" => Some(InputMode::WideLatin),
        _ => None,
    }
}

fn direct_mode_transition(command: Option<&str>, pending: &str) -> Option<InputMode> {
    pending
        .is_empty()
        .then(|| command.and_then(input_mode_for_command))
        .flatten()
}

fn to_wide_latin(value: &str) -> String {
    value
        .chars()
        .map(|character| match character {
            ' ' => '\u{3000}',
            '!'..='~' => char::from_u32(character as u32 + 0xfee0)
                .expect("ASCII fullwidth mapping is valid Unicode"),
            _ => character,
        })
        .collect()
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

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    fn rule() -> RomKanaRule {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("test-data/rules");
        RomKanaRule::load("base", &[root]).unwrap()
    }

    #[test]
    fn shifted_vowel_completes_pending_stem_instead_of_starting_okuri() {
        let rule = rule();
        let event = KeyEvent::character("A", 'a', Modifiers::SHIFT);
        let mut stem = Composition::default();
        stem.push(&rule, "k", KanaMode::Hiragana);
        assert!(!should_start_okuri(&event, &stem));

        stem.push(&rule, "a", KanaMode::Hiragana);
        assert_eq!(stem.display(), "か");
        assert!(should_start_okuri(&event, &stem));

        stem.push(&rule, "k", KanaMode::Hiragana);
        assert_eq!(stem.display(), "かk");
        assert!(!should_start_okuri(&event, &stem));
    }

    #[test]
    fn first_previous_candidate_returns_to_preedit() {
        assert!(should_restore_preedit(Some("previous-candidate"), 0));
        assert!(!should_restore_preedit(Some("previous-candidate"), 2));
        assert!(should_restore_preedit(Some("delete"), 2));
        assert!(should_restore_preedit(Some("abort"), 2));
    }

    #[test]
    fn printable_key_commits_selection_but_modifier_does_not() {
        assert!(should_commit_selection_for_event(&KeyEvent::character(
            "n",
            'n',
            Modifiers::empty()
        )));
        assert!(!should_commit_selection_for_event(&KeyEvent::named(
            "Shift_L",
            Modifiers::SHIFT
        )));
    }

    #[test]
    fn converts_ascii_to_wide_latin() {
        assert_eq!(to_wide_latin("Az 09!?"), "Ａｚ　０９！？");
        assert_eq!(to_wide_latin("日本語"), "日本語");
    }

    #[test]
    fn maps_all_five_input_mode_commands() {
        assert_eq!(
            input_mode_for_command("set-input-mode-hiragana"),
            Some(InputMode::Hiragana)
        );
        assert_eq!(
            input_mode_for_command("set-input-mode-katakana"),
            Some(InputMode::Katakana)
        );
        assert_eq!(
            input_mode_for_command("set-input-mode-hankaku-katakana"),
            Some(InputMode::HankakuKatakana)
        );
        assert_eq!(
            input_mode_for_command("set-input-mode-latin"),
            Some(InputMode::Latin)
        );
        assert_eq!(
            input_mode_for_command("set-input-mode-wide-latin"),
            Some(InputMode::WideLatin)
        );
    }

    #[test]
    fn abbrev_is_available_only_from_kana_modes() {
        assert!(supports_abbrev(InputMode::Hiragana));
        assert!(supports_abbrev(InputMode::Katakana));
        assert!(supports_abbrev(InputMode::HankakuKatakana));
        assert!(!supports_abbrev(InputMode::Latin));
        assert!(!supports_abbrev(InputMode::WideLatin));
    }

    #[test]
    fn does_not_change_mode_in_the_middle_of_romaji() {
        assert_eq!(
            direct_mode_transition(Some("set-input-mode-latin"), ""),
            Some(InputMode::Latin)
        );
        assert_eq!(
            direct_mode_transition(Some("set-input-mode-latin"), "k"),
            None
        );
    }
}
