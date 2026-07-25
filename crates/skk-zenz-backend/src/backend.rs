use anyhow::Result;
use std::collections::{HashMap, HashSet};

use crate::candidate::{Candidate, CandidateSource, DictionaryCandidate};
use crate::config::{BackendConfig, CandidateOrderConfig, CandidateSortKey, Fcitx5Config};
use crate::dictionary::{Dictionary, SkkFileDictionary, UserDictionary, open_system_dictionaries};
use crate::input::{
    Composition, InputEvent, InputMode, InputState, InputUpdate, Phase, Registration,
    SelectionOrigin,
};
use crate::keymap::{KeyEvent, Keymap, Modifiers};
use crate::numeric::NumericInput;
use crate::rom_kana::{KanaConversion, KanaMode, RomKanaRule, convert_kana_script};
use crate::zenz::{RerankUpdate, Score, ZenzWorker};

#[derive(Clone, Debug)]
struct ActiveRerank {
    generation: u64,
    order: Vec<CandidateSortKey>,
    selection_touched: bool,
    evaluated: usize,
    total: usize,
}

pub struct Backend {
    scorer: ZenzWorker,
    system_dictionaries: Vec<SkkFileDictionary>,
    user_dictionary: UserDictionary,
    rom_kana_rule: RomKanaRule,
    keymaps: HashMap<InputMode, Keymap>,
    candidate_order: CandidateOrderConfig,
    input_state: InputState,
    active_rerank: Option<ActiveRerank>,
    fcitx5: Fcitx5Config,
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
            scorer: ZenzWorker::load(config.zenz)?,
            system_dictionaries: open_system_dictionaries(&config.system_dictionaries)?,
            user_dictionary: UserDictionary::open(
                &config.user_dictionary,
                config.recency_half_life_days,
            )?,
            rom_kana_rule: RomKanaRule::load(&config.rom_kana_rule, &config.rule_search_paths)?,
            keymaps,
            candidate_order: config.candidate_order,
            input_state: InputState::default(),
            active_rerank: None,
            fcitx5: config.fcitx5,
        })
    }

    pub fn clear_cache(&mut self) {
        self.scorer.clear();
        self.active_rerank = None;
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
        let (consumed, mut commit) = match event {
            InputEvent::SetInputMode(mode) => {
                let output = self.finish_input()?;
                self.input_state.phase = Phase::Direct;
                self.input_state.direct_pending.clear();
                self.input_state.mode = mode;
                if let Some(output) = &output {
                    self.input_state.context.push_str(output);
                }
                (true, output)
            }
            InputEvent::SetContext(context) => {
                if self.input_state.registrations.is_empty() {
                    self.input_state.context = context;
                }
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
        if let Some(output) = commit.take() {
            if let Some(registration) = self.input_state.registrations.last_mut() {
                registration.text.push_str(&output);
            } else {
                commit = Some(output);
            }
        }
        if prepare_speculatively
            && commit.is_none()
            && self.current_order_uses_ai_score()
            && let Some(input) = self.input_state.speculative_input()
        {
            let context = self.input_state.context.clone();
            self.prepare(&context, &input)?;
        }
        Ok(self.input_update(consumed, commit))
    }

    pub fn poll_rerank(&mut self) -> Option<InputUpdate> {
        let mut updates = Vec::new();
        while let Some(update) = self.scorer.try_update() {
            updates.push(update);
        }
        let mut changed = false;
        for update in updates {
            match update {
                RerankUpdate::Candidate {
                    generation,
                    candidate,
                } => {
                    let Some(active) = &mut self.active_rerank else {
                        continue;
                    };
                    if active.generation != generation {
                        continue;
                    }
                    let Phase::Selecting {
                        candidates, cursor, ..
                    } = &mut self.input_state.phase
                    else {
                        continue;
                    };
                    merge_reranked_candidate(
                        candidates,
                        cursor,
                        candidate,
                        &active.order,
                        active.selection_touched,
                    );
                    active.evaluated += 1;
                    changed = true;
                }
                RerankUpdate::Finished { generation } => {
                    if self
                        .active_rerank
                        .as_ref()
                        .is_some_and(|active| active.generation == generation)
                    {
                        self.active_rerank = None;
                        changed = true;
                    }
                }
                RerankUpdate::Failed { generation, error } => {
                    if self
                        .active_rerank
                        .as_ref()
                        .is_some_and(|active| active.generation == generation)
                    {
                        eprintln!("skk-zenz: asynchronous rerank failed: {error}");
                        self.active_rerank = None;
                        changed = true;
                    }
                }
            }
        }
        changed.then(|| self.input_update(false, None))
    }

    pub fn rerank_pending(&self) -> bool {
        self.active_rerank.is_some() && matches!(self.input_state.phase, Phase::Selecting { .. })
    }

    fn input_update(&self, consumed: bool, commit: Option<String>) -> InputUpdate {
        let (candidate_window_visible, candidate_page_start) = match &self.input_state.phase {
            Phase::Selecting { cursor, .. }
                if *cursor >= self.fcitx5.candidate_window_threshold =>
            {
                (true, self.fcitx5.candidate_window_threshold)
            }
            _ => (false, self.fcitx5.candidate_window_threshold),
        };
        InputUpdate {
            consumed,
            preedit: self.input_state.preedit(),
            candidates: self.input_state.candidates(),
            selected_candidate: self.input_state.selected_candidate(),
            commit,
            input_mode: self.input_state.mode,
            rerank_progress: self
                .active_rerank
                .as_ref()
                .filter(|_| self.rerank_pending())
                .map(|active| (active.evaluated, active.total)),
            candidate_window_visible,
            candidate_page_start,
            candidate_page_size: self.fcitx5.page_size,
            show_annotations: self.fcitx5.show_annotations,
        }
    }

    fn current_order_uses_ai_score(&self) -> bool {
        let order = match &self.input_state.phase {
            Phase::Preedit { okuri: Some(_), .. } => &self.candidate_order.okuri_ari,
            Phase::Preedit { okuri: None, .. } => &self.candidate_order.okuri_nasi,
            Phase::Abbrev { .. } => &self.candidate_order.abbrev,
            Phase::Direct | Phase::Selecting { .. } => return false,
        };
        order.contains(&CandidateSortKey::AiScore)
    }

    pub fn lookup(
        &mut self,
        context: &str,
        input: &str,
        okuri: Option<&str>,
    ) -> Result<Vec<Candidate>> {
        eprintln!("[BACKEND] lookup context={context:?} input={input:?} okuri={okuri:?}");
        let order = if okuri.is_some() {
            self.candidate_order.okuri_ari.clone()
        } else {
            self.candidate_order.okuri_nasi.clone()
        };
        eprintln!("[BACKEND] lookup order={order:?}");
        let numeric_input = NumericInput::extract(input);
        let dictionary_candidates =
            self.lookup_dictionaries(input, okuri, numeric_input.as_ref())?;

        let mut candidates = Vec::with_capacity(dictionary_candidates.len());
        for candidate in dictionary_candidates {
            for text in self.expand_numeric_candidate(&candidate, numeric_input.as_ref())? {
                let output = format!("{text}{}", candidate.okuri.as_deref().unwrap_or_default());
                candidates.push(Candidate {
                    input: candidate.input.clone(),
                    okuri: candidate.okuri.clone(),
                    text: candidate.text.clone(),
                    output,
                    annotation: candidate.annotation.clone(),
                    source: candidate.source,
                    loss: 0.0,
                    token_count: 0,
                    recency: candidate.recency,
                });
            }
        }
        let candidates = self.rerank_candidates(context, input, candidates, &order)?;
        eprint!("[BACKEND] lookup -> ");
        for candidate in &candidates {
            eprint!(
                "{:?}({}, {:?}) ",
                candidate.output, candidate.loss, candidate.recency
            );
        }
        Ok(candidates)
    }

    fn lookup_abbrev(&mut self, input: &str) -> Result<Vec<Candidate>> {
        let order = self.candidate_order.abbrev.clone();
        eprintln!("[BACKEND] abbr lookup order={order:?}");
        let context = self.input_state.context.clone();
        let numeric_input = NumericInput::extract(input);
        let dictionary_candidates =
            self.lookup_dictionaries(input, None, numeric_input.as_ref())?;
        let mut candidates = Vec::with_capacity(dictionary_candidates.len());
        for candidate in dictionary_candidates {
            for output in self.expand_numeric_candidate(&candidate, numeric_input.as_ref())? {
                candidates.push(Candidate {
                    input: candidate.input.clone(),
                    okuri: None,
                    text: candidate.text.clone(),
                    output,
                    annotation: candidate.annotation.clone(),
                    source: candidate.source,
                    loss: 0.0,
                    token_count: 0,
                    recency: candidate.recency,
                });
            }
        }
        self.rerank_candidates(&context, input, candidates, &order)
    }

    fn rerank_candidates(
        &mut self,
        context: &str,
        input: &str,
        mut candidates: Vec<Candidate>,
        order: &[CandidateSortKey],
    ) -> Result<Vec<Candidate>> {
        let initial_order: Vec<_> = order
            .iter()
            .copied()
            .filter(|key| *key != CandidateSortKey::AiScore)
            .collect();
        candidates.sort_by(|left, right| compare_candidates(left, right, &initial_order));
        deduplicate_candidate_outputs(&mut candidates);
        if candidates.is_empty() {
            self.active_rerank = None;
            return Ok(candidates);
        }
        if !order.contains(&CandidateSortKey::AiScore) {
            self.active_rerank = None;
            return Ok(candidates);
        }

        let total = candidates.len();
        let (generation, mut initial) = self.scorer.rerank(context, input, candidates)?;
        let evaluated = initial.len();
        initial.sort_by(|left, right| compare_candidates(left, right, order));
        self.active_rerank = Some(ActiveRerank {
            generation,
            order: order.to_vec(),
            selection_touched: false,
            evaluated,
            total,
        });
        Ok(initial)
    }

    fn lookup_dictionaries(
        &self,
        input: &str,
        okuri: Option<&str>,
        numeric_input: Option<&NumericInput>,
    ) -> Result<Vec<DictionaryCandidate>> {
        let mut candidates = self.lookup_dictionary_key(input, okuri)?;
        if let Some(numeric_input) = numeric_input
            && numeric_input.key != input
        {
            candidates.extend(self.lookup_dictionary_key(&numeric_input.key, okuri)?);
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

    fn lookup_dictionary_key(
        &self,
        input: &str,
        okuri: Option<&str>,
    ) -> Result<Vec<DictionaryCandidate>> {
        let mut candidates = self.user_dictionary.lookup(input, okuri)?;
        for dictionary in &self.system_dictionaries {
            candidates.extend(dictionary.lookup(input, okuri)?);
        }
        Ok(candidates)
    }

    fn expand_numeric_candidate(
        &self,
        candidate: &DictionaryCandidate,
        numeric_input: Option<&NumericInput>,
    ) -> Result<Vec<String>> {
        let Some(numeric_input) = numeric_input.filter(|numeric| candidate.input == numeric.key)
        else {
            return Ok(vec![candidate.text.clone()]);
        };
        numeric_input.expand(&candidate.text, |value| {
            self.lookup_dictionary_key(value, None).map(|candidates| {
                candidates
                    .into_iter()
                    .map(|candidate| candidate.text)
                    .collect()
            })
        })
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
        self.user_dictionary.select(&dictionary_candidate)
    }

    pub fn purge(&mut self, candidate: &Candidate) -> Result<bool> {
        self.user_dictionary.purge(
            &candidate.input,
            candidate.okuri.as_deref(),
            &candidate.text,
        )
    }

    fn handle_key(&mut self, event: KeyEvent) -> Result<(bool, Option<String>)> {
        let command = self
            .keymaps
            .get(&self.input_state.mode)
            .and_then(|keymap| keymap.lookup(&event))
            .map(str::to_owned);
        if matches!(
            command.as_deref(),
            Some("next-candidate" | "previous-candidate")
        ) && let Some(active) = &mut self.active_rerank
        {
            active.selection_touched = true;
        }
        if !self.input_state.registrations.is_empty()
            && matches!(self.input_state.phase, Phase::Direct)
        {
            match command.as_deref() {
                Some("abort") => return Ok(self.abort_registration()),
                Some("delete") if self.input_state.direct_pending.is_empty() => {
                    return Ok(self.delete_registration_text());
                }
                Some("commit" | "commit-unhandled")
                    if self.input_state.direct_pending.is_empty() =>
                {
                    return self.finish_registration(command.as_deref());
                }
                _ => {}
            }
        }
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
        let Some(text) = input_text(&event, command) else {
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
        if is_upper_input(&event, command) {
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
        let Some(text) = input_text(&event, command) else {
            self.input_state.phase = Phase::Preedit { stem, okuri };
            return Ok((false, None));
        };
        if okuri.is_none() && should_start_okuri(&event, command, &stem) {
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
            let text = if is_upper_input(&event, command) {
                text.to_ascii_lowercase()
            } else {
                text.to_owned()
            };
            okuri.push(&self.rom_kana_rule, &text, self.input_state.mode.into());
            if okuri.has_output() {
                return self.start_lookup(stem, Some(okuri.clone()));
            }
        } else {
            let text = if is_upper_input(&event, command) {
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
                    self.begin_registration(input.clone(), None, Phase::Abbrev { input }, None);
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
        let Some(text) = input_text(&event, command) else {
            self.input_state.phase = Phase::Abbrev { input };
            return Ok((false, None));
        };
        input.push_str(text);
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
            self.input_state.phase = selection_source_phase(input, okuri, origin);
            return Ok((true, None));
        }
        match command {
            Some("next-candidate") => {
                if let Some(next_cursor) = next_candidate_cursor(
                    cursor,
                    candidates.len(),
                    self.fcitx5.candidate_window_threshold,
                    self.fcitx5.page_size,
                ) {
                    cursor = next_cursor;
                } else {
                    let source_phase = selection_source_phase(input.clone(), okuri.clone(), origin);
                    let empty_return_phase = Phase::Selecting {
                        input: input.clone(),
                        okuri: okuri.clone(),
                        candidates,
                        cursor,
                        origin,
                    };
                    self.begin_registration(input, okuri, source_phase, Some(empty_return_phase));
                    return Ok((true, None));
                }
            }
            Some("previous-candidate") => {
                cursor = previous_candidate_cursor(
                    cursor,
                    self.fcitx5.candidate_window_threshold,
                    self.fcitx5.page_size,
                )
                .expect("cursor 0 is restored to preedit before this branch");
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
            _ if should_forward_after_selection(&event, command) => {
                if let Some(candidate) = candidates.get(cursor).cloned() {
                    self.select(&candidate)?;
                    self.input_state.context.push_str(&candidate.output);
                    self.input_state.phase = Phase::Direct;
                    let (consumed, following_commit) = self.handle_direct(event, command)?;
                    let mut commit = candidate.output;
                    if let Some(following_commit) = following_commit {
                        commit.push_str(&following_commit);
                    }
                    return Ok((consumed, Some(commit)));
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
            self.begin_registration(input, okuri_text, Phase::Preedit { stem, okuri }, None);
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

    fn begin_registration(
        &mut self,
        input: String,
        okuri: Option<String>,
        source_phase: Phase,
        empty_return_phase: Option<Phase>,
    ) {
        self.input_state.registrations.push(Registration {
            input,
            okuri,
            source_phase,
            empty_return_phase,
            text: String::new(),
            mode: self.input_state.mode,
            context_before: self.input_state.context.clone(),
        });
        self.input_state.phase = Phase::Direct;
        self.input_state.direct_pending.clear();
    }

    fn abort_registration(&mut self) -> (bool, Option<String>) {
        let Some(registration) = self.input_state.registrations.pop() else {
            return (false, None);
        };
        self.input_state.mode = registration.mode;
        self.input_state.context = registration.context_before;
        self.input_state.phase = registration.source_phase;
        self.input_state.direct_pending.clear();
        (true, None)
    }

    fn delete_registration_text(&mut self) -> (bool, Option<String>) {
        let Some(registration) = self.input_state.registrations.last_mut() else {
            return (false, None);
        };
        if registration.text.pop().is_some() {
            self.input_state.context.pop();
        }
        (true, None)
    }

    fn finish_registration(&mut self, command: Option<&str>) -> Result<(bool, Option<String>)> {
        let Some(registration) = self.input_state.registrations.pop() else {
            return Ok((false, None));
        };
        if registration.text.is_empty() {
            self.input_state.mode = registration.mode;
            self.input_state.context = registration.context_before;
            self.input_state.phase = registration
                .empty_return_phase
                .unwrap_or(registration.source_phase);
            return Ok((true, None));
        }

        self.register(
            &registration.input,
            registration.okuri.as_deref(),
            &registration.text,
            None,
        )?;
        self.input_state.mode = registration.mode;
        self.input_state.phase = Phase::Direct;
        let nested = !self.input_state.registrations.is_empty();
        Ok((nested || command == Some("commit"), Some(registration.text)))
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

fn upper_command_text(command: Option<&str>) -> Option<&str> {
    command
        .and_then(|command| command.strip_prefix("upper-"))
        .filter(|text| !text.is_empty())
}

fn input_text<'a>(event: &'a KeyEvent, command: Option<&'a str>) -> Option<&'a str> {
    upper_command_text(command).or(event.text.as_deref())
}

fn is_upper_input(event: &KeyEvent, command: Option<&str>) -> bool {
    upper_command_text(command).is_some() || is_preedit_start(event)
}

fn should_start_okuri(event: &KeyEvent, command: Option<&str>, stem: &Composition) -> bool {
    is_upper_input(event, command) && stem.has_output() && !stem.has_pending()
}

fn should_restore_preedit(command: Option<&str>, cursor: usize) -> bool {
    command == Some("abort") || command == Some("previous-candidate") && cursor == 0
}

fn selection_source_phase(input: String, okuri: Option<String>, origin: SelectionOrigin) -> Phase {
    match origin {
        SelectionOrigin::Preedit => Phase::Preedit {
            stem: Composition::from_output(input),
            okuri: okuri.map(Composition::from_output),
        },
        SelectionOrigin::Abbrev => Phase::Abbrev { input },
    }
}

fn should_forward_after_selection(event: &KeyEvent, command: Option<&str>) -> bool {
    command == Some("delete") || input_text(event, command).is_some()
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

fn next_candidate_cursor(
    cursor: usize,
    candidate_count: usize,
    window_threshold: usize,
    page_size: usize,
) -> Option<usize> {
    let next = if cursor < window_threshold {
        cursor.saturating_add(1)
    } else {
        let page = (cursor - window_threshold) / page_size;
        window_threshold.saturating_add(page.saturating_add(1).saturating_mul(page_size))
    };
    (next < candidate_count).then_some(next)
}

fn previous_candidate_cursor(
    cursor: usize,
    window_threshold: usize,
    page_size: usize,
) -> Option<usize> {
    if cursor <= window_threshold {
        cursor.checked_sub(1)
    } else {
        Some(cursor.saturating_sub(page_size).max(window_threshold))
    }
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

fn compare_candidates(
    left: &Candidate,
    right: &Candidate,
    order: &[CandidateSortKey],
) -> std::cmp::Ordering {
    order
        .iter()
        .map(|key| match key {
            CandidateSortKey::FromUser => {
                source_order(left.source).cmp(&source_order(right.source))
            }
            CandidateSortKey::Recency => right
                .recency
                .unwrap_or_default()
                .total_cmp(&left.recency.unwrap_or_default()),
            CandidateSortKey::AiScore => left.loss.total_cmp(&right.loss),
        })
        .find(|ordering| !ordering.is_eq())
        .unwrap_or(std::cmp::Ordering::Equal)
}

fn deduplicate_candidate_outputs(candidates: &mut Vec<Candidate>) {
    let mut seen = HashSet::new();
    candidates.retain(|candidate| seen.insert(candidate.output.clone()));
}

fn merge_reranked_candidate(
    candidates: &mut Vec<Candidate>,
    cursor: &mut usize,
    candidate: Candidate,
    order: &[CandidateSortKey],
    preserve_selection: bool,
) {
    let selected = candidates
        .get(*cursor)
        .map(|candidate| candidate.output.clone());
    candidates.push(candidate);
    candidates.sort_by(|left, right| compare_candidates(left, right, order));
    if !preserve_selection {
        *cursor = 0;
    } else if let Some(selected) = selected
        && let Some(index) = candidates
            .iter()
            .position(|candidate| candidate.output == selected)
    {
        *cursor = index;
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
            recency: candidate.recency,
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
        assert!(!should_start_okuri(&event, None, &stem));

        stem.push(&rule, "a", KanaMode::Hiragana);
        assert_eq!(stem.display(), "か");
        assert!(should_start_okuri(&event, None, &stem));

        stem.push(&rule, "k", KanaMode::Hiragana);
        assert_eq!(stem.display(), "かk");
        assert!(!should_start_okuri(&event, None, &stem));
    }

    #[test]
    fn upper_command_uses_lower_text_and_upper_semantics() {
        let event = KeyEvent::character(":", ':', Modifiers::SHIFT);
        assert_eq!(input_text(&event, Some("upper-;")), Some(";"));
        assert!(is_upper_input(&event, Some("upper-;")));

        let mut stem = Composition::default();
        stem.push(&rule(), "ka", KanaMode::Hiragana);
        assert!(should_start_okuri(&event, Some("upper-;"), &stem));
    }

    #[test]
    fn first_previous_candidate_returns_to_preedit() {
        assert!(should_restore_preedit(Some("previous-candidate"), 0));
        assert!(!should_restore_preedit(Some("previous-candidate"), 2));
        assert!(!should_restore_preedit(Some("delete"), 2));
        assert!(should_restore_preedit(Some("abort"), 2));
    }

    #[test]
    fn regular_input_is_forwarded_after_committing_selection() {
        assert!(should_forward_after_selection(
            &KeyEvent::character("n", 'n', Modifiers::empty()),
            None
        ));
        assert!(!should_forward_after_selection(
            &KeyEvent::named("Shift_L", Modifiers::SHIFT),
            None
        ));
        assert!(should_forward_after_selection(
            &KeyEvent::character(":", ':', Modifiers::SHIFT),
            Some("upper-;")
        ));
        assert!(should_forward_after_selection(
            &KeyEvent::named("BackSpace", Modifiers::empty()),
            Some("delete")
        ));
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
    fn candidate_order_follows_configured_key_priority() {
        let candidate = |recency, loss| Candidate {
            input: String::new(),
            okuri: None,
            text: String::new(),
            output: String::new(),
            annotation: None,
            source: CandidateSource::System,
            loss,
            token_count: 0,
            recency,
        };
        let frequent = candidate(Some(2.0), 2.0);
        let accurate = candidate(Some(1.0), 1.0);
        assert_eq!(
            compare_candidates(
                &frequent,
                &accurate,
                &[CandidateSortKey::Recency, CandidateSortKey::AiScore],
            ),
            std::cmp::Ordering::Less
        );
        assert_eq!(
            compare_candidates(
                &frequent,
                &accurate,
                &[CandidateSortKey::AiScore, CandidateSortKey::Recency],
            ),
            std::cmp::Ordering::Greater
        );
    }

    #[test]
    fn asynchronous_rerank_preserves_selected_candidate() {
        let candidate = |output: &str, loss| Candidate {
            input: String::new(),
            okuri: None,
            text: output.to_owned(),
            output: output.to_owned(),
            annotation: None,
            source: CandidateSource::System,
            loss,
            token_count: 1,
            recency: None,
        };
        let mut candidates = vec![candidate("second", 2.0), candidate("third", 3.0)];
        let mut cursor = 1;

        merge_reranked_candidate(
            &mut candidates,
            &mut cursor,
            candidate("first", 1.0),
            &[CandidateSortKey::AiScore],
            true,
        );

        assert_eq!(
            candidates
                .iter()
                .map(|candidate| candidate.output.as_str())
                .collect::<Vec<_>>(),
            ["first", "second", "third"]
        );
        assert_eq!(candidates[cursor].output, "third");
    }

    #[test]
    fn asynchronous_rerank_selects_new_best_before_user_interaction() {
        let candidate = |output: &str, loss| Candidate {
            input: String::new(),
            okuri: None,
            text: output.to_owned(),
            output: output.to_owned(),
            annotation: None,
            source: CandidateSource::System,
            loss,
            token_count: 1,
            recency: None,
        };
        let mut candidates = vec![candidate("second", 2.0)];
        let mut cursor = 0;

        merge_reranked_candidate(
            &mut candidates,
            &mut cursor,
            candidate("first", 1.0),
            &[CandidateSortKey::AiScore],
            false,
        );

        assert_eq!(candidates[cursor].output, "first");
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

    #[test]
    fn candidate_cursor_advances_individually_then_by_page() {
        assert_eq!(next_candidate_cursor(0, 20, 3, 8), Some(1));
        assert_eq!(next_candidate_cursor(1, 20, 3, 8), Some(2));
        assert_eq!(next_candidate_cursor(2, 20, 3, 8), Some(3));
        assert_eq!(next_candidate_cursor(3, 20, 3, 8), Some(11));
        assert_eq!(next_candidate_cursor(11, 20, 3, 8), Some(19));
        assert_eq!(next_candidate_cursor(19, 20, 3, 8), None);
    }

    #[test]
    fn candidate_cursor_rewinds_by_page_then_individually() {
        assert_eq!(previous_candidate_cursor(19, 3, 8), Some(11));
        assert_eq!(previous_candidate_cursor(11, 3, 8), Some(3));
        assert_eq!(previous_candidate_cursor(3, 3, 8), Some(2));
        assert_eq!(previous_candidate_cursor(2, 3, 8), Some(1));
        assert_eq!(previous_candidate_cursor(1, 3, 8), Some(0));
        assert_eq!(previous_candidate_cursor(0, 3, 8), None);
    }
}
