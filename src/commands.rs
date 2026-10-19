//! Command recognition, independent of speech decoding and execution.
use crate::config::{Command, CommandConfig};
use std::{collections::HashSet, error::Error};

pub struct CommandRecognizer {
    max_distance: usize,
    commands: Vec<(String, Command)>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Recognition<'a> {
    pub command: Option<&'a Command>,
    /// Minimum distance, including rejected/ambiguous matches; None for no text/candidates.
    pub distance: Option<usize>,
    pub ambiguous: bool,
}

pub fn normalize(text: &str) -> String {
    let normalized: String = text
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .collect();
    normalized.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn comparison_text(text: &str) -> String {
    normalize(text)
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect()
}

impl CommandRecognizer {
    pub fn new(config: CommandConfig) -> Result<Self, Box<dyn Error>> {
        let mut seen = HashSet::new();
        let mut commands = Vec::with_capacity(config.commands.len());
        for command in config.commands {
            let phrase = comparison_text(&command.phrase);
            if phrase.is_empty() || !seen.insert(phrase.clone()) {
                return Err(format!(
                    "Empty or duplicate normalized command phrase: {:?}",
                    command.phrase
                )
                .into());
            }
            commands.push((phrase, command));
        }
        eprintln!(
            "[commands] Loaded {} commands (max_distance={})",
            commands.len(),
            config.max_distance
        );
        Ok(Self {
            max_distance: config.max_distance,
            commands,
        })
    }

    pub fn recognize(&self, text: &str) -> Recognition<'_> {
        let text = comparison_text(text);
        let mut best = Recognition {
            command: None,
            distance: None,
            ambiguous: false,
        };
        if text.is_empty() {
            return best;
        }
        for (phrase, command) in &self.commands {
            let distance = levenshtein(&text, phrase);
            match best.distance {
                None => {
                    best = Recognition {
                        command: Some(command),
                        distance: Some(distance),
                        ambiguous: false,
                    };
                }
                Some(previous) if distance < previous => {
                    best = Recognition {
                        command: Some(command),
                        distance: Some(distance),
                        ambiguous: false,
                    };
                }
                Some(previous) if distance == previous => best.ambiguous = true,
                _ => {}
            }
        }
        if best.ambiguous || best.distance.is_some_and(|d| d > self.max_distance) {
            best.command = None;
        }
        best
    }
}

