use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::{Context, Result};
use bitflags::bitflags;

use crate::rule_file::RuleFileLoader;

mod parser;

use parser::{Binding, parse_binding, parse_named_key};

bitflags! {
    #[derive(Clone, Copy, Debug, Default, Hash, PartialEq, Eq)]
    pub struct Modifiers: u16 {
        const SHIFT = 1 << 0;
        const CONTROL = 1 << 1;
        const ALT = 1 << 2;
        const META = 1 << 3;
        const SUPER = 1 << 4;
        const HYPER = 1 << 5;
        const MOD5 = 1 << 6;
    }
}

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub enum Key {
    Character(char),
    Named(NamedKey),
}

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub enum NamedKey {
    Backspace,
    Enter,
    Escape,
    Tab,
    Space,
    ArrowLeft,
    ArrowRight,
    ArrowUp,
    ArrowDown,
    Other(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyEvent {
    /// キーボードレイアウトとShiftを適用した後の入力文字列。
    pub text: Option<String>,
    /// 修飾キーを適用する前の、レイアウト適用済み論理キー。
    pub key: Key,
    pub modifiers: Modifiers,
}

#[derive(Clone, Debug)]
pub struct Keymap {
    bindings: Vec<(Binding, String)>,
}

impl KeyEvent {
    pub fn character(text: impl Into<String>, key: char, modifiers: Modifiers) -> Self {
        Self {
            text: Some(text.into()),
            key: Key::Character(key),
            modifiers,
        }
    }

    pub fn named(name: impl Into<String>, modifiers: Modifiers) -> Self {
        Self {
            text: None,
            key: Key::Named(parse_named_key(&name.into())),
            modifiers,
        }
    }
}

impl Keymap {
    pub fn load(rule: &str, mode: &str, search_paths: &[PathBuf]) -> Result<Self> {
        let mut loader = RuleFileLoader::new(search_paths, "keymap", "keymap定義");
        let mut entries = HashMap::new();
        for rule_file in loader.load(rule, mode)? {
            if let Some(definitions) = rule_file.define.get("keymap") {
                for (key, value) in definitions {
                    if value.is_null() {
                        entries.remove(key);
                    } else {
                        let command = value.as_str().with_context(|| {
                            format!("keymapの{key:?}は文字列またはnullが必要です")
                        })?;
                        entries.insert(key.clone(), command.to_owned());
                    }
                }
            }
        }
        let bindings = entries
            .into_iter()
            .map(|(key, command)| Ok((parse_binding(&key)?, command)))
            .collect::<Result<_>>()?;
        Ok(Self { bindings })
    }

    pub fn lookup(&self, event: &KeyEvent) -> Option<&str> {
        self.bindings
            .iter()
            .filter(|(binding, _)| matches!(binding, Binding::Chord { .. }))
            .chain(
                self.bindings
                    .iter()
                    .filter(|(binding, _)| matches!(binding, Binding::Text(_))),
            )
            .find_map(|(binding, command)| matches(binding, event).then_some(command.as_str()))
    }
}

fn matches(binding: &Binding, event: &KeyEvent) -> bool {
    match binding {
        Binding::Text(text) => event.text.as_deref() == Some(text),
        Binding::Chord { key, modifiers } => event.key == *key && event.modifiers == *modifiers,
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    #[test]
    fn distinguishes_text_from_unmodified_key() {
        let shifted_one = KeyEvent::character("!", '1', Modifiers::SHIFT);
        assert!(matches(&parse_binding("!").unwrap(), &shifted_one));
        assert!(matches(&parse_binding("S-1").unwrap(), &shifted_one));
        assert!(!matches(&parse_binding("1").unwrap(), &shifted_one));
    }

    #[test]
    fn control_character_uses_logical_key_without_text() {
        let control_q = KeyEvent {
            text: None,
            key: Key::Character('q'),
            modifiers: Modifiers::CONTROL,
        };
        assert!(matches(&parse_binding("C-q").unwrap(), &control_q));
    }

    #[test]
    fn control_code_bindings_match_named_keys() {
        assert!(matches(
            &parse_binding("\u{8}").unwrap(),
            &KeyEvent {
                text: None,
                key: Key::Named(NamedKey::Backspace),
                modifiers: Modifiers::empty()
            }
        ));
    }

    #[test]
    fn loads_includes_overrides_and_deletions() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("test-data/rules");
        let keymap = Keymap::load("custom", "hiragana", &[root]).unwrap();
        assert_eq!(
            keymap.lookup(&KeyEvent::character("!", '1', Modifiers::SHIFT)),
            Some("shift-one")
        );
        assert_eq!(
            keymap.lookup(&KeyEvent::character("X", 'x', Modifiers::SHIFT)),
            Some("uppercase-x")
        );
        assert_eq!(
            keymap.lookup(&KeyEvent::named("Tab", Modifiers::SHIFT)),
            Some("previous-completion")
        );
        assert_eq!(
            keymap.lookup(&KeyEvent {
                text: None,
                key: Key::Character('q'),
                modifiers: Modifiers::CONTROL
            }),
            None
        );
    }
}
