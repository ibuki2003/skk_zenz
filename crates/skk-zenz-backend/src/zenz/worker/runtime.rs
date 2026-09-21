use std::ops::ControlFlow;
use std::sync::mpsc::TryRecvError;

use super::*;
use crate::zenz::GenerationSession;

pub(super) fn run_worker(
    scorer: ZenzScorer,
    commands: Receiver<WorkerCommand>,
    updates: Sender<RerankUpdate>,
    reload_updates: Sender<Result<(), String>>,
    model_events: ModelDownloadEventHandler,
) {
    WorkerRuntime {
        scorer,
        commands,
        updates,
        reload_updates,
        model_events,
        pending: None,
        generation_session: None,
    }
    .run();
}

struct WorkerRuntime {
    scorer: ZenzScorer,
    commands: Receiver<WorkerCommand>,
    updates: Sender<RerankUpdate>,
    reload_updates: Sender<Result<(), String>>,
    model_events: ModelDownloadEventHandler,
    pending: Option<WorkerCommand>,
    generation_session: Option<GenerationSession>,
}

impl WorkerRuntime {
    fn run(&mut self) {
        loop {
            let command = match self.next_command() {
                Some(command) => command,
                None => return,
            };
            if self.handle_command(command).is_break() {
                return;
            }
        }
    }

    fn next_command(&mut self) -> Option<WorkerCommand> {
        self.pending.take().or_else(|| self.commands.recv().ok())
    }

    fn handle_command(&mut self, command: WorkerCommand) -> ControlFlow<()> {
        match command {
            WorkerCommand::Clear => {
                self.generation_session = None;
                self.scorer.clear();
            }
            WorkerCommand::Prepare {
                context,
                input,
                response,
            } => self.prepare(context, input, response),
            WorkerCommand::PrepareSpeculatively { context, input } => {
                self.prepare_speculatively(context, input)?;
            }
            WorkerCommand::StartGeneration {
                context,
                input,
                output_prefix,
                options,
                candidate_count,
                response,
            } => self.start_generation(
                context,
                input,
                output_prefix,
                options,
                candidate_count,
                response,
            ),
            WorkerCommand::ContinueGeneration {
                candidate_count,
                response,
            } => self.continue_generation(candidate_count, response),
            WorkerCommand::CancelGeneration => self.generation_session = None,
            WorkerCommand::Reload { config } => self.reload(config),
            WorkerCommand::Rerank {
                generation,
                context,
                input,
                initial,
                remaining,
                response,
            } => self.rerank(generation, context, input, initial, remaining, response)?,
            WorkerCommand::Shutdown => return ControlFlow::Break(()),
        }
        ControlFlow::Continue(())
    }

    fn prepare(
        &mut self,
        context: String,
        input: String,
        response: SyncSender<Result<Score, String>>,
    ) {
        self.generation_session = None;
        let result = self
            .scorer
            .prepare(&context, &input)
            .map_err(|error| format!("{error:#}"));
        let _ = response.send(result);
    }

    fn prepare_speculatively(&mut self, mut context: String, mut input: String) -> ControlFlow<()> {
        self.generation_session = None;
        let mut superseded = false;
        loop {
            match self.commands.try_recv() {
                Ok(WorkerCommand::PrepareSpeculatively {
                    context: newer_context,
                    input: newer_input,
                }) => {
                    context = newer_context;
                    input = newer_input;
                }
                Ok(command) => {
                    self.pending = Some(command);
                    superseded = true;
                    break;
                }
                Err(TryRecvError::Disconnected) => return ControlFlow::Break(()),
                Err(TryRecvError::Empty) => break,
            }
        }
        if !superseded && let Err(_error) = self.scorer.prepare(&context, &input) {
            #[cfg(debug_assertions)]
            eprintln!("speculative prepare failed: {_error:#}");
        }
        ControlFlow::Continue(())
    }

    fn start_generation(
        &mut self,
        context: String,
        input: String,
        output_prefix: String,
        options: GenerationOptions,
        candidate_count: usize,
        response: SyncSender<Result<GenerationResult, String>>,
    ) {
        let result = self
            .scorer
            .start_generation(&context, &input, &output_prefix, options)
            .and_then(|mut session| {
                let result = self
                    .scorer
                    .continue_generation(&mut session, candidate_count)?;
                self.generation_session = (!result.finished).then_some(session);
                Ok(result)
            })
            .map_err(|error| format!("{error:#}"));
        if result.is_err() {
            self.generation_session = None;
        }
        let _ = response.send(result);
    }

    fn continue_generation(
        &mut self,
        candidate_count: usize,
        response: SyncSender<Result<GenerationResult, String>>,
    ) {
        let result = self
            .generation_session
            .as_mut()
            .context("続行できる候補生成がありません")
            .and_then(|session| self.scorer.continue_generation(session, candidate_count))
            .map_err(|error| format!("{error:#}"));
        if result.as_ref().is_ok_and(|result| result.finished) || result.is_err() {
            self.generation_session = None;
        }
        let _ = response.send(result);
    }

    fn reload(&mut self, config: ZenzConfig) {
        self.generation_session = None;
        let result = resolve_model(config, &self.model_events)
            .and_then(|config| self.scorer.reload(&config))
            .map_err(|error| format!("{error:#}"));
        let _ = self.reload_updates.send(result);
    }

    fn rerank(
        &mut self,
        generation: u64,
        context: String,
        input: String,
        initial: Vec<Candidate>,
        remaining: Vec<Candidate>,
        response: SyncSender<Result<Vec<Candidate>, String>>,
    ) -> ControlFlow<()> {
        self.generation_session = None;
        let initial = score_candidates(&mut self.scorer, &context, &input, initial);
        let initial_succeeded = initial.is_ok();
        if response
            .send(initial.map_err(|error| format!("{error:#}")))
            .is_err()
            || !initial_succeeded
        {
            return ControlFlow::Continue(());
        }
        for candidate in remaining {
            match self.commands.try_recv() {
                Ok(command) => {
                    self.pending = Some(command);
                    break;
                }
                Err(TryRecvError::Disconnected) => return ControlFlow::Break(()),
                Err(TryRecvError::Empty) => {}
            }
            match score_candidate(&mut self.scorer, &context, &input, candidate) {
                Ok(candidate) => {
                    if self
                        .updates
                        .send(RerankUpdate::Candidate {
                            generation,
                            candidate,
                        })
                        .is_err()
                    {
                        return ControlFlow::Break(());
                    }
                }
                Err(error) => {
                    let _ = self.updates.send(RerankUpdate::Failed {
                        generation,
                        error: format!("{error:#}"),
                    });
                    break;
                }
            }
        }
        let _ = self.updates.send(RerankUpdate::Finished { generation });
        ControlFlow::Continue(())
    }
}

fn score_candidates(
    scorer: &mut ZenzScorer,
    context: &str,
    input: &str,
    candidates: Vec<Candidate>,
) -> anyhow::Result<Vec<Candidate>> {
    candidates
        .into_iter()
        .map(|candidate| score_candidate(scorer, context, input, candidate))
        .collect()
}

fn score_candidate(
    scorer: &mut ZenzScorer,
    context: &str,
    input: &str,
    mut candidate: Candidate,
) -> anyhow::Result<Candidate> {
    let score = scorer.score(context, input, &candidate.output)?;
    candidate.loss = score.loss;
    candidate.token_count = score.token_count;
    Ok(candidate)
}
