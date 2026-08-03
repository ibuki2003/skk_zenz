use anyhow::Result;
use std::collections::HashMap;

use crate::candidate::Candidate;
use crate::config::{BackendConfig, CandidateOrder, Fcitx5Config};
use crate::config_watcher::ConfigWatcher;
use crate::conversion::ConversionCore;
use crate::conversion_history;
use crate::dictionary::SqliteUserDictionary;
use crate::input::{
    Composition, InputEvent, InputMode, InputState, InputUpdate, Phase, Registration,
    SelectionOrigin,
};
use crate::keymap::{KeyEvent, Keymap, Modifiers};
use crate::rom_kana::{KanaConversion, KanaMode, RomKanaRule, convert_kana_script};
use crate::zenz::{ModelDownloadEventHandler, RerankUpdate, Score, ZenzWorker};

#[derive(Clone, Debug)]
struct ActiveRerank {
    generation: u64,
    order: CandidateOrder,
    selection_touched: bool,
    evaluated: usize,
    total: usize,
}

pub struct Backend {
    scorer: ZenzWorker,
    conversion: ConversionCore<SqliteUserDictionary>,
    rom_kana_rule: RomKanaRule,
    keymaps: HashMap<InputMode, Keymap>,
    input_state: InputState,
    active_rerank: Option<ActiveRerank>,
    fcitx5: Fcitx5Config,
    config_path: std::path::PathBuf,
    conversion_history_path: std::path::PathBuf,
    conversion_history_enabled: bool,
    config_watcher: ConfigWatcher,
    zenz_config: crate::zenz::ZenzConfig,
    pending_zenz_configs: std::collections::VecDeque<crate::zenz::ZenzConfig>,
    model_reload_pending: bool,
}

mod candidates;
mod events;
mod helpers;
mod lifecycle;
mod state_machine;
