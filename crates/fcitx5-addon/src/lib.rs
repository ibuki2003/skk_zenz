use std::ffi::{CStr, c_char, c_void};
use std::path::Path;
use std::ptr;

use skk_zenz_backend::{
    Backend, BackendConfig, InputEvent, InputUpdate, Key, KeyEvent, Modifiers, NamedKey,
};

#[repr(C)]
pub struct FfiCandidate {
    text: *const c_char,
    annotation: *const c_char,
}

#[repr(C)]
pub struct FfiUpdate {
    consumed: bool,
    preedit: *const c_char,
    commit: *const c_char,
    candidates: *const FfiCandidate,
    candidate_count: usize,
    selected_candidate: usize,
    input_mode: u8,
    rerank_pending: bool,
    evaluated_candidate_count: usize,
    total_candidate_count: usize,
    candidate_window_visible: bool,
    candidate_page_start: usize,
    candidate_page_size: usize,
    show_annotations: bool,
}

pub type UpdateCallback = unsafe extern "C" fn(*mut c_void, *const FfiUpdate);

unsafe fn required_str<'a>(value: *const c_char) -> Option<&'a str> {
    if value.is_null() {
        return None;
    }
    // SAFETY: The C++ adapter keeps the NUL-terminated string alive for this call.
    unsafe { CStr::from_ptr(value) }.to_str().ok()
}

fn with_update(
    update: &InputUpdate,
    rerank_pending: bool,
    user_data: *mut c_void,
    callback: UpdateCallback,
) {
    let preedit = nul_terminated(&update.preedit);
    let commit = update.commit.as_deref().map(nul_terminated);
    let texts: Vec<Vec<u8>> = update
        .candidates
        .iter()
        .map(|candidate| nul_terminated(&candidate.output))
        .collect();
    let annotations: Vec<Option<Vec<u8>>> = update
        .candidates
        .iter()
        .map(|candidate| candidate.annotation.as_deref().map(nul_terminated))
        .collect();
    let candidates: Vec<FfiCandidate> = texts
        .iter()
        .zip(&annotations)
        .map(|(text, annotation)| FfiCandidate {
            text: text.as_ptr().cast(),
            annotation: annotation
                .as_ref()
                .map_or(ptr::null(), |value| value.as_ptr().cast()),
        })
        .collect();
    let ffi_update = FfiUpdate {
        consumed: update.consumed,
        preedit: preedit.as_ptr().cast(),
        commit: commit
            .as_ref()
            .map_or(ptr::null(), |value| value.as_ptr().cast()),
        candidates: candidates.as_ptr(),
        candidate_count: candidates.len(),
        selected_candidate: update.selected_candidate.unwrap_or(usize::MAX),
        input_mode: match update.input_mode {
            skk_zenz_backend::InputMode::Hiragana => 0,
            skk_zenz_backend::InputMode::Katakana => 1,
            skk_zenz_backend::InputMode::HankakuKatakana => 2,
            skk_zenz_backend::InputMode::Latin => 3,
            skk_zenz_backend::InputMode::WideLatin => 4,
        },
        rerank_pending,
        evaluated_candidate_count: update.rerank_progress.map_or(0, |progress| progress.0),
        total_candidate_count: update.rerank_progress.map_or(0, |progress| progress.1),
        candidate_window_visible: update.candidate_window_visible,
        candidate_page_start: update.candidate_page_start,
        candidate_page_size: update.candidate_page_size,
        show_annotations: update.show_annotations,
    };
    // SAFETY: All pointers in ffi_update remain valid for the synchronous callback.
    unsafe { callback(user_data, &raw const ffi_update) };
}

fn nul_terminated(value: &str) -> Vec<u8> {
    let mut bytes: Vec<u8> = value
        .as_bytes()
        .iter()
        .copied()
        .filter(|byte| *byte != 0)
        .collect();
    bytes.push(0);
    bytes
}

fn dispatch(
    backend: *mut Backend,
    event: InputEvent,
    user_data: *mut c_void,
    callback: UpdateCallback,
) -> bool {
    // SAFETY: backend is created and exclusively owned by the C++ engine.
    let Some(backend) = (unsafe { backend.as_mut() }) else {
        return false;
    };
    match backend.handle_event(event) {
        Ok(update) => {
            with_update(&update, backend.rerank_pending(), user_data, callback);
            true
        }
        Err(error) => {
            eprintln!("skk-zenz: backend event failed: {error:#}");
            false
        }
    }
}

/// Applies any completed asynchronous candidate evaluations.
///
/// # Safety
/// `backend` must be a live pointer returned by this library and callback must
/// be callable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn skk_zenz_backend_poll_rerank(
    backend: *mut Backend,
    user_data: *mut c_void,
    callback: UpdateCallback,
) -> bool {
    // SAFETY: backend is created and exclusively owned by the C++ engine.
    let Some(backend) = (unsafe { backend.as_mut() }) else {
        return false;
    };
    if let Some(update) = backend.poll_rerank() {
        with_update(&update, backend.rerank_pending(), user_data, callback);
    }
    backend.rerank_pending()
}

