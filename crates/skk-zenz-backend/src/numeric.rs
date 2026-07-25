const KANJI_DIGITS: [&str; 10] = ["〇", "一", "二", "三", "四", "五", "六", "七", "八", "九"];
const DAIJI_DIGITS: [&str; 10] = ["零", "壱", "弐", "参", "四", "伍", "六", "七", "八", "九"];

pub(crate) struct NumericInput {
    pub(crate) key: String,
    values: Vec<String>,
}

impl NumericInput {
    pub(crate) fn extract(input: &str) -> Option<Self> {
        let chars = input.chars().collect::<Vec<_>>();
        let mut key = String::new();
        let mut values = Vec::new();
        let mut index = 0;

        while index < chars.len() {
            if decimal_digit(chars[index]).is_none() {
                key.push(chars[index]);
                index += 1;
                continue;
            }

            let mut value = String::new();
            while index < chars.len() {
                if let Some(digit) = decimal_digit(chars[index]) {
                    value.push(digit);
                    index += 1;
                } else if chars[index] == ','
                    && index + 1 < chars.len()
                    && decimal_digit(chars[index + 1]).is_some()
                {
                    index += 1;
                } else {
                    break;
                }
            }
            key.push('#');
            values.push(value);
        }

        (!values.is_empty()).then_some(Self { key, values })
    }

    pub(crate) fn expand<E>(
        &self,
        text: &str,
        mut reconvert: impl FnMut(&str) -> Result<Vec<String>, E>,
    ) -> Result<Vec<String>, E> {
        let mut outputs = vec![String::new()];
        let mut chars = text.char_indices().peekable();
        let mut value_index = 0;

        while let Some((_, character)) = chars.next() {
            if character != '#' || value_index >= self.values.len() {
                for output in &mut outputs {
                    output.push(character);
                }
                continue;
            }
            let Some(&(_, conversion_type @ '0'..='9')) = chars.peek() else {
                for output in &mut outputs {
                    output.push(character);
                }
                continue;
            };
            chars.next();
            let value = &self.values[value_index];
            let replacements = if conversion_type == '4' {
                let replacements = reconvert(value)?;
                if replacements.is_empty() {
                    vec![value.clone()]
                } else {
                    replacements
                }
            } else {
                vec![
                    convert_numeric(value, conversion_type)
                        .unwrap_or_else(|| format!("#{conversion_type}")),
                ]
            };
            outputs = outputs
                .into_iter()
                .flat_map(|output| {
                    replacements.iter().map(move |replacement| {
                        let mut expanded = output.clone();
                        expanded.push_str(replacement);
                        expanded
                    })
                })
                .collect();
            value_index += 1;
        }
        Ok(outputs)
    }
}

fn decimal_digit(character: char) -> Option<char> {
    match character {
        '0'..='9' => Some(character),
        '０'..='９' => char::from_u32('0' as u32 + character as u32 - '０' as u32),
        _ => None,
    }
}

fn convert_numeric(value: &str, conversion_type: char) -> Option<String> {
    match conversion_type {
        '0' => Some(value.to_owned()),
        '1' => Some(
            value
                .bytes()
                .map(|digit| char::from_u32('０' as u32 + u32::from(digit - b'0')).unwrap())
                .collect(),
        ),
        '2' => Some(map_digits(value, &KANJI_DIGITS)),
        '3' => positional_kanji(value, false),
        '5' => positional_kanji(value, true),
        '8' => Some(group_digits(value)),
        '9' => shogi_digits(value),
        _ => None,
    }
}

fn map_digits(value: &str, digits: &[&str; 10]) -> String {
    value
        .bytes()
        .map(|digit| digits[usize::from(digit - b'0')])
        .collect()
}

