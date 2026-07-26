use super::helpers::*;
use super::*;

impl Backend {
    pub(super) fn handle_key(&mut self, event: KeyEvent) -> Result<(bool, Option<String>)> {
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

    pub(super) fn select_candidate_at(&mut self, index: usize) -> Result<(bool, Option<String>)> {
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
    pub(super) fn handle_selecting(
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

    pub(super) fn begin_registration(
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

    pub(super) fn abort_registration(&mut self) -> (bool, Option<String>) {
        let Some(registration) = self.input_state.registrations.pop() else {
            return (false, None);
        };
        self.input_state.mode = registration.mode;
        self.input_state.context = registration.context_before;
        self.input_state.phase = registration.source_phase;
        self.input_state.direct_pending.clear();
        (true, None)
    }

    pub(super) fn delete_registration_text(&mut self) -> (bool, Option<String>) {
        let Some(registration) = self.input_state.registrations.last_mut() else {
            return (false, None);
        };
        if registration.text.pop().is_some() {
            self.input_state.context.pop();
        }
        (true, None)
    }

    pub(super) fn finish_registration(
        &mut self,
        command: Option<&str>,
    ) -> Result<(bool, Option<String>)> {
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
    fn pending_romaji_takes_priority_over_abbrev() {
        assert!(should_start_abbrev(Some("abbrev"), InputMode::Hiragana, ""));
        assert!(!should_start_abbrev(
            Some("abbrev"),
            InputMode::Hiragana,
            "z"
        ));
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
