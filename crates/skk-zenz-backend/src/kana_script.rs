use crate::rom_kana::KanaMode;

pub(crate) fn hiragana_to_katakana(value: &str) -> String {
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

pub(crate) fn katakana_to_hankaku(value: &str) -> String {
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
