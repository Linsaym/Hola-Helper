//! Startup configuration shared by recognition and local action execution.
use serde::Deserialize;
use std::{
    error::Error,
    fs,
    path::{Component, Path},
};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommandConfig {
    pub max_distance: usize,
    pub commands: Vec<Command>,
}

#[derive(Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Command {
    pub phrase: String,
    pub action: String,
}

impl CommandConfig {
    pub fn load(path: &Path) -> Result<Self, Box<dyn Error>> {
        let json = fs::read_to_string(path).map_err(|e| {
            format!(
                "Cannot read commands configuration '{}': {e}",
                path.display()
            )
        })?;
        Self::from_json(&json)
            .map_err(|e| format!("Invalid commands configuration '{}': {e}", path.display()).into())
    }

    pub fn from_json(json: &str) -> Result<Self, Box<dyn Error>> {
        let config: Self = serde_json::from_str(json.trim_start_matches('\u{feff}'))?;
        for (index, command) in config.commands.iter().enumerate() {
            if !command.phrase.chars().any(char::is_alphanumeric) {
                return Err(
                    format!("commands[{index}].phrase must contain letters or digits").into(),
                );
            }
            validate_action_name(&command.action)
                .map_err(|e| format!("commands[{index}].action: {e}"))?;
        }
        Ok(config)
    }
}

/// Only a Windows filename stem is accepted, never a path or alternate data stream.
pub fn validate_action_name(name: &str) -> Result<(), String> {
    let mut components = Path::new(name).components();
    if name.is_empty()
        || name.trim() != name
        || name.ends_with('.')
        || name
            .chars()
            .any(|c| c.is_control() || "\\/:<>\"|?*".contains(c))
        || !matches!(components.next(), Some(Component::Normal(_)))
        || components.next().is_some()
    {
        return Err(format!(
            "Expected a filename stem without a path, got {name:?}"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_external_commands_and_configurable_threshold() {
        let config = CommandConfig::from_json(
            r#"{"max_distance":0,"commands":[{"phrase":"открой редактор","action":"Editor"}]}"#,
        )
        .unwrap();
        assert_eq!(config.max_distance, 0);
        assert_eq!(config.commands[0].action, "Editor");
        assert!(CommandConfig::from_json("\u{feff}{\"max_distance\":6,\"commands\":[]}").is_ok());
    }

    #[test]
    fn rejects_invalid_schema_thresholds_and_phrases() {
        for json in [
            "{",
            "{}",
            r#"{"max_distance":-1,"commands":[]}"#,
            r#"{"max_distance":1.5,"commands":[]}"#,
            r#"{"max_distance":6,"commands":[],"typo":1}"#,
            r#"{"max_distance":6,"commands":[{"phrase":"...","action":"game"}]}"#,
            r#"{"max_distance":6,"commands":[{"phrase":"play"}]}"#,
            r#"{"max_distance":6,"commands":[{"phrase":"play","action":"../game"}]}"#,
        ] {
            assert!(CommandConfig::from_json(json).is_err(), "{json}");
        }
        assert!(CommandConfig::load(Path::new("config/nonexistent.json")).is_err());
    }

    #[test]
    fn rejects_paths_traversal_and_windows_filename_aliases() {
        for name in [
            "",
            ".",
            "..",
            "../game",
            "..\\game",
            "/game",
            "C:\\game",
            "C:game",
            "\\\\server\\share\\game",
            "game/sub",
            "game:stream",
            "game.",
            "game ",
            "game\0",
            "*.bat",
        ] {
            assert!(validate_action_name(name).is_err(), "{name:?}");
        }
        for name in ["game", "Editor", "Мой редактор", "my.app"] {
            assert!(validate_action_name(name).is_ok(), "{name:?}");
        }
    }
}