fn positional_kanji(value: &str, daiji: bool) -> Option<String> {
    let value = value.trim_start_matches('0');
    if value.is_empty() {
        return Some(if daiji { "零" } else { "〇" }.to_owned());
    }
    if value.len() > 20 {
        return None;
    }

    let digits = if daiji { &DAIJI_DIGITS } else { &KANJI_DIGITS };
    let small_units = if daiji {
        ["", "拾", "百", "阡"]
    } else {
        ["", "十", "百", "千"]
    };
    let large_units = ["", "万", "億", "兆", "京"];
    let mut output = String::new();
    let first_group_len = value.len() % 4;
    let first_group_len = if first_group_len == 0 {
        4
    } else {
        first_group_len
    };
    let mut offset = 0;
    let mut group_len = first_group_len;

    while offset < value.len() {
        let group = &value[offset..offset + group_len];
        let group_index = (value.len() - offset - group_len) / 4;
        let converted = convert_group(group, digits, &small_units, daiji);
        if !converted.is_empty() {
            if !daiji && group_index == 1 && converted == "千" {
                output.push('一');
            }
            output.push_str(&converted);
            output.push_str(large_units[group_index]);
        }
        offset += group_len;
        group_len = 4;
    }
    Some(output)
}

fn convert_group(group: &str, digits: &[&str; 10], units: &[&str; 4], daiji: bool) -> String {
    let mut output = String::new();
    for (index, digit) in group.bytes().enumerate() {
        let digit = usize::from(digit - b'0');
        if digit == 0 {
            continue;
        }
        let position = group.len() - index - 1;
        if daiji || digit != 1 || position == 0 {
            output.push_str(digits[digit]);
        }
        output.push_str(units[position]);
    }
    output
}

fn group_digits(value: &str) -> String {
    let value = value.trim_start_matches('0');
    let value = if value.is_empty() { "0" } else { value };
    let mut output = String::new();
    for (index, character) in value.chars().enumerate() {
        if index > 0 && (value.len() - index).is_multiple_of(3) {
            output.push(',');
        }
        output.push(character);
    }
    output
}

fn shogi_digits(value: &str) -> Option<String> {
    let [file, rank] = value.as_bytes() else {
        return None;
    };
    Some(format!(
        "{}{}",
        char::from_u32('０' as u32 + u32::from(file - b'0')).unwrap(),
        KANJI_DIGITS[usize::from(rank - b'0')]
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_ascii_fullwidth_and_grouped_numbers() {
        let input = NumericInput::extract("だい1,024かいと０９こ").unwrap();
        assert_eq!(input.key, "だい#かいと#こ");
        assert_eq!(input.values, ["1024", "09"]);
    }

    #[test]
    fn expands_numeric_conversion_types_in_order() {
        let input = NumericInput::extract("1024と34").unwrap();
        assert_eq!(
            input
                .expand("#0/#1/#2/#3/#5/#8と#9", |_| {
                    Ok::<_, std::convert::Infallible>(Vec::new())
                })
                .unwrap(),
            ["1024/３４/#2/#3/#5/#8と#9"]
        );

        let input = NumericInput::extract("1024").unwrap();
        let expand = |template| {
            input
                .expand(template, |_| Ok::<_, std::convert::Infallible>(Vec::new()))
                .unwrap()
        };
        assert_eq!(expand("#2"), ["一〇二四"]);
        assert_eq!(expand("#3"), ["千二十四"]);
        assert_eq!(expand("#5"), ["壱阡弐拾四"]);
        assert_eq!(expand("#8"), ["1,024"]);

        let input = NumericInput::extract("34").unwrap();
        assert_eq!(
            input
                .expand("#9", |_| { Ok::<_, std::convert::Infallible>(Vec::new()) })
                .unwrap(),
            ["３四"]
        );
    }

    #[test]
    fn reconversion_expands_to_each_dictionary_candidate() {
        let input = NumericInput::extract("12").unwrap();
        assert_eq!(
            input
                .expand("第#4回", |value| {
                    assert_eq!(value, "12");
                    Ok::<_, std::convert::Infallible>(vec!["十二".to_owned(), "拾弐".to_owned()])
                })
                .unwrap(),
            ["第十二回", "第拾弐回"]
        );
    }

    #[test]
    fn positional_conversion_handles_zero_and_large_units() {
        assert_eq!(convert_numeric("0", '3').as_deref(), Some("〇"));
        assert_eq!(convert_numeric("10", '3').as_deref(), Some("十"));
        assert_eq!(convert_numeric("50000", '3').as_deref(), Some("五万"));
        assert_eq!(
            convert_numeric("11111", '3').as_deref(),
            Some("一万千百十一")
        );
        assert_eq!(convert_numeric("10000000", '3').as_deref(), Some("一千万"));
    }
}
