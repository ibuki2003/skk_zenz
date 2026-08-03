use crate::rom_kana::{KanaMode, RomKanaRule};

#[derive(Clone, Debug, Default)]
pub(crate) struct Composition {
    pub(super) output: String,
    pending: String,
}

impl Composition {
    pub(crate) fn from_output(output: String) -> Self {
        Self {
            output,
            pending: String::new(),
        }
    }

    pub(crate) fn push(&mut self, rule: &RomKanaRule, text: &str, mode: KanaMode) {
        let input = format!("{}{text}", self.pending);
        let conversion = rule.convert_partial(&input, mode);
        self.output.push_str(&conversion.output);
        self.pending = conversion.preedit;
    }

    pub(crate) fn pop(&mut self) -> bool {
        if self.pending.pop().is_some() {
            return true;
        }
        self.output.pop().is_some()
    }

    pub(crate) fn finish(&self, rule: &RomKanaRule, mode: KanaMode) -> String {
        let mut value = self.output.clone();
        let conversion = rule.convert(&self.pending, mode);
        value.push_str(&conversion.output);
        value
    }

    pub(crate) fn display(&self) -> String {
        format!("{}{}", self.output, self.pending)
    }

    pub(crate) fn has_output(&self) -> bool {
        !self.output.is_empty()
    }

    pub(crate) fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }
}
