use super::helpers::merge_reranked_candidate;
use super::*;

impl Backend {
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
                self.cancel_generation();
                self.input_state.reset();
                (true, None)
            }
            InputEvent::SelectCandidate(index) => self.select_candidate_at(index)?,
            InputEvent::FocusOut { commit } => {
                let output = if commit { self.finish_input()? } else { None };
                self.cancel_generation();
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
            self.scorer
                .prepare_speculatively(&self.input_state.context, &input);
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
                    let Phase::Selecting(selection) = &mut self.input_state.phase else {
                        continue;
                    };
                    merge_reranked_candidate(
                        &self.conversion,
                        &mut selection.candidates,
                        &mut selection.cursor,
                        candidate,
                        active.order,
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
                RerankUpdate::Failed {
                    generation,
                    error: _error,
                } => {
                    if self
                        .active_rerank
                        .as_ref()
                        .is_some_and(|active| active.generation == generation)
                    {
                        #[cfg(debug_assertions)]
                        eprintln!("skk-zenz: asynchronous rerank failed: {_error}");
                        self.active_rerank = None;
                        changed = true;
                    }
                }
            }
        }
        changed.then(|| self.input_update(false, None))
    }

    pub fn rerank_pending(&self) -> bool {
        self.active_rerank.is_some() && matches!(self.input_state.phase, Phase::Selecting(_))
    }

    pub(super) fn input_update(&self, consumed: bool, commit: Option<String>) -> InputUpdate {
        let (candidate_window_visible, candidate_page_start) = match &self.input_state.phase {
            Phase::Selecting(selection)
                if selection.cursor >= self.fcitx5.candidate_window_threshold =>
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
}
