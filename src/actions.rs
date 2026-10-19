//! Execute only explicitly supported local actions; never interpret STT as code.
use crate::commands::Command;
use std::{error::Error, os::windows::ffi::OsStrExt, path::Path};
use windows_sys::Win32::UI::{Shell::ShellExecuteW, WindowsAndMessaging::SW_SHOWNORMAL};

pub fn execute(command: Command) -> Result<(), Box<dyn Error>> {
    match command {
        Command::LaunchSteam => open_shortcut(Path::new("actions/steam.lnk")),
    }
}

fn open_shortcut(path: &Path) -> Result<(), Box<dyn Error>> {
    if !path.is_file() {
        return Err(format!("Action shortcut does not exist: '{}'", path.display()).into());
    }
    let absolute = std::env::current_dir()?.join(path);
    let filename: Vec<u16> = absolute.as_os_str().encode_wide().chain(Some(0)).collect();
    let operation: Vec<u16> = "open\0".encode_utf16().collect();
    // SAFETY: strings are NUL-terminated and live throughout the call. No parent
    // window, parameters, or working-directory override is supplied.
    let result = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            operation.as_ptr(),
            filename.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
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

    #[test]
    fn missing_shortcut_returns_an_error() {
        assert!(open_shortcut(Path::new("actions/nonexistent-shortcut.lnk")).is_err());
    }

    #[test]
    #[ignore = "opens the user's Steam app through the existing actions/steam.lnk"]
    fn bundled_steam_shortcut_launches() -> Result<(), Box<dyn Error>> {
        execute(Command::LaunchSteam)
    }
}
