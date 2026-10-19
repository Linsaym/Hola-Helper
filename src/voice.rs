//! Offline Windows WAV playback, called only from the processing thread.
use std::{
    error::Error,
    fs,
    os::windows::ffi::OsStrExt,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    thread,
    time::Duration,
};
use windows_sys::Win32::{
    Media::Multimedia::{mciGetErrorStringW, mciSendStringW},
    Security::Cryptography::{BCRYPT_USE_SYSTEM_PREFERRED_RNG, BCryptGenRandom},
};

pub fn reply_or_warn(stopping: &AtomicBool) {
    if let Err(error) = play_random(Path::new("assets/voice/wake"), stopping) {
        eprintln!("[voice] Cannot play wake response: {error}");
    }
}

pub fn failure_or_warn(stopping: &AtomicBool) {
    if let Err(error) = play_random(Path::new("assets/voice/fail"), stopping) {
        eprintln!("[voice] Cannot play failure response: {error}");
    }
}

pub fn success_or_warn(action: &str, stopping: &AtomicBool) {
    if let Err(error) = play_success(Path::new("assets/voice/success"), action, stopping) {
        eprintln!("[voice] Cannot play success response for {action:?}: {error}");
    }
}

fn wav_files(directory: &Path) -> Result<Vec<PathBuf>, Box<dyn Error>> {
    let mut files = Vec::new();
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        if entry.file_type()?.is_file()
            && entry
                .path()
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("wav"))
        {
            files.push(entry.path());
        }
    }
    Ok(files)
}

fn play_random(directory: &Path, stopping: &AtomicBool) -> Result<(), Box<dyn Error>> {
    if stopping.load(Ordering::Relaxed) {
        return Ok(());
    }
    let files = wav_files(directory)?;
    if files.is_empty() {
        return Err(format!("No WAV files in '{}'", directory.display()).into());
    }
    play_random_files(&files, stopping)
}

fn success_files(directory: &Path, action: &str) -> Result<Vec<PathBuf>, Box<dyn Error>> {
    let files = match wav_files(directory) {
        Ok(files) => files,
        Err(error)
            if error
                .downcast_ref::<std::io::Error>()
                .is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound) =>
        {
            return Ok(Vec::new());
        }
        Err(error) => return Err(error),
    };
    Ok(select_success_files(files, action))
}

fn select_success_files(files: Vec<PathBuf>, action: &str) -> Vec<PathBuf> {
    let action = action.to_lowercase();
    let mut personal = Vec::new();
    let mut general = Vec::new();
    for path in files {
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        let stem = stem.to_lowercase();
        if is_numbered_response(&stem, &action) {
            personal.push(path);
        } else if is_numbered_response(&stem, "random") {
            general.push(path);
        }
    }
    if personal.is_empty() {
        general
    } else {
        personal
    }
}

fn is_numbered_response(stem: &str, prefix: &str) -> bool {
    stem.strip_prefix(prefix)
        .is_some_and(|suffix| !suffix.is_empty() && suffix.bytes().all(|b| b.is_ascii_digit()))
}

fn play_success(
    directory: &Path,
    action: &str,
    stopping: &AtomicBool,
) -> Result<(), Box<dyn Error>> {
    if stopping.load(Ordering::Relaxed) {
        return Ok(());
    }
    play_random_files(&success_files(directory, action)?, stopping)
}

fn play_random_files(files: &[PathBuf], stopping: &AtomicBool) -> Result<(), Box<dyn Error>> {
    if files.is_empty() || stopping.load(Ordering::Relaxed) {
        return Ok(());
    }
    let mut random = [0_u8; 8];
    // SAFETY: valid writable buffer; null algorithm selects the system RNG.
    let status = unsafe {
        BCryptGenRandom(
            std::ptr::null_mut(),
            random.as_mut_ptr(),
            8,
            BCRYPT_USE_SYSTEM_PREFERRED_RNG,
        )
    };
    if status < 0 {
        return Err(format!("Windows RNG failed: {status:#x}").into());
    }
    let index = (u64::from_ne_bytes(random) % files.len() as u64) as usize;
    let path = fs::canonicalize(&files[index])?;
    // MCI takes a quoted filename; Windows filenames cannot contain quotes.
    let mut path: Vec<u16> = path.as_os_str().encode_wide().collect();
    // canonicalize returns an extended-length path; legacy MCI expects the
    // conventional drive/UNC spelling. Preserve Unicode without lossy conversion.
    let prefix: Vec<u16> = "\\\\?\\UNC\\".encode_utf16().collect();
    if path.starts_with(&prefix) {
        path.splice(..prefix.len(), "\\\\".encode_utf16());
    } else if path.starts_with(&"\\\\?\\".encode_utf16().collect::<Vec<_>>()) {
        path.drain(..4);
    }
    let mut open: Vec<u16> = "open \"".encode_utf16().collect();
    open.extend(path);
    open.extend("\" type waveaudio alias hola_wake_reply".encode_utf16());
    command(&open)?;
    let _device = Playback;
    command_str("play hola_wake_reply")?;
    while !stopping.load(Ordering::Relaxed) {
        match command_str("status hola_wake_reply mode")?.as_str() {
            "stopped" => break,
            "playing" | "seeking" => thread::sleep(Duration::from_millis(20)),
            mode => return Err(format!("Unexpected playback mode: {mode}").into()),
        }
    }
    Ok(())
}

