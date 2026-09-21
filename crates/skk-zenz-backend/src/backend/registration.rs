use super::helpers::*;
use super::*;

impl Backend {
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

    pub(super) fn start_registration_generation(&mut self) -> Result<(bool, Option<String>)> {
        let Some(registration) = self.input_state.registrations.last().cloned() else {
            return Ok((false, None));
        };
        let model_input = format!(
            "{}{}",
            registration.input,
            registration.okuri.as_deref().unwrap_or_default()
        );
        let candidate_count = self.fcitx5.candidate_window_threshold.max(1);
        let excluded_outputs = self
            .conversion
            .lookup(&registration.input, registration.okuri.as_deref())?
            .into_iter()
            .map(|candidate| candidate.output)
            .collect();
        let mut result = self.start_generation(
            &registration.context_before,
            &model_input,
            &registration.text,
            crate::zenz::GenerationOptions::default(),
            candidate_count,
        )?;
        let mut candidates =
            generated_registration_candidates(&registration, result.candidates, &excluded_outputs);
        while candidates.len() < candidate_count && !result.finished {
            result = self.continue_generation(candidate_count - candidates.len())?;
            candidates.extend(generated_registration_candidates(
                &registration,
                result.candidates,
                &excluded_outputs,
            ));
        }
        self.active_generation =
            (!result.finished).then_some(ActiveGeneration { excluded_outputs });
        if candidates.is_empty() {
            self.cancel_generation();
            self.input_state.phase = Phase::Direct;
        } else {
            self.input_state.phase = Phase::Selecting(CandidateSelection {
                input: registration.input,
                okuri: registration.okuri,
                candidates,
                cursor: 0,
                origin: SelectionOrigin::RegistrationGeneration,
            });
        }
        Ok((true, None))
    }

