pub(crate) fn dictionary_key(input: &str, okuri: Option<&str>) -> String {
    match okuri.and_then(okuri_prefix) {
        Some(prefix) => format!("{input}{prefix}"),
        None => input.to_owned(),
    }
}

fn okuri_prefix(okuri: &str) -> Option<char> {
    let first = okuri.chars().next()?;
    [
        ("あぁ", 'a'),
        ("いぃ", 'i'),
        ("うぅ", 'u'),
        ("えぇ", 'e'),
        ("おぉ", 'o'),
        ("かきくけこ", 'k'),
        ("がぎぐげご", 'g'),
        ("さしすせそ", 's'),
        ("ざじずぜぞ", 'z'),
        ("たちつてとっ", 't'),
        ("だぢづでど", 'd'),
        ("なにぬねのん", 'n'),
        ("はひふへほ", 'h'),
        ("ばびぶべぼ", 'b'),
        ("ぱぴぷぺぽ", 'p'),
        ("まみむめも", 'm'),
        ("やゆよゃゅょ", 'y'),
        ("らりるれろ", 'r'),
        ("わゐゑを", 'w'),
    ]
    .into_iter()
    .find_map(|(characters, prefix)| characters.contains(first).then_some(prefix))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_okuri_ari_key() {
        assert_eq!(dictionary_key("おく", Some("る")), "おくr");
        assert_eq!(dictionary_key("か", Some("いて")), "かi");
        assert_eq!(dictionary_key("かな", None), "かな");
    }
}
