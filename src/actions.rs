//! Resolve configured local actions; never interpret recognized text as code.
use crate::config::validate_action_name;
use std::{
    error::Error,
    fs,
    os::windows::ffi::OsStrExt,
    path::{Path, PathBuf},
};
use windows_sys::Win32::UI::{Shell::ShellExecuteW, WindowsAndMessaging::SW_SHOWNORMAL};

pub fn execute(action: &str) -> Result<(), Box<dyn Error>> {
    let path = resolve(Path::new("actions"), action)?;
    open(&path)
}

fn resolve(directory: &Path, action: &str) -> Result<PathBuf, Box<dyn Error>> {
    validate_action_name(action)?;
    let root = fs::canonicalize(directory).map_err(|e| {
        format!(
            "Cannot open actions directory '{}': {e}",
            directory.display()
        )
    })?;
    let mut matches = Vec::new();
    let action_key = action.to_lowercase();
    for entry in fs::read_dir(&root)? {
        let entry = entry?;
        let path = directory.join(entry.file_name());
        if !path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("lnk") || e.eq_ignore_ascii_case("bat"))
            || !path
                .file_stem()
                .and_then(|s| s.to_str())
                .is_some_and(|s| s.to_lowercase() == action_key)
        {
            continue;
        }
        if fs::metadata(&path)?.is_file() {
            matches.push(path);
        }
    }
    if matches.is_empty() {
        return Err(format!(
            "No .lnk or .bat action named {action:?} in '{}'",
            directory.display()
        )
        .into());
    }
    if matches.len() > 1 {
        return Err(
            format!("Ambiguous action {action:?}: multiple files match: {matches:?}").into(),
        );
    }
    let path = matches.remove(0);
    checked_path(&root, &path)
}

fn checked_path(root: &Path, path: &Path) -> Result<PathBuf, Box<dyn Error>> {
    let resolved = fs::canonicalize(&path)?;
    if !resolved.starts_with(root) {
        return Err(format!(
            "Action '{}' resolves outside actions directory",
            path.display()
        )
        .into());
    }
    // Keep a conventional absolute path for Windows Shell; canonicalize may
    // introduce the extended-length \\?\ prefix unsupported by some handlers.
    Ok(std::path::absolute(path)?)
}