/// Unicode character distance with one reusable row, without extra dependencies.
fn levenshtein(left: &str, right: &str) -> usize {
    let right: Vec<char> = right.chars().collect();
    let mut row: Vec<usize> = (0..=right.len()).collect();
    for (i, a) in left.chars().enumerate() {
        let mut diagonal = row[0];
        row[0] = i + 1;
        for (j, &b) in right.iter().enumerate() {
            let above = row[j + 1];
            row[j + 1] = (above + 1)
                .min(row[j] + 1)
                .min(diagonal + usize::from(a != b));
            diagonal = above;
        }
    }
    row[right.len()]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        time::{SystemTime, UNIX_EPOCH},
    };

    fn recognizer(max_distance: usize, entries: &[(&str, &str)]) -> CommandRecognizer {
        let json = serde_json::json!({
            "max_distance": max_distance,
            "commands": entries.iter().map(|(phrase, action)| serde_json::json!({
                "phrase": phrase, "action": action
            })).collect::<Vec<_>>()
        });
        CommandRecognizer::new(CommandConfig::from_json(&json.to_string()).unwrap()).unwrap()
    }

    #[test]
    fn normalization_ignores_case_punctuation_and_word_boundaries() {
        let recognizer = recognizer(6, &[("давай поиграем", "game")]);
        for text in [
            "Давай поиграем!",
            " ДАВАЙ\tПОИГРАЕМ! ",
            "«Давай\u{a0}поиграем». ",
            "давайпоиграем",
            "да вай по игра ем",
        ] {
            let result = recognizer.recognize(text);
            assert_eq!(result.command.unwrap().action, "game");
            assert_eq!(result.distance, Some(0));
            assert!(!result.ambiguous);
        }
    }

    #[test]
    fn distance_counts_unicode_characters_insertions_deletions_and_substitutions() {
        for (left, right, expected) in [
            ("", "", 0),
            ("", "кот", 3),
            ("кот", "", 3),
            ("кот", "кит", 1),
            ("кот", "крот", 1),
            ("kitten", "sitting", 3),
        ] {
            assert_eq!(levenshtein(left, right), expected);
            assert_eq!(levenshtein(right, left), expected);
        }
    }

    #[test]
    fn applies_the_configured_threshold_including_zero() {
        let recognizer = recognizer(6, &[("давай поиграем", "game")]);
        let accepted = recognizer.recognize("давай поиграем абвгде");
        assert_eq!(accepted.distance, Some(6));
        assert_eq!(accepted.command.unwrap().action, "game");
        let rejected = recognizer.recognize("давай поиграем абвгдеж");
        assert_eq!(rejected.distance, Some(7));
        assert!(rejected.command.is_none());
        let exact_only = self::recognizer(0, &[("открой редактор", "editor")]);
        assert!(exact_only.recognize("открой редактор!").command.is_some());
        assert!(exact_only.recognize("открой редактар").command.is_none());
    }

    #[test]
    fn uses_only_distance_without_word_or_negation_rules() {
        let recognizer = recognizer(6, &[("давай поиграем", "game")]);
        for (text, distance) in [
            ("даваи поиграем", 1),
            ("давай проиграем", 1),
            ("не давай поиграем", 2),
        ] {
            let result = recognizer.recognize(text);
            assert!(result.command.is_some());
            assert_eq!(result.distance, Some(distance));
        }
        assert!(recognizer.recognize("... \t").command.is_none());
        assert_eq!(recognizer.recognize("... \t").distance, None);
    }

    #[test]
    fn rejects_equal_minima_between_distinct_actions_and_accepts_a_better_match() {
        let recognizer = recognizer(
            6,
            &[("давай поиграем", "first"), ("давай поиграим", "second")],
        );
        let ambiguous = recognizer.recognize("давай поиграум");
        assert!(ambiguous.command.is_none());
        assert!(ambiguous.ambiguous);
        assert_eq!(ambiguous.distance, Some(1));
        assert_eq!(
            recognizer
                .recognize("давай поиграим")
                .command
                .unwrap()
                .action,
            "second"
        );
        let better = self::recognizer(
            6,
            &[
                ("давай поиграем", "first"),
                ("давай поиграим", "second"),
                ("давай поиграум", "third"),
            ],
        );
        let selected = better.recognize("давай поиграум");
        assert_eq!(selected.command.unwrap().action, "third");
        assert!(!selected.ambiguous);
        assert!(
            self::recognizer(6, &[])
                .recognize("anything")
                .command
                .is_none()
        );
    }

    #[test]
    fn duplicate_normalized_phrases_are_configuration_errors() {
        let config = CommandConfig::from_json(r#"{"max_distance":6,"commands":[{"phrase":"Открой редактор!","action":"one"},{"phrase":"откройредактор","action":"two"}]}"#).unwrap();
        assert!(CommandRecognizer::new(config).is_err());
    }

    #[test]
    fn config_is_kept_in_memory_and_changes_apply_on_next_load_without_recompilation()
    -> Result<(), Box<dyn Error>> {
        let nonce = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        let path = std::env::temp_dir().join(format!(
            "hola-command-config-{}-{nonce}.json",
            std::process::id()
        ));
        fs::write(
            &path,
            r#"{"max_distance":0,"commands":[{"phrase":"открой редактор","action":"editor"}]}"#,
        )?;
        let original = CommandRecognizer::new(CommandConfig::load(&path)?)?;
        fs::write(
            &path,
            r#"{"max_distance":0,"commands":[{"phrase":"открой окно","action":"window"}]}"#,
        )?;
        let reloaded = CommandRecognizer::new(CommandConfig::load(&path)?)?;
        fs::remove_file(&path)?;
        assert_eq!(
            original
                .recognize("открой редактор")
                .command
                .unwrap()
                .action,
            "editor"
        );
        assert!(original.recognize("открой окно").command.is_none());
        assert_eq!(
            reloaded.recognize("открой окно").command.unwrap().action,
            "window"
        );
        assert!(reloaded.recognize("открой редактор").command.is_none());
        Ok(())
    }
}
