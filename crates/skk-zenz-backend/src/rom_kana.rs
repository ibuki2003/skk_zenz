use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use serde_json::Value;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum KanaMode {
    #[default]
    Hiragana,
    Katakana,
    HankakuKatakana,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KanaConversion {
    pub output: String,
    pub preedit: String,
}

#[derive(Clone, Debug)]
pub struct RomKanaRule {
    entries: HashMap<String, Entry>,
    prefixes: HashSet<String>,
}

#[derive(Clone, Debug)]
struct Entry {
    carryover: String,
    hiragana: String,
    katakana: String,
    hankaku_katakana: String,
}

#[derive(Debug, Deserialize)]
struct MapFile {
    #[serde(default)]
    include: Vec<String>,
    #[serde(default)]
    define: HashMap<String, HashMap<String, Value>>,
}

struct RuleLoader {
    search_paths: Vec<PathBuf>,
    entries: HashMap<String, Entry>,
    loaded: HashSet<(String, String)>,
    loading: HashSet<(String, String)>,
}

impl RomKanaRule {
    pub fn load(rule: &str, search_paths: &[PathBuf]) -> Result<Self> {
        let mut loader = RuleLoader {
            search_paths: search_paths.to_vec(),
            entries: HashMap::new(),
            loaded: HashSet::new(),
            loading: HashSet::new(),
        };
        loader.load(rule, "default")?;
        let prefixes = loader
            .entries
            .keys()
            .flat_map(|key| (1..=key.len()).map(|length| key[..length].to_owned()))
            .collect();
        Ok(Self {
            entries: loader.entries,
            prefixes,
        })
    }

    pub fn convert(&self, input: &str, mode: KanaMode) -> KanaConversion {
        let mut conversion = self.convert_partial(input, mode);
        if conversion.preedit == "n" {
            conversion.output.push_str(mode.n());
            conversion.preedit.clear();
        }
        conversion
    }

    pub fn convert_partial(&self, input: &str, mode: KanaMode) -> KanaConversion {
        let mut conversion = Conversion::new(self, mode);
        for character in input.chars() {
            conversion.append(character);
        }
        KanaConversion {
            output: conversion.output,
            preedit: conversion.preedit,
        }
    }
}

impl RuleLoader {
    fn load(&mut self, rule: &str, name: &str) -> Result<()> {
        let identity = (rule.to_owned(), name.to_owned());
        if self.loaded.contains(&identity) {
            return Ok(());
        }
        if !self.loading.insert(identity.clone()) {
            bail!("rom-kana ruleのincludeが循環しています: {rule}/{name}");
        }

        let path = self.locate(rule, name)?;
        let contents = fs::read_to_string(&path)
            .with_context(|| format!("rom-kana定義を読めません: {}", path.display()))?;
        let map_file: MapFile = serde_json::from_str(&contents)
            .with_context(|| format!("rom-kana定義を解釈できません: {}", path.display()))?;

        for include in map_file.include {
            let (parent_rule, parent_name) = include
                .split_once('/')
                .map_or((rule, include.as_str()), |(rule, name)| (rule, name));
            self.load(parent_rule, parent_name)?;
        }
        if let Some(definitions) = map_file.define.get("rom-kana") {
            for (romaji, value) in definitions {
                if value.is_null() {
                    self.entries.remove(romaji);
                } else {
                    self.entries
                        .insert(romaji.clone(), parse_entry(romaji, value)?);
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
            .map(|root| {
                root.join(rule)
                    .join("rom-kana")
                    .join(format!("{name}.json"))
            })
            .find(|path| path.is_file())
            .with_context(|| {
                format!(
                    "rom-kana定義が見つかりません: {rule}/rom-kana/{name}.json (search_paths={:?})",
                    self.search_paths
                )
            })
    }
}

fn parse_entry(romaji: &str, value: &Value) -> Result<Entry> {
    if romaji.is_empty() || !romaji.is_ascii() {
        bail!("rom-kanaのキーは空でないASCII文字列である必要があります: {romaji:?}");
    }
    let components = value
        .as_array()
        .with_context(|| format!("rom-kanaの{romaji:?}は配列またはnullである必要があります"))?;
    if !(2..=4).contains(&components.len()) {
        bail!("rom-kanaの{romaji:?}は2〜4要素である必要があります");
    }
    let component = |index: usize| -> Result<&str> {
        components[index]
            .as_str()
            .with_context(|| format!("rom-kanaの{romaji:?}の要素は文字列である必要があります"))
    };
    let hiragana = component(1)?.to_owned();
    let katakana = match components.get(2) {
        Some(value) => value
            .as_str()
            .with_context(|| format!("rom-kanaの{romaji:?}の要素は文字列である必要があります"))?
            .to_owned(),
        None => hiragana_to_katakana(&hiragana),
    };
    let hankaku_katakana = match components.get(3) {
        Some(value) => value
            .as_str()
            .with_context(|| format!("rom-kanaの{romaji:?}の要素は文字列である必要があります"))?
            .to_owned(),
        None => katakana_to_hankaku(&katakana),
    };
    Ok(Entry {
        carryover: component(0)?.to_owned(),
        hiragana,
        katakana,
        hankaku_katakana,
    })
}

struct Conversion<'rule> {
    rule: &'rule RomKanaRule,
    mode: KanaMode,
    output: String,
    preedit: String,
}

impl<'rule> Conversion<'rule> {
    fn new(rule: &'rule RomKanaRule, mode: KanaMode) -> Self {
        Self {
            rule,
            mode,
            output: String::new(),
            preedit: String::new(),
        }
    }

    fn append(&mut self, character: char) {
        self.preedit.push(character);
        if self.rule.prefixes.contains(&self.preedit) {
            let is_terminal = !self
                .rule
                .entries
                .keys()
                .any(|key| key.len() > self.preedit.len() && key.starts_with(&self.preedit));
            if is_terminal {
                let entry = self
                    .rule
                    .entries
                    .get(&self.preedit)
                    .expect("terminal prefixにはentryがある");
                self.output.push_str(entry.kana(self.mode));
                let carryover = entry.carryover.clone();
                self.preedit.clear();
                for character in carryover.chars() {
                    self.append(character);
                }
            }
            return;
        }

        let last = self.preedit.pop().expect("直前に追加済み");
        if self.preedit == "n" {
            self.output.push_str(self.mode.n());
        }
        self.preedit.clear();
        let root = last.to_string();
        if self.rule.prefixes.contains(&root) {
            self.append(last);
        } else {
            self.output.push(last);
        }
    }
}

impl Entry {
    fn kana(&self, mode: KanaMode) -> &str {
        match mode {
            KanaMode::Hiragana => &self.hiragana,
            KanaMode::Katakana => &self.katakana,
            KanaMode::HankakuKatakana => &self.hankaku_katakana,
        }
    }
}

impl KanaMode {
    fn n(self) -> &'static str {
        match self {
            Self::Hiragana => "ん",
            Self::Katakana => "ン",
            Self::HankakuKatakana => "ﾝ",
        }
    }
}

fn hiragana_to_katakana(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if ('ぁ'..='ゖ').contains(&character) {
                char::from_u32(character as u32 + 0x60).expect("ひらがなの変換先は有効")
            } else {
                character
            }
        })
        .collect()
}