    pub(super) fn handle_registration_generation_selection(
        &mut self,
        event: KeyEvent,
        command: Option<&Command>,
        mut selection: CandidateSelection,
    ) -> Result<(bool, Option<String>)> {
        match command {
            Some(Command::Abort) => {
                self.cancel_generation();
                self.input_state.phase = Phase::Direct;
                return Ok((true, None));
            }
            Some(Command::Delete) => {
                self.cancel_generation();
                self.input_state.phase = Phase::Direct;
                return Ok(self.delete_registration_text());
            }
            Some(Command::NextCandidate | Command::Complete) => {
                let mut next = next_candidate_cursor(
                    selection.cursor,
                    selection.candidates.len(),
                    self.fcitx5.candidate_window_threshold,
                    self.fcitx5.page_size,
                );
                if next.is_none() && self.active_generation.is_some() {
                    let registration = self
                        .input_state
                        .registrations
                        .last()
                        .cloned()
                        .expect("generation selection requires registration");
                    let excluded_outputs = self
                        .active_generation
                        .as_ref()
                        .expect("generation pending was checked above")
                        .excluded_outputs
                        .clone();
                    let mut generated = Vec::new();
                    let mut finished = false;
                    while generated.len() < self.fcitx5.page_size && !finished {
                        let result =
                            self.continue_generation(self.fcitx5.page_size - generated.len())?;
                        finished = result.finished;
                        generated.extend(generated_registration_candidates(
                            &registration,
                            result.candidates,
                            &excluded_outputs,
                        ));
                    }
                    if finished {
                        self.active_generation = None;
                    }
                    selection.candidates.extend(generated);
                    next = next_candidate_cursor(
                        selection.cursor,
                        selection.candidates.len(),
                        self.fcitx5.candidate_window_threshold,
                        self.fcitx5.page_size,
                    );
                }
                if let Some(next) = next {
                    selection.cursor = next;
                }
            }
            Some(Command::PreviousCandidate | Command::Previous) => {
                if selection.cursor == 0 {
                    self.cancel_generation();
                    self.input_state.phase = Phase::Direct;
                    return Ok((true, None));
                }
                selection.cursor = previous_candidate_cursor(
                    selection.cursor,
                    self.fcitx5.candidate_window_threshold,
                    self.fcitx5.page_size,
                )
                .expect("cursor 0 is handled above");
            }
            Some(Command::Commit | Command::CommitUnhandled) => {
                if let Some(candidate) = selection.candidates.get(selection.cursor).cloned() {
                    return self.confirm_generated_registration(&candidate, command, true);
                }
            }
            _ if should_forward_after_selection(&event, command) => {
                if let Some(candidate) = selection.candidates.get(selection.cursor).cloned() {
                    let (_, generated) = self.confirm_generated_registration(
                        &candidate,
                        Some(&Command::Commit),
                        true,
                    )?;
                    let (consumed, following) = self.handle_direct(event, command)?;
                    let mut commit = generated.unwrap_or_default();
                    if let Some(following) = following {
                        commit.push_str(&following);
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

    pub(super) fn confirm_generated_registration(
        &mut self,
        candidate: &Candidate,
        command: Option<&Command>,
        append_context: bool,
    ) -> Result<(bool, Option<String>)> {
        let Some(registration) = self.input_state.registrations.pop() else {
            return Ok((false, None));
        };
        self.register(
            &registration.input,
            registration.okuri.as_deref(),
            &candidate.text,
            None,
        )?;
        self.cancel_generation();
        self.input_state.mode = registration.mode;
        self.input_state.context = registration.context_before;
        if append_context {
            self.input_state.context.push_str(&candidate.output);
        }
        self.input_state.phase = Phase::Direct;
        let nested = !self.input_state.registrations.is_empty();
        Ok((
            nested || command == Some(&Command::Commit),
            Some(candidate.output.clone()),
        ))
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
        command: Option<&Command>,
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
        Ok((
            nested || command == Some(&Command::Commit),
            Some(registration.text),
        ))
    }
}

pub(super) fn generated_registration_candidates(
    registration: &Registration,
    generated: Vec<crate::zenz::GeneratedCandidate>,
    excluded_outputs: &std::collections::HashSet<String>,
) -> Vec<Candidate> {
    generated
        .into_iter()
        .filter(|generated| !excluded_outputs.contains(&generated.text))
        .filter_map(|generated| {
            let text = match registration.okuri.as_deref() {
                Some(okuri) => generated.text.strip_suffix(okuri)?.to_owned(),
                None => generated.text.clone(),
            };
            (!text.is_empty()).then(|| Candidate {
                input: registration.input.clone(),
                okuri: registration.okuri.clone(),
                text,
                output: generated.text,
                annotation: Some("AI".to_owned()),
                source: crate::candidate::CandidateSource::System,
                loss: generated.loss,
                token_count: generated.token_count,
                recency: None,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::zenz::GeneratedCandidate;

    #[test]
    fn generated_okuri_candidate_keeps_output_and_strips_dictionary_text() {
        let registration = Registration {
            input: "み".to_owned(),
            okuri: Some("る".to_owned()),
            source_phase: Phase::Direct,
            empty_return_phase: None,
            text: String::new(),
            mode: InputMode::Hiragana,
            context_before: String::new(),
        };
        let candidates = generated_registration_candidates(
            &registration,
            vec![
                GeneratedCandidate {
                    text: "見る".to_owned(),
                    loss: 1.0,
                    token_count: 2,
                },
                GeneratedCandidate {
                    text: "見た".to_owned(),
                    loss: 2.0,
                    token_count: 2,
                },
            ],
            &std::collections::HashSet::new(),
        );
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].text, "見");
        assert_eq!(candidates[0].output, "見る");
    }

    #[test]
    fn generated_candidates_already_in_dictionary_are_excluded() {
        let registration = Registration {
            input: "へんかん".to_owned(),
            okuri: None,
            source_phase: Phase::Direct,
            empty_return_phase: None,
            text: String::new(),
            mode: InputMode::Hiragana,
            context_before: String::new(),
        };
        let excluded = std::collections::HashSet::from(["変換".to_owned()]);
        let candidates = generated_registration_candidates(
            &registration,
            vec![
                GeneratedCandidate {
                    text: "変換".to_owned(),
                    loss: 1.0,
                    token_count: 2,
                },
                GeneratedCandidate {
                    text: "返還".to_owned(),
                    loss: 2.0,
                    token_count: 2,
                },
            ],
            &excluded,
        );
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].output, "返還");
    }
}
