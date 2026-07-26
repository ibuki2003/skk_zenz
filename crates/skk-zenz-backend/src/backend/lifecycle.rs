use super::*;
use anyhow::Context;

impl Backend {
    pub fn load(config: BackendConfig) -> Result<Self> {
        Self::load_with_model_events(config, std::sync::Arc::new(|_| {}))
    }

    pub fn load_with_model_events(
        config: BackendConfig,
        model_events: ModelDownloadEventHandler,
    ) -> Result<Self> {
        let config_watcher = ConfigWatcher::new(&config.config_path)?;
        let conversion_history_path = config
            .config_path
            .parent()
            .context("設定ファイルの親ディレクトリがありません")?
            .join("conversion_history.csv");
        let zenz_config = config.zenz.clone();
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
            scorer: ZenzWorker::load_with_model_events(config.zenz, model_events)?,
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
            config_path: config.config_path,
            conversion_history_path,
            config_watcher,
            zenz_config,
            pending_zenz_configs: std::collections::VecDeque::new(),
            model_reload_pending: false,
        })
    }

    pub fn config_watch_fd(&self) -> i32 {
        self.config_watcher.fd()
    }

    pub fn reload_config_if_changed(&mut self) -> Option<Result<()>> {
        match self.config_watcher.take_changed() {
            Ok(false) => return None,
            Err(error) => return Some(Err(error)),
            Ok(true) => {}
        }
        Some(self.reload_config())
    }

    pub fn model_reload_pending(&self) -> bool {
        self.model_reload_pending
    }

    pub fn poll_model_reload(&mut self) -> Option<Result<()>> {
        let update = self.scorer.try_reload_update()?;
        let config = self.pending_zenz_configs.pop_front();
        if update.is_ok()
            && let Some(config) = config
        {
            self.zenz_config = config;
        }
        self.model_reload_pending = !self.pending_zenz_configs.is_empty();
        Some(update.map_err(anyhow::Error::msg))
    }

    pub(super) fn reload_config(&mut self) -> Result<()> {
        let config = BackendConfig::from_toml_file(&self.config_path)?;
        let current_or_pending = self
            .pending_zenz_configs
            .back()
            .unwrap_or(&self.zenz_config);
        let reload_model = config.zenz != *current_or_pending;
        let rom_kana_rule = RomKanaRule::load(&config.rom_kana_rule, &config.rule_search_paths)?;
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
        let system_dictionaries = open_system_dictionaries(&config.system_dictionaries)?;

        self.rom_kana_rule = rom_kana_rule;
        self.keymaps = keymaps;
        self.system_dictionaries = system_dictionaries;
        self.user_dictionary
            .set_half_life_days(config.recency_half_life_days);
        self.candidate_order = config.candidate_order;
        self.fcitx5 = config.fcitx5;
        self.active_rerank = None;
        if reload_model {
            self.scorer.reload(config.zenz.clone())?;
            self.pending_zenz_configs.push_back(config.zenz);
            self.model_reload_pending = true;
        }
        Ok(())
    }

    pub fn clear_cache(&mut self) {
        self.scorer.clear();
        self.active_rerank = None;
    }

    pub fn prepare(&mut self, context: &str, input: &str) -> Result<Score> {
        #[cfg(debug_assertions)]
        eprintln!("[BACKEND] prepare context={context:?} input={input:?}");
        let result = self.scorer.prepare(context, input);
        #[cfg(debug_assertions)]
        eprintln!("[BACKEND] prepare -> {result:?}");
        result
    }

    pub fn convert_romaji(&self, input: &str, mode: KanaMode) -> KanaConversion {
        self.rom_kana_rule.convert(input, mode)
    }
}