struct Playback;

impl Drop for Playback {
    fn drop(&mut self) {
        // Closing also stops playback, including on Ctrl+C and error paths.
        if let Err(error) = command_str("close hola_wake_reply") {
            eprintln!("[voice] Cannot close playback device: {error}");
        }
    }
}

fn command_str(text: &str) -> Result<String, Box<dyn Error>> {
    command(&text.encode_utf16().collect::<Vec<_>>())
}

fn command(text: &[u16]) -> Result<String, Box<dyn Error>> {
    let mut terminated = text.to_vec();
    terminated.push(0);
    let mut output = [0_u16; 256];
    // SAFETY: command is NUL-terminated, output is a writable 256-wide-char buffer;
    // no notification/window callback is requested.
    let status = unsafe {
        mciSendStringW(
            terminated.as_ptr(),
            output.as_mut_ptr(),
            output.len() as u32,
            std::ptr::null_mut(),
        )
    };
    if status != 0 {
        // SAFETY: output remains a valid writable buffer of the given length.
        unsafe {
            mciGetErrorStringW(status, output.as_mut_ptr(), output.len() as u32);
        }
        let end = output.iter().position(|&c| c == 0).unwrap_or(output.len());
        return Err(format!(
            "MCI error {status}: {}",
            String::from_utf16_lossy(&output[..end])
        )
        .into());
    }
    let end = output.iter().position(|&c| c == 0).unwrap_or(output.len());
    Ok(String::from_utf16_lossy(&output[..end]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_responses_are_readable_wavs() -> Result<(), Box<dyn Error>> {
        for path in wav_files(Path::new("assets/voice/wake"))? {
            let mut wav = hound::WavReader::open(path)?;
            assert!(wav.duration() > 0);
            let _: Vec<i32> = wav.samples().collect::<Result<_, _>>()?;
        }
        Ok(())
    }

    #[test]
    fn missing_response_directory_returns_an_error() {
        assert!(wav_files(Path::new("assets/voice/nonexistent-directory")).is_err());
    }

    #[test]
    fn personal_responses_take_priority_case_insensitively_with_any_number() {
        let files = [
            "work1.wav",
            "WORK2.WAV",
            "work999999999999999999999.wav",
            "random1.wav",
            "workshop1.wav",
            "work.wav",
            "work-not-numbered.wav",
        ]
        .map(PathBuf::from)
        .to_vec();
        assert_eq!(
            select_success_files(files, "WoRk"),
            ["work1.wav", "WORK2.WAV", "work999999999999999999999.wav"]
                .map(PathBuf::from)
                .to_vec()
        );
    }

    #[test]
    fn general_responses_are_used_only_when_personal_responses_are_absent() {
        let files = [
            "other1.wav",
            "Random1.wav",
            "RANDOM42.WAV",
            "random.wav",
            "randomized1.wav",
        ]
        .map(PathBuf::from)
        .to_vec();
        assert_eq!(
            select_success_files(files, "editor"),
            ["Random1.wav", "RANDOM42.WAV"].map(PathBuf::from).to_vec()
        );
        assert!(select_success_files(vec![PathBuf::from("other1.wav")], "editor").is_empty());
        assert!(select_success_files(Vec::new(), "editor").is_empty());
    }

    #[test]
    fn missing_success_directory_is_silent_and_success_names_support_unicode()
    -> Result<(), Box<dyn Error>> {
        play_success(
            Path::new("assets/voice/nonexistent-directory"),
            "editor",
            &AtomicBool::new(false),
        )?;
        let files = ["РАБОТА1.wav", "работа2.wav", "random1.wav"]
            .map(PathBuf::from)
            .to_vec();
        assert_eq!(select_success_files(files, "Работа").len(), 2);
        Ok(())
    }

    #[test]
    #[ignore = "plays a success WAV for the first configured action on the default Windows output device"]
    fn configured_success_response_plays() -> Result<(), Box<dyn Error>> {
        let config = crate::config::CommandConfig::load(Path::new("config/commands.json"))?;
        let action = &config
            .commands
            .first()
            .ok_or("No configured commands")?
            .action;
        let files = success_files(Path::new("assets/voice/success"), action)?;
        assert!(
            !files.is_empty(),
            "No success WAVs available for playback test"
        );
        play_random_files(&files, &AtomicBool::new(false))
    }

    #[test]
    #[ignore = "plays a random local failure WAV on the default Windows output device"]
    fn bundled_failure_response_plays() -> Result<(), Box<dyn Error>> {
        play_random(Path::new("assets/voice/fail"), &AtomicBool::new(false))
    }
}
