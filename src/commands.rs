//! Exact command recognition, independent of speech decoding and execution.
#[derive(Debug, PartialEq, Eq)]
pub enum Command {
    LaunchSteam,
}

pub fn recognize(text: &str) -> Option<Command> {
    let normalized: String = text
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .collect();
    let normalized = normalized.split_whitespace().collect::<Vec<_>>().join(" ");
    match normalized.as_str() {
        "запусти steam" | "запусти стим" => Some(Command::LaunchSteam),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_only_complete_commands_after_normalization() {
        for text in ["Запусти Steam", "  ЗАПУСТИ\tSteam! ", "«Запусти Стим». "]
        {
            assert_eq!(recognize(text), Some(Command::LaunchSteam));
        }
        for text in [
            "",
            "...",
            "Steam",
            "запусти",
            "запусти steem",
            "не запусти Steam",
            "запусти Steam и браузер",
            "запусти Steam & calc.exe",
        ] {
            assert_eq!(recognize(text), None, "{text}");
        }
    }
}
