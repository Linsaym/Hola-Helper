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
    if files.is_empty() {
        return Err(format!("No WAV files in '{}'", directory.display()).into());
    }
    Ok(files)
}

fn play_random(directory: &Path, stopping: &AtomicBool) -> Result<(), Box<dyn Error>> {
    if stopping.load(Ordering::Relaxed) {
        return Ok(());
    }
    let files = wav_files(directory)?;
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
    #[ignore = "plays a random local failure WAV on the default Windows output device"]
    fn bundled_failure_response_plays() -> Result<(), Box<dyn Error>> {
        play_random(Path::new("assets/voice/fail"), &AtomicBool::new(false))
    }
}
