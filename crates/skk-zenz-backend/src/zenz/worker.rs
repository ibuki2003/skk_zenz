use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender, SyncSender, TryRecvError};
use std::thread::{self, JoinHandle};

use anyhow::{Context, Result, anyhow};

use crate::candidate::Candidate;

use super::model_resolver::resolve_model;
use super::{Score, ZenzConfig, ZenzScorer};

pub(crate) const INITIAL_RERANK_COUNT: usize = 3;

#[derive(Clone, Debug)]
pub enum ModelDownloadEvent {
    Started { reference: String },
    Completed { reference: String },
    Failed { reference: String, error: String },
}

pub type ModelDownloadEventHandler = Arc<dyn Fn(ModelDownloadEvent) + Send + Sync>;

pub(crate) enum RerankUpdate {
    Candidate {
        generation: u64,
        candidate: Candidate,
    },
    Finished {
        generation: u64,
    },
    Failed {
        generation: u64,
        error: String,
    },
}

enum WorkerCommand {
    Clear,
    Prepare {
        context: String,
        input: String,
        response: SyncSender<Result<Score, String>>,
    },
    PrepareSpeculatively {
        context: String,
        input: String,
    },
    Reload {
        config: ZenzConfig,
    },
    Rerank {
        generation: u64,
        context: String,
        input: String,
        initial: Vec<Candidate>,
        remaining: Vec<Candidate>,
        response: SyncSender<Result<Vec<Candidate>, String>>,
    },
    Shutdown,
}

pub(crate) struct ZenzWorker {
    commands: Sender<WorkerCommand>,
    updates: Receiver<RerankUpdate>,
    reload_updates: Receiver<Result<(), String>>,
    thread: Option<JoinHandle<()>>,
    next_generation: u64,
}

impl ZenzWorker {
    pub(crate) fn load_with_model_events(
        config: ZenzConfig,
        model_events: ModelDownloadEventHandler,
    ) -> Result<Self> {
        let (commands, command_receiver) = mpsc::channel();
        let (updates, update_receiver) = mpsc::channel();
        let (reload_updates, reload_update_receiver) = mpsc::channel();
        let (ready_sender, ready_receiver) = mpsc::sync_channel(0);
        let thread = thread::Builder::new()
            .name("skk-zenz-scorer".to_owned())
            .spawn(move || {
                let scorer = resolve_model(config, &model_events)
                    .and_then(|config| ZenzScorer::load(&config));
                match scorer {
                    Ok(scorer) => {
                        let _ = ready_sender.send(Ok(()));
                        run_worker(
                            scorer,
                            command_receiver,
                            updates,
                            reload_updates,
                            model_events,
                        );
                    }
                    Err(error) => {
                        let _ = ready_sender.send(Err(format!("{error:#}")));
                    }
                }
            })
            .context("AI評価workerを起動できません")?;
        match ready_receiver
            .recv()
            .context("AI評価workerの初期化結果を受信できません")?
        {
            Ok(()) => Ok(Self {
                commands,
                updates: update_receiver,
                reload_updates: reload_update_receiver,
                thread: Some(thread),
                next_generation: 1,
            }),
            Err(error) => {
                let _ = thread.join();
                Err(anyhow!(error))
            }
        }
    }

    pub(crate) fn clear(&self) {
        let _ = self.commands.send(WorkerCommand::Clear);
    }

    pub(crate) fn prepare(&self, context: &str, input: &str) -> Result<Score> {
        let (sender, receiver) = mpsc::sync_channel(0);
        self.commands
            .send(WorkerCommand::Prepare {
                context: context.to_owned(),
                input: input.to_owned(),
                response: sender,
            })
            .context("AI評価workerへprepare要求を送信できません")?;
        receive_response(receiver, "prepare")
    }

    pub(crate) fn prepare_speculatively(&self, context: &str, input: &str) {
        let _ = self.commands.send(WorkerCommand::PrepareSpeculatively {
            context: context.to_owned(),
            input: input.to_owned(),
        });
    }

    pub(crate) fn reload(&self, config: ZenzConfig) -> Result<()> {
        self.commands
            .send(WorkerCommand::Reload { config })
            .context("AI評価workerへモデル再読み込み要求を送信できません")
    }

    pub(crate) fn try_reload_update(&self) -> Option<Result<(), String>> {
        self.reload_updates.try_recv().ok()
    }

