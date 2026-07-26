use super::*;

pub(super) fn is_preedit_start(event: &KeyEvent) -> bool {
    event.modifiers == Modifiers::SHIFT
        && event
            .text
            .as_deref()
            .is_some_and(|text| text.len() == 1 && text.as_bytes()[0].is_ascii_uppercase())
}

pub(super) fn upper_command_text(command: Option<&str>) -> Option<&str> {
    command
        .and_then(|command| command.strip_prefix("upper-"))
        .filter(|text| !text.is_empty())
}

pub(super) fn command_for_rom_kana<'a>(
    command: Option<&'a str>,
    rule: &RomKanaRule,
    mode: InputMode,
) -> Option<&'a str> {
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

pub(super) fn input_text<'a>(event: &'a KeyEvent, command: Option<&'a str>) -> Option<&'a str> {
    upper_command_text(command).or(event.text.as_deref())
}

pub(super) fn is_upper_input(event: &KeyEvent, command: Option<&str>) -> bool {
    upper_command_text(command).is_some() || is_preedit_start(event)
}

pub(super) fn should_start_okuri(
    event: &KeyEvent,
    command: Option<&str>,
    stem: &Composition,
) -> bool {
    is_upper_input(event, command) && stem.has_output() && !stem.has_pending()
}

pub(super) fn should_restore_preedit(command: Option<&str>, cursor: usize) -> bool {
    command == Some("abort") || command == Some("previous-candidate") && cursor == 0
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
    }
}

pub(super) fn should_forward_after_selection(event: &KeyEvent, command: Option<&str>) -> bool {
    command == Some("delete") || input_text(event, command).is_some()
}

pub(super) fn supports_abbrev(mode: InputMode) -> bool {
    matches!(
        mode,
        InputMode::Hiragana | InputMode::Katakana | InputMode::HankakuKatakana
    )
}

pub(super) fn should_start_abbrev(command: Option<&str>, mode: InputMode, pending: &str) -> bool {
    command == Some("abbrev") && supports_abbrev(mode) && pending.is_empty()
}

pub(super) fn input_mode_for_command(command: &str) -> Option<InputMode> {
    match command {
        "set-input-mode-hiragana" => Some(InputMode::Hiragana),
        "set-input-mode-katakana" => Some(InputMode::Katakana),
        "set-input-mode-hankaku-katakana" => Some(InputMode::HankakuKatakana),
        "set-input-mode-latin" => Some(InputMode::Latin),
        "set-input-mode-wide-latin" => Some(InputMode::WideLatin),
        _ => None,
    }
}

pub(super) fn direct_mode_transition(command: Option<&str>, pending: &str) -> Option<InputMode> {
    pending
        .is_empty()
        .then(|| command.and_then(input_mode_for_command))
        .flatten()
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

pub(super) fn source_order(source: CandidateSource) -> u8 {
    match source {
        CandidateSource::User => 0,
        CandidateSource::System => 1,
    }
}

pub(super) fn compare_candidates(
    left: &Candidate,
    right: &Candidate,
    order: &[CandidateSortKey],
) -> std::cmp::Ordering {
    order
        .iter()
        .map(|key| match key {
            CandidateSortKey::FromUser => {
                source_order(left.source).cmp(&source_order(right.source))
            }
            CandidateSortKey::Recency => right
                .recency
                .unwrap_or_default()
                .total_cmp(&left.recency.unwrap_or_default()),
            CandidateSortKey::AiScore => left.loss.total_cmp(&right.loss),
        })
        .find(|ordering| !ordering.is_eq())
        .unwrap_or(std::cmp::Ordering::Equal)
}

pub(super) fn deduplicate_candidate_outputs(candidates: &mut Vec<Candidate>) {
    let mut seen = HashSet::new();
    candidates.retain(|candidate| seen.insert(candidate.output.clone()));
}

pub(super) fn merge_reranked_candidate(
    candidates: &mut Vec<Candidate>,
    cursor: &mut usize,
    candidate: Candidate,
    order: &[CandidateSortKey],
    preserve_selection: bool,
) {
    let selected = candidates
        .get(*cursor)
        .map(|candidate| candidate.output.clone());
    candidates.push(candidate);
    candidates.sort_by(|left, right| compare_candidates(left, right, order));
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

impl From<&Candidate> for DictionaryCandidate {
    fn from(candidate: &Candidate) -> Self {
        Self {
            input: candidate.input.clone(),
            okuri: candidate.okuri.clone(),
            text: candidate.text.clone(),
            annotation: candidate.annotation.clone(),
            source: candidate.source,
            recency: candidate.recency,
        }
    }
}
