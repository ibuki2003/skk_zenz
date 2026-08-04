use anyhow::{Context, Result};
use llama_cpp_2::model::{AddBos, LlamaModel};
use llama_cpp_2::token::LlamaToken;

use crate::kana_script::hiragana_to_katakana;

pub(super) const OUTPUT_TAG: char = '\u{EE01}';
const INPUT_TAG: char = '\u{EE00}';
const CONTEXT_TAG: char = '\u{EE02}';

pub(super) fn prompt_tokens(
    model: &LlamaModel,
    context: &str,
    input: &str,
) -> Result<Vec<LlamaToken>> {
    let context = suffix_chars(context, 40);
    let input = zenz_input(input);
    let prompt = if context.is_empty() {
        format!("{INPUT_TAG}{input}")
    } else {
        format!("{CONTEXT_TAG}{context}{INPUT_TAG}{input}")
    };
    model
        .str_to_token(&prompt, AddBos::Always)
        .with_context(|| format!("promptのtoken化に失敗しました: {prompt:?}"))
}

fn zenz_input(input: &str) -> String {
    hiragana_to_katakana(&input.replace('>', ""))
}

fn suffix_chars(value: &str, max_chars: usize) -> String {
    let skip = value.chars().count().saturating_sub(max_chars);
    value.chars().skip(skip).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_suffix_is_character_based() {
        assert_eq!(suffix_chars("あいうえお", 3), "うえお");
    }

    #[test]
    fn converts_hiragana_only() {
        assert_eq!(hiragana_to_katakana("かなABC"), "カナABC");
    }

    #[test]
    fn removes_prefix_and_suffix_markers() {
        assert_eq!(zenz_input(">せつび>"), "セツビ");
    }
}