    pub(crate) fn rerank(
        &mut self,
        context: &str,
        input: &str,
        mut candidates: Vec<Candidate>,
    ) -> Result<(u64, Vec<Candidate>)> {
        let generation = self.next_generation;
        self.next_generation = self.next_generation.wrapping_add(1).max(1);
        let remaining = if candidates.len() > INITIAL_RERANK_COUNT {
            candidates.split_off(INITIAL_RERANK_COUNT)
        } else {
            Vec::new()
        };
        let (sender, receiver) = mpsc::sync_channel(0);
        self.commands
            .send(WorkerCommand::Rerank {
                generation,
                context: context.to_owned(),
                input: input.to_owned(),
                initial: candidates,
                remaining,
                response: sender,
            })
            .context("AI評価workerへrerank要求を送信できません")?;
        let initial = receive_response(receiver, "先頭候補のrerank")?;
        Ok((generation, initial))
    }

    pub(crate) fn try_update(&self) -> Option<RerankUpdate> {
        self.updates.try_recv().ok()
    }
}

impl Drop for ZenzWorker {
    fn drop(&mut self) {
        let _ = self.commands.send(WorkerCommand::Shutdown);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn receive_response<T>(receiver: Receiver<Result<T, String>>, operation: &str) -> Result<T> {
    match receiver
        .recv()
        .with_context(|| format!("AI評価workerから{operation}結果を受信できません"))?
    {
        Ok(value) => Ok(value),
        Err(error) => Err(anyhow!(error)),
    }
}

fn run_worker(
    mut scorer: ZenzScorer,
    commands: Receiver<WorkerCommand>,
    updates: Sender<RerankUpdate>,
    reload_updates: Sender<Result<(), String>>,
    model_events: ModelDownloadEventHandler,
) {
    let mut pending = None;
    loop {
        let command = match pending.take() {
            Some(command) => command,
            None => match commands.recv() {
                Ok(command) => command,
                Err(_) => return,
            },
        };
        match command {
            WorkerCommand::Clear => scorer.clear(),
            WorkerCommand::Prepare {
                context,
                input,
                response,
            } => {
                let result = scorer
                    .prepare(&context, &input)
                    .map_err(|error| format!("{error:#}"));
                let _ = response.send(result);
            }
            WorkerCommand::PrepareSpeculatively {
                mut context,
                mut input,
            } => {
                let mut superseded = false;
                loop {
                    match commands.try_recv() {
                        Ok(WorkerCommand::PrepareSpeculatively {
                            context: newer_context,
                            input: newer_input,
                        }) => {
                            context = newer_context;
                            input = newer_input;
                        }
                        Ok(command) => {
                            pending = Some(command);
                            superseded = true;
                            break;
                        }
                        Err(TryRecvError::Disconnected) => return,
                        Err(TryRecvError::Empty) => break,
                    }
                }
                if !superseded && let Err(error) = scorer.prepare(&context, &input) {
                    eprintln!("speculative prepare failed: {error:#}");
                }
            }
            WorkerCommand::Reload { config } => {
                let result = resolve_model(config, &model_events)
                    .and_then(|config| scorer.reload(&config))
                    .map_err(|error| format!("{error:#}"));
                let _ = reload_updates.send(result);
            }
            WorkerCommand::Rerank {
                generation,
                context,
                input,
                initial,
                remaining,
                response,
            } => {
                let initial = score_candidates(&mut scorer, &context, &input, initial);
                let initial_succeeded = initial.is_ok();
                if response
                    .send(initial.map_err(|error| format!("{error:#}")))
                    .is_err()
                    || !initial_succeeded
                {
                    continue;
                }
                for candidate in remaining {
                    match commands.try_recv() {
                        Ok(command) => {
                            pending = Some(command);
                            break;
                        }
                        Err(TryRecvError::Disconnected) => return,
                        Err(TryRecvError::Empty) => {}
                    }
                    match score_candidate(&mut scorer, &context, &input, candidate) {
                        Ok(candidate) => {
                            if updates
                                .send(RerankUpdate::Candidate {
                                    generation,
                                    candidate,
                                })
                                .is_err()
                            {
                                return;
                            }
                        }
                        Err(error) => {
                            let _ = updates.send(RerankUpdate::Failed {
                                generation,
                                error: format!("{error:#}"),
                            });
                            break;
                        }
                    }
                }
                let _ = updates.send(RerankUpdate::Finished { generation });
            }
            WorkerCommand::Shutdown => return,
        }
    }
}

fn score_candidates(
    scorer: &mut ZenzScorer,
    context: &str,
    input: &str,
    candidates: Vec<Candidate>,
) -> Result<Vec<Candidate>> {
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
) -> Result<Candidate> {
    let score = scorer.score(context, input, &candidate.output)?;
    candidate.loss = score.loss;
    candidate.token_count = score.token_count;
    Ok(candidate)
}
