use anyhow::{Result, bail};

use super::{Key, Modifiers, NamedKey};

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub(super) enum Binding {
    Text(String),
    Chord { key: Key, modifiers: Modifiers },
}

pub(super) fn parse_binding(value: &str) -> Result<Binding> {
    let control_key = match value {
        "\u{8}" => Some(NamedKey::Backspace),
        "\n" | "\r" => Some(NamedKey::Enter),
        "\t" => Some(NamedKey::Tab),
        _ => None,
    };
    if let Some(key) = control_key {
        return Ok(Binding::Chord {
            key: Key::Named(key),
            modifiers: Modifiers::empty(),
        });
    }
    if let Some(inner) = value
        .strip_prefix('(')
        .and_then(|value| value.strip_suffix(')'))
    {
        return parse_long_chord(inner);
    }
    let mut parts = value.split('-').peekable();
    let mut modifiers = Modifiers::empty();
    while let Some(part) = parts.peek().copied() {
        let modifier = match part {
            "S" => Modifiers::SHIFT,
            "C" => Modifiers::CONTROL,
            "A" => Modifiers::ALT,
            "M" => Modifiers::META,
            "G" => Modifiers::MOD5,
            _ => break,
        };
        modifiers |= modifier;
        parts.next();
    }
    if !modifiers.is_empty() {
        let base = parts.collect::<Vec<_>>().join("-");
        if base.is_empty() {
            bail!("keybindの修飾キーの後にキーがありません: {value:?}");
        }
        return Ok(Binding::Chord {
            key: parse_key(&base),
            modifiers,
        });
    }
    if value.chars().count() == 1 {
        return Ok(Binding::Text(value.to_owned()));
    }
    Ok(Binding::Chord {
        key: parse_key(value),
        modifiers,
    })
}

fn parse_long_chord(value: &str) -> Result<Binding> {
    let mut parts = value.split_whitespace().peekable();
    let mut modifiers = Modifiers::empty();
    while let Some(part) = parts.peek().copied() {
        let modifier = match part {
            "shift" => Modifiers::SHIFT,
            "control" => Modifiers::CONTROL,
            "alt" => Modifiers::ALT,
            "meta" => Modifiers::META,
            "super" => Modifiers::SUPER,
            "hyper" => Modifiers::HYPER,
            _ => break,
        };
        modifiers |= modifier;
        parts.next();
    }
    let base = parts.collect::<Vec<_>>().join(" ");
    if base.is_empty() {
        bail!("keybindにキーがありません: ({value})");
    }
    Ok(Binding::Chord {
        key: parse_key(&base),
        modifiers,
    })
}

fn parse_key(value: &str) -> Key {
    let mut characters = value.chars();
    match (characters.next(), characters.next()) {
        (Some(character), None) => Key::Character(normalize_base_character(character)),
        _ => Key::Named(parse_named_key(value)),
    }
}

pub(super) fn parse_named_key(value: &str) -> NamedKey {
    match value {
        "BackSpace" | "Backspace" => NamedKey::Backspace,
        "Return" | "Enter" => NamedKey::Enter,
        "Escape" | "Esc" => NamedKey::Escape,
        "Tab" => NamedKey::Tab,
        "space" | "SPC" => NamedKey::Space,
        "Left" => NamedKey::ArrowLeft,
        "Right" => NamedKey::ArrowRight,
        "Up" => NamedKey::ArrowUp,
        "Down" => NamedKey::ArrowDown,
        _ => NamedKey::Other(value.to_owned()),
    }
}

fn normalize_base_character(character: char) -> char {
    if character.is_ascii_alphabetic() {
        character.to_ascii_lowercase()
    } else {
        character
    }
}