fn open(path: &Path) -> Result<(), Box<dyn Error>> {
    let filename: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let directory: Vec<u16> = path
        .parent()
        .ok_or("Action has no parent directory")?
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    let operation: Vec<u16> = "open\0".encode_utf16().collect();
    // SAFETY: strings are NUL-terminated and live throughout the call. No parent
    // window or parameters are supplied. The script's directory is its working directory.
    let result = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            operation.as_ptr(),
            filename.as_ptr(),
            std::ptr::null(),
            directory.as_ptr(),
            SW_SHOWNORMAL,
        )
    } as isize;
    if result <= 32 {
        return Err(format!(
            "Cannot open action '{}': ShellExecuteW error {result}",
            path.display()
        )
        .into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path =
                std::env::temp_dir().join(format!("hola-actions-{}-{nonce}", std::process::id()));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            // Only remove this fixture's immediate files/empty directories; never recurse.
            if let Ok(entries) = fs::read_dir(&self.0) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if entry
                        .file_type()
                        .is_ok_and(|t| t.is_dir() && !t.is_symlink())
                    {
                        let _ = fs::remove_dir(path);
                    } else {
                        let _ = fs::remove_file(path);
                    }
                }
            }
            let _ = fs::remove_dir(&self.0);
        }
    }

    #[test]
    fn finds_supported_extensions_and_unicode_names_case_insensitively()
    -> Result<(), Box<dyn Error>> {
        let directory = TestDirectory::new();
        fs::write(directory.0.join("Editor.LNK"), [])?;
        fs::write(directory.0.join("ИГРА.BaT"), [])?;
        assert!(resolve(&directory.0, "EDITOR")?.ends_with("Editor.LNK"));
        assert!(resolve(&directory.0, "игра")?.ends_with("ИГРА.BaT"));
        Ok(())
    }

    #[test]
    fn rejects_missing_actions_unsupported_extensions_and_directories() -> Result<(), Box<dyn Error>>
    {
        let directory = TestDirectory::new();
        fs::write(directory.0.join("game.exe"), [])?;
        fs::create_dir(directory.0.join("game.bat"))?;
        assert!(resolve(&directory.0, "missing").is_err());
        assert!(resolve(&directory.0, "game").is_err());
        Ok(())
    }

    #[test]
    fn duplicate_stems_are_ambiguous_even_across_extensions_and_case() -> Result<(), Box<dyn Error>>
    {
        let directory = TestDirectory::new();
        fs::write(directory.0.join("Game.lnk"), [])?;
        fs::write(directory.0.join("GAME.BAT"), [])?;
        let error = resolve(&directory.0, "game").unwrap_err().to_string();
        assert!(error.contains("Ambiguous action"), "{error}");
        Ok(())
    }

    #[test]
    fn refuses_paths_before_resolving_or_launching() {
        for action in [
            "../game",
            "..\\game",
            "C:\\Windows\\game",
            "C:game",
            "\\\\server\\game",
            "game:stream",
        ] {
            assert!(execute(action).is_err(), "{action}");
        }
    }

    #[test]
    fn canonical_boundary_check_accepts_inside_and_rejects_outside() -> Result<(), Box<dyn Error>> {
        let directory = TestDirectory::new();
        let outside = TestDirectory::new();
        let inside_file = directory.0.join("inside.bat");
        let outside_file = outside.0.join("outside.bat");
        fs::write(&inside_file, [])?;
        fs::write(&outside_file, [])?;
        let root = fs::canonicalize(&directory.0)?;
        assert!(checked_path(&root, &inside_file).is_ok());
        let error = checked_path(&root, &outside_file).unwrap_err().to_string();
        assert!(error.contains("outside actions directory"), "{error}");
        Ok(())
    }

    #[test]
    #[ignore = "requires Windows symbolic-link privileges; does not launch any process"]
    fn refuses_a_file_link_that_resolves_outside_actions() -> Result<(), Box<dyn Error>> {
        let directory = TestDirectory::new();
        let outside = TestDirectory::new();
        let target = outside.0.join("target.bat");
        fs::write(&target, [])?;
        std::os::windows::fs::symlink_file(&target, directory.0.join("game.bat"))?;
        let error = resolve(&directory.0, "game").unwrap_err().to_string();
        assert!(error.contains("outside actions directory"), "{error}");
        Ok(())
    }

    #[test]
    #[ignore = "runs a temporary batch action that only writes a marker in its own directory"]
    fn batch_action_runs_in_its_own_directory() -> Result<(), Box<dyn Error>> {
        let directory = TestDirectory::new();
        fs::write(
            directory.0.join("Script.BAT"),
            b"@echo off\r\n>hola-batch-result.txt echo ok\r\n",
        )?;
        let path = resolve(&directory.0, "script")?;
        open(&path)?;
        let marker = directory.0.join("hola-batch-result.txt");
        for _ in 0..250 {
            if fs::read_to_string(&marker).is_ok_and(|text| text.trim() == "ok") {
                return Ok(());
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        Err("Batch action did not write its marker in the actions directory".into())
    }

    #[test]
    #[ignore = "launches the first action from the user's config/commands.json"]
    fn configured_action_launches() -> Result<(), Box<dyn Error>> {
        let config = crate::config::CommandConfig::load(Path::new("config/commands.json"))?;
        let phrase = config
            .commands
            .first()
            .ok_or("No configured commands")?
            .phrase
            .clone();
        let recognizer = crate::commands::CommandRecognizer::new(config)?;
        let recognition = recognizer.recognize(&phrase);
        let command = recognition
            .command
            .ok_or("Configured phrase did not match")?;
        execute(&command.action)
    }
}
