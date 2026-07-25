use std::collections::HashMap;
use std::ffi::{CStr, c_char};
use std::io::{IsTerminal, stderr};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

static EVENT_SEQUENCE: AtomicU64 = AtomicU64::new(1);
static CONTEXTS: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
static USE_COLOR: OnceLock<bool> = OnceLock::new();

const KEY_COLOR: &str = "\x1b[36m";
const SURROUNDING_COLOR: &str = "\x1b[35m";
const GUESSED_COLOR: &str = "\x1b[32m";
const RESET_COLOR: &str = "\x1b[0m";

fn timestamp_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn sequence() -> u64 {
    EVENT_SEQUENCE.fetch_add(1, Ordering::Relaxed)
}

fn contexts() -> &'static Mutex<HashMap<String, String>> {
    CONTEXTS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn use_color() -> bool {
    *USE_COLOR.get_or_init(|| stderr().is_terminal() && std::env::var_os("NO_COLOR").is_none())
}

fn label(name: &str, color: &str) -> String {
    if use_color() {
        format!("{color}[{name}]{RESET_COLOR}")
    } else {
        format!("[{name}]")
    }
}

unsafe fn from_c_str<'a>(value: *const c_char) -> &'a str {
    if value.is_null() {
        return "<null>";
    }

    // SAFETY: All callers pass pointers to NUL-terminated strings that remain
    // alive and unchanged until the FFI call returns.
    unsafe { CStr::from_ptr(value) }
        .to_str()
        .unwrap_or("<invalid UTF-8>")
}

fn appendable_character(text: &str) -> Option<char> {
    let mut characters = text.chars();
    let character = characters.next()?;
    (characters.next().is_none() && !character.is_control()).then_some(character)
}

#[derive(Debug, PartialEq, Eq)]
enum Guess {
    Append(char),
    Pop(Option<char>),
    Flush(String),
}

fn apply_key(context: &mut String, is_backspace: bool, append: &str) -> Guess {
    if is_backspace {
        return Guess::Pop(context.pop());
    }

    if let Some(character) = appendable_character(append) {
        context.push(character);
        return Guess::Append(character);
    }

    Guess::Flush(std::mem::take(context))
}

/// Records a key before the active input method has a chance to consume it.
///
/// `uuid` and `key` must point to NUL-terminated strings for the duration of
/// this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn skk_zenz_log_key(uuid: *const c_char, key: *const c_char, release: bool) {
    // SAFETY: The C++ adapter guarantees valid pointers for this call.
    let (uuid, key) = unsafe { (from_c_str(uuid), from_c_str(key)) };

    eprintln!(
        "{} ts_ms={} seq={} uuid={} key={key:?} release={release}",
        label("KEY", KEY_COLOR),
        timestamp_ms(),
        sequence(),
        uuid,
    );
}

/// Updates context from a key that passed through the active input method.
///
/// Filtered keys never reach this function; accepted keys are excluded by the
/// adapter. `append` is empty when the key cannot safely be interpreted as one
/// printable character.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn skk_zenz_handle_unconsumed_key(
    uuid: *const c_char,
    release: bool,
    is_backspace: bool,
    ignore: bool,
    append: *const c_char,
) {
    // SAFETY: The C++ adapter guarantees valid pointers for this call.
    let (uuid, append) = unsafe { (from_c_str(uuid).to_owned(), from_c_str(append)) };

    if release || ignore {
        return;
    }

    let mut contexts = contexts()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let context = contexts.entry(uuid.clone()).or_default();
    let guess = apply_key(context, is_backspace, append);

    match guess {
        Guess::Append(character) => eprintln!(
            "{} ts_ms={} seq={} uuid={} action=append value={character:?} context={context:?}",
            label("GUESSED", GUESSED_COLOR),
            timestamp_ms(),
            sequence(),
            uuid,
        ),
        Guess::Pop(character) => eprintln!(
            "{} ts_ms={} seq={} uuid={} action=pop value={character:?} context={context:?}",
            label("GUESSED", GUESSED_COLOR),
            timestamp_ms(),
            sequence(),
            uuid,
        ),
        Guess::Flush(previous) => eprintln!(
            "{} ts_ms={} seq={} uuid={} action=flush previous={previous:?} context={context:?}",
            label("GUESSED", GUESSED_COLOR),
            timestamp_ms(),
            sequence(),
            uuid,
        ),
    }
}

