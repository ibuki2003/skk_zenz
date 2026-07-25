use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use bitflags::bitflags;
use serde::Deserialize;
use serde_json::Value;

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

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
enum Binding {
    Text(String),
    Chord { key: Key, modifiers: Modifiers },
}

#[derive(Debug, Deserialize)]
struct MapFile {
    #[serde(default)]
    include: Vec<String>,
    #[serde(default)]
    define: HashMap<String, HashMap<String, Value>>,
}

struct KeymapLoader {
    search_paths: Vec<PathBuf>,
    entries: HashMap<String, String>,
    loaded: HashSet<(String, String)>,
    loading: HashSet<(String, String)>,
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
        let mut loader = KeymapLoader {
            search_paths: search_paths.to_vec(),
            entries: HashMap::new(),
            loaded: HashSet::new(),
            loading: HashSet::new(),
        };
        loader.load(rule, mode)?;
        let bindings = loader
            .entries
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

impl KeymapLoader {
    fn load(&mut self, rule: &str, name: &str) -> Result<()> {
        let identity = (rule.to_owned(), name.to_owned());
        if self.loaded.contains(&identity) {
            return Ok(());
        }
        if !self.loading.insert(identity.clone()) {
            bail!("keymapのincludeが循環しています: {rule}/{name}");
        }
        let path = self.locate(rule, name)?;
        let contents = fs::read_to_string(&path)
            .with_context(|| format!("keymap定義を読めません: {}", path.display()))?;
        let map_file: MapFile = serde_json::from_str(&contents)
            .with_context(|| format!("keymap定義を解釈できません: {}", path.display()))?;

        for include in map_file.include {
            let (parent_rule, parent_name) = include
                .split_once('/')
                .map_or((rule, include.as_str()), |(rule, name)| (rule, name));
            self.load(parent_rule, parent_name)?;
        }
        if let Some(definitions) = map_file.define.get("keymap") {
            for (key, value) in definitions {
                if value.is_null() {
                    self.entries.remove(key);
                } else {
                    let command = value
                        .as_str()
                        .with_context(|| format!("keymapの{key:?}は文字列またはnullが必要です"))?;
                    self.entries.insert(key.clone(), command.to_owned());
                }
            }
        }
        self.loading.remove(&identity);
        self.loaded.insert(identity);
        Ok(())
    }

    fn locate(&self, rule: &str, name: &str) -> Result<PathBuf> {
        self.search_paths
            .iter()
            .map(|root| root.join(rule).join("keymap").join(format!("{name}.json")))
            .find(|path| path.is_file())
            .with_context(|| {
                format!(
                    "keymap定義が見つかりません: {rule}/keymap/{name}.json (search_paths={:?})",
                    self.search_paths
                )
            })
    }
}

fn parse_binding(value: &str) -> Result<Binding> {
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

fn parse_named_key(value: &str) -> NamedKey {
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
            keymap.lookup(&KeyEvent {
                text: None,
                key: Key::Character('q'),
                modifiers: Modifiers::CONTROL
            }),
            None
        );
    }
}
