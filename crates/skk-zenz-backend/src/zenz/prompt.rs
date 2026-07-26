use anyhow::{Context, Result};
use llama_cpp_2::model::{AddBos, LlamaModel};
use llama_cpp_2::token::LlamaToken;

use crate::kana_script::hiragana_to_katakana;

pub(super) const OUTPUT_TAG: char = '\u{EE01}';
const INPUT_TAG: char = '\u{EE00}';
const CONTEXT_TAG: char = '\u{EE02}';
const BOS_TOKEN: LlamaToken = LlamaToken(2);

pub(super) fn prompt_tokens(
    model: &LlamaModel,
    context: &str,
    input: &str,
) -> Result<Vec<LlamaToken>> {
    let context = suffix_chars(context, 40);
    let input = hiragana_to_katakana(input);
    let prompt = if context.is_empty() {
        format!("{INPUT_TAG}{input}")
    } else {
        format!("{CONTEXT_TAG}{context}{INPUT_TAG}{input}")
    };
    let mut tokens = model
        .str_to_token(&prompt, AddBos::Never)
        .with_context(|| format!("promptのtoken化に失敗しました: {prompt:?}"))?;
    tokens.insert(0, BOS_TOKEN);
    Ok(tokens)
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
}