pub(crate) fn convert_kana_script(value: &str, mode: KanaMode) -> String {
    match mode {
        KanaMode::Hiragana => value
            .chars()
            .map(|character| match character {
                '\u{30a1}'..='\u{30f6}' => {
                    char::from_u32(character as u32 - 0x60).expect("hiragana mapping is valid")
                }
                _ => character,
            })
            .collect(),
        KanaMode::Katakana => hiragana_to_katakana(value),
        KanaMode::HankakuKatakana => katakana_to_hankaku(&hiragana_to_katakana(value)),
    }
}

fn katakana_to_hankaku(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            let converted = match character {
                '。' => "｡",
                '「' => "｢",
                '」' => "｣",
                '、' => "､",
                '・' => "･",
                'ヲ' => "ｦ",
                'ァ' => "ｧ",
                'ィ' => "ｨ",
                'ゥ' => "ｩ",
                'ェ' => "ｪ",
                'ォ' => "ｫ",
                'ャ' => "ｬ",
                'ュ' => "ｭ",
                'ョ' => "ｮ",
                'ッ' => "ｯ",
                'ー' => "ｰ",
                'ア' => "ｱ",
                'イ' => "ｲ",
                'ウ' => "ｳ",
                'エ' => "ｴ",
                'オ' => "ｵ",
                'カ' => "ｶ",
                'キ' => "ｷ",
                'ク' => "ｸ",
                'ケ' => "ｹ",
                'コ' => "ｺ",
                'サ' => "ｻ",
                'シ' => "ｼ",
                'ス' => "ｽ",
                'セ' => "ｾ",
                'ソ' => "ｿ",
                'タ' => "ﾀ",
                'チ' => "ﾁ",
                'ツ' => "ﾂ",
                'テ' => "ﾃ",
                'ト' => "ﾄ",
                'ナ' => "ﾅ",
                'ニ' => "ﾆ",
                'ヌ' => "ﾇ",
                'ネ' => "ﾈ",
                'ノ' => "ﾉ",
                'ハ' => "ﾊ",
                'ヒ' => "ﾋ",
                'フ' => "ﾌ",
                'ヘ' => "ﾍ",
                'ホ' => "ﾎ",
                'マ' => "ﾏ",
                'ミ' => "ﾐ",
                'ム' => "ﾑ",
                'メ' => "ﾒ",
                'モ' => "ﾓ",
                'ヤ' => "ﾔ",
                'ユ' => "ﾕ",
                'ヨ' => "ﾖ",
                'ラ' => "ﾗ",
                'リ' => "ﾘ",
                'ル' => "ﾙ",
                'レ' => "ﾚ",
                'ロ' => "ﾛ",
                'ワ' => "ﾜ",
                'ン' => "ﾝ",
                'ガ' => "ｶﾞ",
                'ギ' => "ｷﾞ",
                'グ' => "ｸﾞ",
                'ゲ' => "ｹﾞ",
                'ゴ' => "ｺﾞ",
                'ザ' => "ｻﾞ",
                'ジ' => "ｼﾞ",
                'ズ' => "ｽﾞ",
                'ゼ' => "ｾﾞ",
                'ゾ' => "ｿﾞ",
                'ダ' => "ﾀﾞ",
                'ヂ' => "ﾁﾞ",
                'ヅ' => "ﾂﾞ",
                'デ' => "ﾃﾞ",
                'ド' => "ﾄﾞ",
                'バ' => "ﾊﾞ",
                'ビ' => "ﾋﾞ",
                'ブ' => "ﾌﾞ",
                'ベ' => "ﾍﾞ",
                'ボ' => "ﾎﾞ",
                'パ' => "ﾊﾟ",
                'ピ' => "ﾋﾟ",
                'プ' => "ﾌﾟ",
                'ペ' => "ﾍﾟ",
                'ポ' => "ﾎﾟ",
                'ヴ' => "ｳﾞ",
                _ => return character.to_string(),
            };
            converted.to_owned()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(entries: &[(&str, &str, &str)]) -> RomKanaRule {
        let entries: HashMap<_, _> = entries
            .iter()
            .map(|(romaji, carryover, kana)| {
                (
                    (*romaji).to_owned(),
                    Entry {
                        carryover: (*carryover).to_owned(),
                        hiragana: (*kana).to_owned(),
                        katakana: hiragana_to_katakana(kana),
                        hankaku_katakana: katakana_to_hankaku(&hiragana_to_katakana(kana)),
                    },
                )
            })
            .collect();
        let prefixes = entries
            .keys()
            .flat_map(|key| (1..=key.len()).map(|length| key[..length].to_owned()))
            .collect();
        RomKanaRule { entries, prefixes }
    }

    #[test]
    fn converts_and_flushes_terminal_n() {
        let rule = rule(&[("ka", "", "か"), ("n", "", "ん")]);
        assert_eq!(
            rule.convert("kan", KanaMode::Hiragana),
            KanaConversion {
                output: "かん".to_owned(),
                preedit: String::new()
            }
        );
    }

    #[test]
    fn partial_conversion_retains_terminal_n() {
        let rule = rule(&[("na", "", "な")]);
        assert_eq!(rule.convert_partial("n", KanaMode::Hiragana).preedit, "n");
        assert_eq!(rule.convert("n", KanaMode::Hiragana).output, "ん");
    }

    #[test]
    fn applies_carryover() {
        let rule = rule(&[("ka", "", "か"), ("kk", "k", "っ")]);
        assert_eq!(rule.convert("kka", KanaMode::Hiragana).output, "っか");
    }

    #[test]
    fn retains_incomplete_preedit() {
        let rule = rule(&[("kya", "", "きゃ")]);
        assert_eq!(rule.convert("ky", KanaMode::Hiragana).preedit, "ky");
    }

    #[test]
    fn loads_cross_rule_and_local_includes_in_order() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("test-data/rules");
        let rule = RomKanaRule::load("custom", &[root]).unwrap();
        assert_eq!(rule.convert("ka", KanaMode::Hiragana).output, "カ");
        assert_eq!(rule.convert("nn", KanaMode::Hiragana).output, "ん");
        assert_eq!(rule.convert("xi", KanaMode::Hiragana).output, "xi");
    }

    #[test]
    fn derives_katakana_variants_when_omitted() {
        let rule = rule(&[("ga", "", "が")]);
        assert_eq!(rule.convert("ga", KanaMode::Katakana).output, "ガ");
        assert_eq!(rule.convert("ga", KanaMode::HankakuKatakana).output, "ｶﾞ");
    }

    #[test]
    fn converts_existing_kana_between_input_modes() {
        assert_eq!(convert_kana_script("かなー", KanaMode::Katakana), "カナー");
        assert_eq!(convert_kana_script("カナー", KanaMode::Hiragana), "かなー");
        assert_eq!(
            convert_kana_script("かなー", KanaMode::HankakuKatakana),
            "ｶﾅｰ"
        );
    }
}
