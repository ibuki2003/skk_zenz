use super::helpers::*;
use super::*;

impl Backend {
    pub(super) fn handle_key(&mut self, event: KeyEvent) -> Result<(bool, Option<String>)> {
        let command = self
            .keymaps
            .get(&self.input_state.mode)
            .and_then(|keymap| keymap.lookup(&event))
            .map(str::to_owned);
        let command = command_for_rom_kana(
            command.as_deref(),
            &self.rom_kana_rule,
            self.input_state.mode,
        );
        if !matches!(
            command,
            Some("complete" | "previous-completion" | "previous")
        ) {
            self.input_state.completion = None;
        }
        if matches!(command, Some("next-candidate" | "previous-candidate"))
            && let Some(active) = &mut self.active_rerank
        {
            active.selection_touched = true;
        }
        if !self.input_state.registrations.is_empty()
            && matches!(self.input_state.phase, Phase::Direct)
        {
            match command {
                Some("abort") => return Ok(self.abort_registration()),
                Some("delete") if self.input_state.direct_pending.is_empty() => {
                    return Ok(self.delete_registration_text());
                }
                Some("commit" | "commit-unhandled")
                    if self.input_state.direct_pending.is_empty() =>
                {
                    return self.finish_registration(command);
                }
                Some("complete") if self.input_state.direct_pending.is_empty() => {
                    return self.start_registration_generation();
                }
                _ => {}
            }
        }
        match std::mem::take(&mut self.input_state.phase) {
            Phase::Direct => self.handle_direct(event, command),
            Phase::Preedit { stem, okuri } => self.handle_preedit(event, command, stem, okuri),
            Phase::Abbrev { input } => self.handle_abbrev(event, command, input),
            Phase::Selecting(selection) => self.handle_selecting(event, command, selection),
        }
    }

    pub(super) fn select_candidate_at(&mut self, index: usize) -> Result<(bool, Option<String>)> {
        let selection = match &self.input_state.phase {
            Phase::Selecting(selection) => {
                selection.candidates.get(index).cloned().map(|candidate| {
                    (
                        selection.input.clone(),
                        selection.okuri.clone(),
                        candidate,
                        selection.origin,
                    )
                })
            }
            _ => None,
        };
        let Some((input, okuri, candidate, origin)) = selection else {
            return Ok((false, None));
        };
        if origin == SelectionOrigin::RegistrationGeneration {
            return self.confirm_generated_registration(&candidate, Some("commit"), true);
        }
        self.confirm_candidate(&input, okuri.as_deref(), &candidate)?;
        self.input_state.context.push_str(&candidate.output);
        self.input_state.phase = Phase::Direct;
        Ok((true, Some(candidate.output)))
    }