fn committed_left(text: &str, cursor: Option<u32>) -> &str {
    cursor.map_or(text, |cursor| split_at_char(text, cursor).0)
}

/// Appends text committed by the active input method to its UUID's context.
///
/// An empty commit is the surrounding-text workaround and intentionally has
/// no effect. For commit-with-cursor, only the part before the resulting cursor
/// belongs to the left context.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn skk_zenz_handle_commit(
    uuid: *const c_char,
    text: *const c_char,
    has_cursor: bool,
    cursor: u32,
) {
    // SAFETY: The C++ adapter guarantees valid pointers for this call.
    let (uuid, text) = unsafe { (from_c_str(uuid).to_owned(), from_c_str(text)) };
    if text.is_empty() {
        return;
    }

    let committed = committed_left(text, has_cursor.then_some(cursor));
    let mut contexts = contexts()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let context = contexts.entry(uuid.clone()).or_default();
    context.push_str(committed);
    eprintln!(
        "{} ts_ms={} seq={} uuid={} action=append source=commit value={committed:?} context={context:?}",
        label("GUESSED", GUESSED_COLOR),
        timestamp_ms(),
        sequence(),
        uuid,
    );
}

fn split_at_char(text: &str, cursor: u32) -> (&str, &str) {
    let byte_offset = text
        .char_indices()
        .nth(cursor as usize)
        .map_or(text.len(), |(offset, _)| offset);
    text.split_at(byte_offset)
}

/// Records surrounding text and, when valid, replaces the context for its UUID.
///
/// `uuid` and `text` must point to NUL-terminated strings for the duration of
/// this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn skk_zenz_handle_surrounding_text(
    uuid: *const c_char,
    valid: bool,
    text: *const c_char,
    cursor: u32,
    anchor: u32,
) {
    // SAFETY: The C++ adapter guarantees valid pointers for this call.
    let (uuid, text) = unsafe { (from_c_str(uuid).to_owned(), from_c_str(text)) };

    if !valid {
        eprintln!(
            "{} ts_ms={} seq={} uuid={} status=unavailable",
            label("SURROUNDING", SURROUNDING_COLOR),
            timestamp_ms(),
            sequence(),
            uuid,
        );
        return;
    }

    let (left, right) = split_at_char(text, cursor);
    eprintln!(
        "{} ts_ms={} seq={} uuid={} left={left:?} right={right:?} cursor={cursor} anchor={anchor}",
        label("SURROUNDING", SURROUNDING_COLOR),
        timestamp_ms(),
        sequence(),
        uuid,
    );

    let mut contexts = contexts()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    contexts.insert(uuid.clone(), left.to_owned());
    eprintln!(
        "{} ts_ms={} seq={} uuid={} action=replace context={left:?}",
        label("GUESSED", GUESSED_COLOR),
        timestamp_ms(),
        sequence(),
        uuid,
    );
}

#[cfg(test)]
mod tests {
    use super::{Guess, apply_key, committed_left, split_at_char};

    #[test]
    fn applies_append_pop_and_conservative_flush() {
        let mut context = String::new();
        assert_eq!(apply_key(&mut context, false, "あ"), Guess::Append('あ'));
        assert_eq!(context, "あ");

        assert_eq!(apply_key(&mut context, true, ""), Guess::Pop(Some('あ')));
        assert_eq!(context, "");

        context.push_str("known");
        assert_eq!(
            apply_key(&mut context, false, ""),
            Guess::Flush("known".to_owned())
        );
        assert_eq!(context, "");
    }

    #[test]
    fn refuses_control_or_multiple_characters() {
        let mut context = "known".to_owned();
        assert_eq!(
            apply_key(&mut context, false, "\n"),
            Guess::Flush("known".to_owned())
        );

        context.push_str("known");
        assert_eq!(
            apply_key(&mut context, false, "ab"),
            Guess::Flush("known".to_owned())
        );
    }

    #[test]
    fn splits_utf8_text_at_character_offset() {
        assert_eq!(split_at_char("abc日本語", 4), ("abc日", "本語"));
    }

    #[test]
    fn clamps_out_of_range_cursor_to_end() {
        assert_eq!(split_at_char("日本語", 99), ("日本語", ""));
    }

    #[test]
    fn commit_with_cursor_only_adds_its_left_part() {
        assert_eq!(committed_left("abc日本語", None), "abc日本語");
        assert_eq!(committed_left("abc日本語", Some(4)), "abc日");
    }
}
