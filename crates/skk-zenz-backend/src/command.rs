use crate::input::InputMode;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    Abort,
    Delete,
    Commit,
    CommitUnhandled,
    Complete,
    PreviousCompletion,
    NextCandidate,
    PreviousCandidate,
    Previous,
    StartPreedit,
    SpecialMidasi,
    Abbrev,
    PurgeCandidate,
    SetInputMode(InputMode),
    Upper(String),
}

impl Command {
    pub(crate) fn parse(value: &str) -> Option<Self> {
        let command = match value {
            "abort" => Self::Abort,
            "delete" => Self::Delete,
            "commit" => Self::Commit,
            "commit-unhandled" => Self::CommitUnhandled,
            "complete" => Self::Complete,
            "previous-completion" => Self::PreviousCompletion,
            "next-candidate" => Self::NextCandidate,
            "previous-candidate" => Self::PreviousCandidate,
            "previous" => Self::Previous,
            "start-preedit" => Self::StartPreedit,
            "special-midasi" => Self::SpecialMidasi,
            "abbrev" => Self::Abbrev,
            "purge-candidate" => Self::PurgeCandidate,
            "set-input-mode-hiragana" => Self::SetInputMode(InputMode::Hiragana),
            "set-input-mode-katakana" => Self::SetInputMode(InputMode::Katakana),
            "set-input-mode-hankaku-katakana" => Self::SetInputMode(InputMode::HankakuKatakana),
            "set-input-mode-latin" => Self::SetInputMode(InputMode::Latin),
            "set-input-mode-wide-latin" => Self::SetInputMode(InputMode::WideLatin),
            _ => Self::Upper(value.strip_prefix("upper-")?.to_owned()),
        };
        match &command {
            Self::Upper(text) if text.is_empty() => None,
            _ => Some(command),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_known_and_upper_commands() {
        assert_eq!(Command::parse("commit"), Some(Command::Commit));
        assert_eq!(
            Command::parse("set-input-mode-latin"),
            Some(Command::SetInputMode(InputMode::Latin))
        );
        assert_eq!(
            Command::parse("upper-k"),
            Some(Command::Upper("k".to_owned()))
        );
    }

    #[test]
    fn ignores_unknown_and_empty_upper_commands() {
        assert_eq!(Command::parse("unknown"), None);
        assert_eq!(Command::parse("upper-"), None);
    }
}