    pub(super) fn handle_direct(
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
            Some("special-midasi") => return Ok((false, None)),
            Some("abbrev")
                if should_start_abbrev(
                    command,
                    self.input_state.mode,
                    &self.input_state.direct_pending,
                ) =>
            {
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
            self.input_state.direct_pending.clear();
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

    pub(super) fn handle_preedit(
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
            Some(command @ ("complete" | "previous-completion" | "previous"))
                if okuri.is_none() =>
            {
                stem = self.complete_preedit(stem, command != "complete")?;
                self.input_state.phase = Phase::Preedit { stem, okuri };
                return Ok((true, None));
            }
            Some("special-midasi") if okuri.is_none() => {
                let mut input = stem.finish(&self.rom_kana_rule, self.input_state.mode.into());
                input.push('>');
                return self.start_lookup(Composition::from_output(input), None);
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

    fn complete_preedit(&mut self, stem: Composition, backwards: bool) -> Result<Composition> {
        if let Some(completion) = &mut self.input_state.completion {
            let candidate = if backwards {
                completion.retreat()
            } else {
                completion.advance()
            };
            return Ok(Composition::from_output(candidate.to_owned()));
        }

        let prefix = stem.finish(&self.rom_kana_rule, self.input_state.mode.into());
        if prefix.is_empty() {
            return Ok(stem);
        }
        let candidates = self.complete(&prefix)?;
        if candidates.is_empty() {
            return Ok(stem);
        }
        let cursor = if backwards { candidates.len() - 1 } else { 0 };
        let candidate = candidates[cursor].clone();
        self.input_state.completion = Some(CompletionState { candidates, cursor });
        Ok(Composition::from_output(candidate))
    }

    pub(super) fn handle_abbrev(
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
                    self.input_state.phase = Phase::Selecting(CandidateSelection {
                        input,
                        okuri: None,
                        candidates,
                        cursor: 0,
                        origin: SelectionOrigin::Abbrev,
                    });
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

    pub(super) fn handle_selecting(
        &mut self,
        event: KeyEvent,
        command: Option<&str>,
        mut selection: CandidateSelection,
    ) -> Result<(bool, Option<String>)> {
        if selection.origin == SelectionOrigin::RegistrationGeneration {
            return self.handle_registration_generation_selection(event, command, selection);
        }
        if should_restore_preedit(command, selection.cursor) {
            self.input_state.phase =
                selection_source_phase(selection.input, selection.okuri, selection.origin);
            return Ok((true, None));
        }
        match command {
            Some("next-candidate") => {
                if let Some(next_cursor) = next_candidate_cursor(
                    selection.cursor,
                    selection.candidates.len(),
                    self.fcitx5.candidate_window_threshold,
                    self.fcitx5.page_size,
                ) {
                    selection.cursor = next_cursor;
                } else {
                    let source_phase = selection_source_phase(
                        selection.input.clone(),
                        selection.okuri.clone(),
                        selection.origin,
                    );
                    let input = selection.input.clone();
                    let okuri = selection.okuri.clone();
                    self.begin_registration(
                        input,
                        okuri,
                        source_phase,
                        Some(Phase::Selecting(selection)),
                    );
                    return Ok((true, None));
                }
            }
            Some("previous-candidate" | "previous") => {
                selection.cursor = previous_candidate_cursor(
                    selection.cursor,
                    self.fcitx5.candidate_window_threshold,
                    self.fcitx5.page_size,
                )
                .expect("cursor 0 is restored to preedit before this branch");
            }
            Some("purge-candidate") => {
                if let Some(candidate) = selection.candidates.get(selection.cursor).cloned() {
                    self.purge(&candidate)?;
                    selection.candidates.remove(selection.cursor);
                    selection.cursor = selection
                        .cursor
                        .min(selection.candidates.len().saturating_sub(1));
                }
            }
            Some("commit") | Some("commit-unhandled") => {
                if let Some(candidate) = self.confirm_selected_candidate(&selection)? {
                    self.input_state.context.push_str(&candidate.output);
                    self.input_state.phase = Phase::Direct;
                    return Ok((command == Some("commit"), Some(candidate.output)));
                }
            }
            Some("delete") => {
                if let Some(candidate) = self.confirm_selected_candidate(&selection)? {
                    let mut output = candidate.output;
                    output.pop();
                    self.input_state.context.push_str(&output);
                    self.input_state.phase = Phase::Direct;
                    return Ok((true, Some(output)));
                }
            }
            Some("special-midasi") => {
                if let Some(candidate) = self.confirm_selected_candidate(&selection)? {
                    self.input_state.context.push_str(&candidate.output);
                    self.input_state.phase = Phase::Preedit {
                        stem: Composition::from_output(">".to_owned()),
                        okuri: None,
                    };
                    return Ok((true, Some(candidate.output)));
                }
            }
            _ if should_forward_after_selection(&event, command) => {
                if let Some(candidate) = self.confirm_selected_candidate(&selection)? {
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
                self.input_state.phase = Phase::Selecting(selection);
                return Ok((false, None));
            }
            _ => {}
        }
        self.input_state.phase = Phase::Selecting(selection);
        Ok((true, None))
    }

    fn confirm_selected_candidate(
        &mut self,
        selection: &CandidateSelection,
    ) -> Result<Option<Candidate>> {
        let Some(candidate) = selection.candidates.get(selection.cursor).cloned() else {
            return Ok(None);
        };
        self.confirm_candidate(&selection.input, selection.okuri.as_deref(), &candidate)?;
        Ok(Some(candidate))
    }

    pub(super) fn start_lookup(
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
            self.input_state.phase = Phase::Selecting(CandidateSelection {
                input,
                okuri: okuri_text,
                candidates,
                cursor: 0,
                origin: SelectionOrigin::Preedit,
            });
        }
        Ok((true, None))
    }

    pub(super) fn finish_direct(&mut self) -> Option<String> {
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

    pub(super) fn set_input_mode(&mut self, mode: InputMode) -> (bool, Option<String>) {
        let commit = self.finish_direct();
        if let Some(commit) = &commit {
            self.input_state.context.push_str(commit);
        }
        self.input_state.mode = mode;
        (true, commit)
    }

    pub(super) fn finish_input(&mut self) -> Result<Option<String>> {
        match self.input_state.phase.clone() {
            Phase::Direct => Ok(self.finish_direct()),
            Phase::Preedit { stem, okuri } => Ok(Some(composition_output(
                &self.rom_kana_rule,
                self.input_state.mode,
                &stem,
                okuri.as_ref(),
            ))),
            Phase::Abbrev { input } => Ok(Some(input)),
            Phase::Selecting(selection) => {
                let candidate = selection.candidates.get(selection.cursor).cloned();
                if selection.origin == SelectionOrigin::RegistrationGeneration {
                    return match candidate {
                        Some(candidate) => self
                            .confirm_generated_registration(&candidate, Some("commit"), false)
                            .map(|(_, output)| output),
                        None => Ok(None),
                    };
                }
                if let Some(candidate) = &candidate {
                    self.confirm_candidate(
                        &selection.input,
                        selection.okuri.as_deref(),
                        candidate,
                    )?;
                }
                Ok(candidate.map(|candidate| candidate.output))
            }
        }
    }
}
