use super::*;

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
