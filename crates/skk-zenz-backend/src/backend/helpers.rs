use super::*;
use crate::dictionary::UserDictionary;

pub(super) fn is_preedit_start(event: &KeyEvent) -> bool {
    event.modifiers == Modifiers::SHIFT
        && event
            .text
            .as_deref()
            .is_some_and(|text| text.len() == 1 && text.as_bytes()[0].is_ascii_uppercase())
}

pub(super) fn upper_command_text(command: Option<&Command>) -> Option<&str> {
    match command {
        Some(Command::Upper(text)) => Some(text),
        _ => None,
    }
}

pub(super) fn command_for_rom_kana<'a>(
    command: Option<&'a Command>,
    rule: &RomKanaRule,
    mode: InputMode,
) -> Option<&'a Command> {
    match upper_command_text(command) {
        Some(text)
            if !matches!(
                mode,
                InputMode::Hiragana | InputMode::Katakana | InputMode::HankakuKatakana
            ) || !rule.can_consume(text) =>
        {
            None
        }
        _ => command,
    }
}

pub(super) fn input_text<'a>(event: &'a KeyEvent, command: Option<&'a Command>) -> Option<&'a str> {
    upper_command_text(command).or(event.text.as_deref())
}

pub(super) fn is_upper_input(event: &KeyEvent, command: Option<&Command>) -> bool {
    upper_command_text(command).is_some() || is_preedit_start(event)
}

pub(super) fn should_start_okuri(
    event: &KeyEvent,
    command: Option<&Command>,
    stem: &Composition,
) -> bool {
    is_upper_input(event, command) && stem.has_output() && !stem.has_pending()
}

pub(super) fn should_restore_preedit(command: Option<&Command>, cursor: usize) -> bool {
    command == Some(&Command::Abort)
        || matches!(
            command,
            Some(Command::PreviousCandidate | Command::Previous)
        ) && cursor == 0
}

pub(super) fn selection_source_phase(
    input: String,
    okuri: Option<String>,
    origin: SelectionOrigin,
) -> Phase {
    match origin {
        SelectionOrigin::Preedit => Phase::Preedit {
            stem: Composition::from_output(input),
            okuri: okuri.map(Composition::from_output),
        },
        SelectionOrigin::Abbrev => Phase::Abbrev { input },
        SelectionOrigin::RegistrationGeneration => Phase::Direct,
    }
}

pub(super) fn should_forward_after_selection(event: &KeyEvent, command: Option<&Command>) -> bool {
    input_text(event, command).is_some()
}

pub(super) fn supports_abbrev(mode: InputMode) -> bool {
    matches!(
        mode,
        InputMode::Hiragana | InputMode::Katakana | InputMode::HankakuKatakana
    )
}

pub(super) fn should_start_abbrev(
    command: Option<&Command>,
    mode: InputMode,
    pending: &str,
) -> bool {
    command == Some(&Command::Abbrev) && supports_abbrev(mode) && pending.is_empty()
}

pub(super) fn input_mode_for_command(command: &Command) -> Option<InputMode> {
    match command {
        Command::SetInputMode(mode) => Some(*mode),
        _ => None,
    }
}

pub(super) fn direct_mode_transition(
    command: Option<&Command>,
    pending: &str,
) -> Option<InputMode> {
    match command {
        Some(Command::SetInputMode(mode)) if pending.is_empty() => Some(*mode),
        _ => None,
    }
}

pub(super) fn next_candidate_cursor(
    cursor: usize,
    candidate_count: usize,
    window_threshold: usize,
    page_size: usize,
) -> Option<usize> {
    let next = if cursor < window_threshold {
        cursor.saturating_add(1)
    } else {
        let page = (cursor - window_threshold) / page_size;
        window_threshold.saturating_add(page.saturating_add(1).saturating_mul(page_size))
    };
    (next < candidate_count).then_some(next)
}

pub(super) fn previous_candidate_cursor(
    cursor: usize,
    window_threshold: usize,
    page_size: usize,
) -> Option<usize> {
    if cursor <= window_threshold {
        cursor.checked_sub(1)
    } else {
        Some(cursor.saturating_sub(page_size).max(window_threshold))
    }
}

pub(super) fn to_wide_latin(value: &str) -> String {
    value
        .chars()
        .map(|character| match character {
            ' ' => '\u{3000}',
            '!'..='~' => char::from_u32(character as u32 + 0xfee0)
                .expect("ASCII fullwidth mapping is valid Unicode"),
            _ => character,
        })
        .collect()
}

pub(super) fn composition_output(
    rule: &RomKanaRule,
    mode: InputMode,
    stem: &Composition,
    okuri: Option<&Composition>,
) -> String {
    let mut output = stem.finish(rule, mode.into());
    if let Some(okuri) = okuri {
        output.push_str(&okuri.finish(rule, mode.into()));
    }
    output
}