/// Creates the backend from a TOML file.
///
/// # Safety
/// `config_path` must point to a valid NUL-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn skk_zenz_backend_new(config_path: *const c_char) -> *mut Backend {
    let config = if config_path.is_null() {
        BackendConfig::from_xdg()
    } else {
        let Some(config_path) = (unsafe { required_str(config_path) }) else {
            return ptr::null_mut();
        };
        BackendConfig::from_toml_file(Path::new(config_path))
    };
    match config.and_then(Backend::load) {
        Ok(backend) => Box::into_raw(Box::new(backend)),
        Err(error) => {
            eprintln!("skk-zenz: backend initialization failed: {error:#}");
            ptr::null_mut()
        }
    }
}

/// Destroys a backend returned by `skk_zenz_backend_new`.
///
/// # Safety
/// `backend` must either be null or a live pointer returned by this library.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn skk_zenz_backend_free(backend: *mut Backend) {
    if !backend.is_null() {
        // SAFETY: Ownership is transferred back exactly once by the C++ engine.
        drop(unsafe { Box::from_raw(backend) });
    }
}

/// Sends one fcitx5 key event to the backend.
///
/// # Safety
/// String pointers must be valid for this call and `callback` must be callable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn skk_zenz_backend_key(
    backend: *mut Backend,
    text: *const c_char,
    key: *const c_char,
    modifiers: u16,
    user_data: *mut c_void,
    callback: UpdateCallback,
) -> bool {
    let text = unsafe { required_str(text) }.map(str::to_owned);
    let Some(key) = (unsafe { required_str(key) }).map(parse_key) else {
        return false;
    };
    dispatch(
        backend,
        InputEvent::Key(KeyEvent {
            text,
            key,
            modifiers: Modifiers::from_bits_truncate(modifiers),
        }),
        user_data,
        callback,
    )
}

/// Synchronizes the left surrounding context.
///
/// # Safety
/// `context` must be a valid NUL-terminated string and callback must be callable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn skk_zenz_backend_set_context(
    backend: *mut Backend,
    context: *const c_char,
    user_data: *mut c_void,
    callback: UpdateCallback,
) -> bool {
    let Some(context) = (unsafe { required_str(context) }) else {
        return false;
    };
    dispatch(
        backend,
        InputEvent::SetContext(context.to_owned()),
        user_data,
        callback,
    )
}

#[unsafe(no_mangle)]
pub extern "C" fn skk_zenz_backend_focus_out(
    backend: *mut Backend,
    user_data: *mut c_void,
    callback: UpdateCallback,
) -> bool {
    dispatch(
        backend,
        InputEvent::FocusOut { commit: false },
        user_data,
        callback,
    )
}

#[unsafe(no_mangle)]
pub extern "C" fn skk_zenz_backend_reset(
    backend: *mut Backend,
    user_data: *mut c_void,
    callback: UpdateCallback,
) -> bool {
    dispatch(backend, InputEvent::Reset, user_data, callback)
}

#[unsafe(no_mangle)]
pub extern "C" fn skk_zenz_backend_set_input_mode(
    backend: *mut Backend,
    mode: u8,
    user_data: *mut c_void,
    callback: UpdateCallback,
) -> bool {
    let mode = match mode {
        0 => skk_zenz_backend::InputMode::Hiragana,
        1 => skk_zenz_backend::InputMode::Katakana,
        2 => skk_zenz_backend::InputMode::HankakuKatakana,
        3 => skk_zenz_backend::InputMode::Latin,
        4 => skk_zenz_backend::InputMode::WideLatin,
        _ => return false,
    };
    dispatch(backend, InputEvent::SetInputMode(mode), user_data, callback)
}

#[unsafe(no_mangle)]
pub extern "C" fn skk_zenz_backend_select_candidate(
    backend: *mut Backend,
    index: usize,
    user_data: *mut c_void,
    callback: UpdateCallback,
) -> bool {
    dispatch(
        backend,
        InputEvent::SelectCandidate(index),
        user_data,
        callback,
    )
}

fn parse_key(value: &str) -> Key {
    let mut characters = value.chars();
    if let (Some(character), None) = (characters.next(), characters.next()) {
        return Key::Character(character);
    }
    Key::Named(match value {
        "BackSpace" => NamedKey::Backspace,
        "Return" | "KP_Enter" => NamedKey::Enter,
        "Escape" => NamedKey::Escape,
        "Tab" | "ISO_Left_Tab" => NamedKey::Tab,
        "space" => NamedKey::Space,
        "Left" => NamedKey::ArrowLeft,
        "Right" => NamedKey::ArrowRight,
        "Up" => NamedKey::ArrowUp,
        "Down" => NamedKey::ArrowDown,
        other => NamedKey::Other(other.to_owned()),
    })
}

#[cfg(test)]
mod tests {
    use super::parse_key;
    use skk_zenz_backend::{Key, NamedKey};

    #[test]
    fn parses_character_and_named_keys() {
        assert_eq!(parse_key("a"), Key::Character('a'));
        assert_eq!(parse_key("BackSpace"), Key::Named(NamedKey::Backspace));
        assert_eq!(
            parse_key("Muhenkan"),
            Key::Named(NamedKey::Other("Muhenkan".to_owned()))
        );
    }
}
