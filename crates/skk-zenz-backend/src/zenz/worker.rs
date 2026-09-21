use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender, SyncSender};
use std::thread::{self, JoinHandle};

use anyhow::{Context, Result, anyhow};

use crate::candidate::Candidate;

use super::model_resolver::resolve_model;
use super::{GenerationOptions, GenerationResult, Score, ZenzConfig, ZenzScorer};

mod runtime;

use runtime::run_worker;

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
    StartGeneration {
        context: String,
        input: String,
        output_prefix: String,
        options: GenerationOptions,
        candidate_count: usize,
        response: SyncSender<Result<GenerationResult, String>>,
    },
    ContinueGeneration {
        candidate_count: usize,
        response: SyncSender<Result<GenerationResult, String>>,
    },
    CancelGeneration,
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

    pub(crate) fn start_generation(
        &self,
        context: &str,
        input: &str,
        output_prefix: &str,
        options: GenerationOptions,
        candidate_count: usize,
    ) -> Result<GenerationResult> {
        let (sender, receiver) = mpsc::sync_channel(0);
        self.commands
            .send(WorkerCommand::StartGeneration {
                context: context.to_owned(),
                input: input.to_owned(),
                output_prefix: output_prefix.to_owned(),
                options,
                candidate_count,
                response: sender,
            })
            .context("AI評価workerへ生成要求を送信できません")?;
        receive_response(receiver, "候補生成開始")
    }

    pub(crate) fn continue_generation(&self, candidate_count: usize) -> Result<GenerationResult> {
        let (sender, receiver) = mpsc::sync_channel(0);
        self.commands
            .send(WorkerCommand::ContinueGeneration {
                candidate_count,
                response: sender,
            })
            .context("AI評価workerへ生成続行要求を送信できません")?;
        receive_response(receiver, "候補生成続行")
    }

    pub(crate) fn cancel_generation(&self) {
        let _ = self.commands.send(WorkerCommand::CancelGeneration);
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