/// Inserts one asynchronously scored candidate while preserving IME selection state.
pub(super) fn merge_reranked_candidate<U: UserDictionary>(
    conversion: &ConversionCore<U>,
    candidates: &mut Vec<Candidate>,
    cursor: &mut usize,
    candidate: Candidate,
    order: CandidateOrder,
    preserve_selection: bool,
) {
    let selected = candidates
        .get(*cursor)
        .map(|candidate| candidate.output.clone());
    candidates.push(candidate);
    conversion.rerank_candidates(candidates, order);
    if !preserve_selection {
        *cursor = 0;
    } else if let Some(selected) = selected
        && let Some(index) = candidates
            .iter()
            .position(|candidate| candidate.output == selected)
    {
        *cursor = index;
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::conversion::ConversionCore;
    use crate::dictionary::MemoryUserDictionary;
    use crate::{CandidateOrderConfig, CandidateSource};

    fn rule() -> RomKanaRule {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("test-data/rules");
        RomKanaRule::load("base", &[root]).unwrap()
    }

    fn conversion_core() -> ConversionCore<MemoryUserDictionary> {
        ConversionCore::load(
            &[] as &[&Path],
            MemoryUserDictionary::new(1.0),
            CandidateOrderConfig::default(),
        )
        .unwrap()
    }

    #[test]
    fn shifted_vowel_completes_pending_stem_instead_of_starting_okuri() {
        let rule = rule();
        let event = KeyEvent::character("A", 'a', Modifiers::SHIFT);
        let mut stem = Composition::default();
        stem.push(&rule, "k", KanaMode::Hiragana);
        assert!(!should_start_okuri(&event, None, &stem));

        stem.push(&rule, "a", KanaMode::Hiragana);
        assert_eq!(stem.display(), "か");
        assert!(should_start_okuri(&event, None, &stem));

        stem.push(&rule, "k", KanaMode::Hiragana);
        assert_eq!(stem.display(), "かk");
        assert!(!should_start_okuri(&event, None, &stem));
    }

    #[test]
    fn upper_command_uses_lower_text_and_upper_semantics() {
        let event = KeyEvent::character(":", ':', Modifiers::SHIFT);
        assert_eq!(
            input_text(&event, Some(&Command::Upper(";".to_owned()))),
            Some(";")
        );
        assert!(is_upper_input(
            &event,
            Some(&Command::Upper(";".to_owned()))
        ));

        let mut stem = Composition::default();
        stem.push(&rule(), "ka", KanaMode::Hiragana);
        assert!(should_start_okuri(
            &event,
            Some(&Command::Upper(";".to_owned())),
            &stem
        ));
    }

    #[test]
    fn upper_command_requires_a_rom_kana_prefix() {
        let rule = rule();
        let event = KeyEvent::character(":", ':', Modifiers::SHIFT);
        let upper_semicolon = Command::Upper(";".to_owned());
        let upper_k = Command::Upper("k".to_owned());

        let invalid = command_for_rom_kana(Some(&upper_semicolon), &rule, InputMode::Hiragana);
        assert_eq!(invalid, None);
        assert_eq!(input_text(&event, invalid), Some(":"));
        assert!(!is_upper_input(&event, invalid));

        let valid = command_for_rom_kana(Some(&upper_k), &rule, InputMode::Hiragana);
        assert_eq!(valid, Some(&upper_k));
        assert_eq!(input_text(&event, valid), Some("k"));
        assert!(is_upper_input(&event, valid));

        let latin = command_for_rom_kana(Some(&upper_k), &rule, InputMode::Latin);
        assert_eq!(latin, None);
        assert_eq!(input_text(&event, latin), Some(":"));
        assert!(!is_upper_input(&event, latin));
    }

    #[test]
    fn first_previous_candidate_returns_to_preedit() {
        assert!(should_restore_preedit(Some(&Command::PreviousCandidate), 0));
        assert!(should_restore_preedit(Some(&Command::Previous), 0));
        assert!(!should_restore_preedit(
            Some(&Command::PreviousCandidate),
            2
        ));
        assert!(!should_restore_preedit(Some(&Command::Previous), 2));
        assert!(!should_restore_preedit(Some(&Command::Delete), 2));
        assert!(should_restore_preedit(Some(&Command::Abort), 2));
    }

    #[test]
    fn regular_input_is_forwarded_after_committing_selection() {
        assert!(should_forward_after_selection(
            &KeyEvent::character("n", 'n', Modifiers::empty()),
            None
        ));
        assert!(!should_forward_after_selection(
            &KeyEvent::named("Shift_L", Modifiers::SHIFT),
            None
        ));
        assert!(should_forward_after_selection(
            &KeyEvent::character(":", ':', Modifiers::SHIFT),
            Some(&Command::Upper(";".to_owned()))
        ));
        assert!(!should_forward_after_selection(
            &KeyEvent::named("BackSpace", Modifiers::empty()),
            Some(&Command::Delete)
        ));
    }

    #[test]
    fn converts_ascii_to_wide_latin() {
        assert_eq!(to_wide_latin("Az 09!?"), "Ａｚ　０９！？");
        assert_eq!(to_wide_latin("日本語"), "日本語");
    }

    #[test]
    fn maps_all_five_input_mode_commands() {
        assert_eq!(
            input_mode_for_command(&Command::SetInputMode(InputMode::Hiragana)),
            Some(InputMode::Hiragana)
        );
        assert_eq!(
            input_mode_for_command(&Command::SetInputMode(InputMode::Katakana)),
            Some(InputMode::Katakana)
        );
        assert_eq!(
            input_mode_for_command(&Command::SetInputMode(InputMode::HankakuKatakana)),
            Some(InputMode::HankakuKatakana)
        );
        assert_eq!(
            input_mode_for_command(&Command::SetInputMode(InputMode::Latin)),
            Some(InputMode::Latin)
        );
        assert_eq!(
            input_mode_for_command(&Command::SetInputMode(InputMode::WideLatin)),
            Some(InputMode::WideLatin)
        );
    }

    #[test]
    fn abbrev_is_available_only_from_kana_modes() {
        assert!(supports_abbrev(InputMode::Hiragana));
        assert!(supports_abbrev(InputMode::Katakana));
        assert!(supports_abbrev(InputMode::HankakuKatakana));
        assert!(!supports_abbrev(InputMode::Latin));
        assert!(!supports_abbrev(InputMode::WideLatin));
    }

    #[test]
    fn pending_romaji_takes_priority_over_abbrev() {
        assert!(should_start_abbrev(
            Some(&Command::Abbrev),
            InputMode::Hiragana,
            ""
        ));
        assert!(!should_start_abbrev(
            Some(&Command::Abbrev),
            InputMode::Hiragana,
            "z"
        ));
    }

    #[test]
    fn asynchronous_rerank_preserves_selected_candidate() {
        let candidate = |output: &str, loss| Candidate {
            input: String::new(),
            okuri: None,
            text: output.to_owned(),
            output: output.to_owned(),
            annotation: None,
            source: CandidateSource::System,
            loss,
            token_count: 1,
            recency: None,
        };
        let mut candidates = vec![candidate("second", 2.0), candidate("third", 3.0)];
        let mut cursor = 1;

        merge_reranked_candidate(
            &conversion_core(),
            &mut candidates,
            &mut cursor,
            candidate("first", 1.0),
            CandidateOrder::AiScore,
            true,
        );

        assert_eq!(
            candidates
                .iter()
                .map(|candidate| candidate.output.as_str())
                .collect::<Vec<_>>(),
            ["first", "second", "third"]
        );
        assert_eq!(candidates[cursor].output, "third");
    }

    #[test]
    fn asynchronous_rerank_selects_new_best_before_user_interaction() {
        let candidate = |output: &str, loss| Candidate {
            input: String::new(),
            okuri: None,
            text: output.to_owned(),
            output: output.to_owned(),
            annotation: None,
            source: CandidateSource::System,
            loss,
            token_count: 1,
            recency: None,
        };
        let mut candidates = vec![candidate("second", 2.0)];
        let mut cursor = 0;

        merge_reranked_candidate(
            &conversion_core(),
            &mut candidates,
            &mut cursor,
            candidate("first", 1.0),
            CandidateOrder::AiScore,
            false,
        );

        assert_eq!(candidates[cursor].output, "first");
    }

    #[test]
    fn does_not_change_mode_in_the_middle_of_romaji() {
        assert_eq!(
            direct_mode_transition(Some(&Command::SetInputMode(InputMode::Latin)), ""),
            Some(InputMode::Latin)
        );
        assert_eq!(
            direct_mode_transition(Some(&Command::SetInputMode(InputMode::Latin)), "k"),
            None
        );
    }

    #[test]
    fn candidate_cursor_advances_individually_then_by_page() {
        assert_eq!(next_candidate_cursor(0, 20, 3, 8), Some(1));
        assert_eq!(next_candidate_cursor(1, 20, 3, 8), Some(2));
        assert_eq!(next_candidate_cursor(2, 20, 3, 8), Some(3));
        assert_eq!(next_candidate_cursor(3, 20, 3, 8), Some(11));
        assert_eq!(next_candidate_cursor(11, 20, 3, 8), Some(19));
        assert_eq!(next_candidate_cursor(19, 20, 3, 8), None);
    }

    #[test]
    fn candidate_cursor_rewinds_by_page_then_individually() {
        assert_eq!(previous_candidate_cursor(19, 3, 8), Some(11));
        assert_eq!(previous_candidate_cursor(11, 3, 8), Some(3));
        assert_eq!(previous_candidate_cursor(3, 3, 8), Some(2));
        assert_eq!(previous_candidate_cursor(2, 3, 8), Some(1));
        assert_eq!(previous_candidate_cursor(1, 3, 8), Some(0));
        assert_eq!(previous_candidate_cursor(0, 3, 8), None);
    }
}
