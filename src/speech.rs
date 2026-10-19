//! Reusable offline Whisper model, owned by a dedicated inference worker.
use std::{
    error::Error,
    ffi::c_void,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender, TryRecvError},
    },
    thread::{self, JoinHandle},
};
use whisper_rs::{
    FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters, WhisperState,
};

#[derive(Debug, PartialEq, Eq)]
pub enum Transcription {
    NoSpeech,
    Unrecognized,
    Text(String),
}

pub struct SpeechWorker {
    requests: Option<SyncSender<Vec<f32>>>,
    results: Receiver<Result<Transcription, String>>,
    stopping: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl SpeechWorker {
    pub fn new(model: &Path, stopping: Arc<AtomicBool>) -> Result<Self, Box<dyn Error>> {
        if !model.is_file() {
            return Err(format!(
                "Whisper model not found: '{}'. Install multilingual ggml-base.bin; see README.md.",
                model.display()
            )
            .into());
        }
        eprintln!("[speech] Loading Whisper model once: {}", model.display());
        let context = WhisperContext::new_with_params(
            model,
            WhisperContextParameters {
                use_gpu: false,
                ..Default::default()
            },
        )
        .map_err(|e| format!("Cannot load Whisper model '{}': {e}", model.display()))?;
        let mut state = context
            .create_state()
            .map_err(|e| format!("Cannot create Whisper state: {e}"))?;
        let (requests, input) = mpsc::sync_channel::<Vec<f32>>(1);
        let (output, results) = mpsc::sync_channel(1);
        let signal = Arc::clone(&stopping);
        let handle = thread::Builder::new()
            .name("speech-recognition".into())
            .spawn(move || {
                // WhisperState retains the model context; both are reused for every command.
                while let Ok(audio) = input.recv() {
                    if signal.load(Ordering::Relaxed) {
                        break;
                    }
                    let result = transcribe(&mut state, &audio, &signal).map_err(|e| e.to_string());
                    if output.try_send(result).is_err() {
                        break;
                    }
                }
            })?;
        eprintln!("[speech] Whisper ready (CPU, language=ru).");
        Ok(Self {
            requests: Some(requests),
            results,
            stopping,
            handle: Some(handle),
        })
    }

    pub fn submit(&self, audio: Vec<f32>) -> Result<(), Box<dyn Error>> {
        self.requests
            .as_ref()
            .ok_or("Speech worker is closed")?
            .try_send(audio)
            .map_err(|e| format!("Cannot submit command audio: {e}").into())
    }

    pub fn try_result(&self) -> Result<Option<Result<Transcription, String>>, Box<dyn Error>> {
        match self.results.try_recv() {
            Ok(result) => Ok(Some(result)),
            Err(TryRecvError::Empty) => Ok(None),
            Err(TryRecvError::Disconnected) => Err("Speech worker disconnected".into()),
        }
    }
}

impl Drop for SpeechWorker {
    fn drop(&mut self) {
        self.stopping.store(true, Ordering::Relaxed);
        self.requests.take();
        if let Some(handle) = self.handle.take()
            && handle.join().is_err()
        {
            eprintln!("[speech] Recognition worker panicked during shutdown");
        }
    }
}

fn transcribe(
    state: &mut WhisperState,
    audio: &[f32],
    stopping: &AtomicBool,
) -> Result<Transcription, Box<dyn Error>> {
    if stopping.load(Ordering::Relaxed) || !crate::command_audio::contains_speech(audio) {
        return Ok(Transcription::NoSpeech);
    }
    let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
    params.set_language(Some("ru"));
    params.set_translate(false);
    params.set_no_context(true);
    params.set_no_timestamps(true);
    params.set_print_special(false);
    params.set_print_progress(false);
    params.set_print_realtime(false);
    params.set_print_timestamps(false);
    params.set_suppress_nst(true);
    params.set_temperature(0.0);
    params.set_temperature_inc(0.0);
    let threads = thread::available_parallelism().map_or(1, |n| n.get().min(4));
    params.set_n_threads(threads as i32);
    // SAFETY: stopping is borrowed for the entire synchronous full() call. The
    // callback only reads its atomic value and never accesses Whisper's state.
    unsafe {
        params.set_abort_callback(Some(abort_when_stopping));
        params.set_abort_callback_user_data(std::ptr::from_ref(stopping).cast_mut().cast());
    }
    state.full(params, audio)?;
    let mut text = String::new();
    for segment in state.as_iter() {
        // Reject the entire utterance if any segment is classified as no speech.
        if segment.no_speech_probability() >= 0.6 {
            return Ok(Transcription::NoSpeech);
        }
        text.push_str(segment.to_str()?);
        text.push(' ');
    }
    let text = text.trim();
    Ok(if text.is_empty() {
        Transcription::Unrecognized
    } else {
        Transcription::Text(text.to_owned())
    })
}

unsafe extern "C" fn abort_when_stopping(data: *mut c_void) -> bool {
    // SAFETY: transcribe supplies a valid AtomicBool for the duration of full().
    unsafe { &*data.cast::<AtomicBool>() }.load(Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn missing_model_returns_an_error_before_starting_worker() {
        assert!(
            SpeechWorker::new(
                Path::new("models/whisper/nonexistent-model.bin"),
                Arc::new(AtomicBool::new(false)),
            )
            .is_err()
        );
    }

    #[test]
    #[ignore = "requires downloaded models/whisper/ggml-base.bin; performs CPU inference"]
    fn silence_does_not_become_a_command_when_model_is_reused() -> Result<(), Box<dyn Error>> {
        let worker = SpeechWorker::new(
            Path::new("models/whisper/ggml-base.bin"),
            Arc::new(AtomicBool::new(false)),
        )?;
        for _ in 0..2 {
            worker.submit(vec![0.0; 2 * crate::command_audio::SAMPLE_RATE])?;
            let result = worker.results.recv_timeout(Duration::from_secs(60))??;
            assert_eq!(result, Transcription::NoSpeech);
        }
        Ok(())
    }

    #[test]
    #[ignore = "requires Base model and HOLA_TEST_COMMAND_WAV recording the first configured command in 16-kHz mono PCM16"]
    fn recognizes_recorded_command_then_silence() -> Result<(), Box<dyn Error>> {
        let path = std::env::var_os("HOLA_TEST_COMMAND_WAV")
            .ok_or("Set HOLA_TEST_COMMAND_WAV to a recording of the first configured command")?;
        let config = crate::config::CommandConfig::load(Path::new("config/commands.json"))?;
        let expected_action = config
            .commands
            .first()
            .ok_or("No configured commands")?
            .action
            .clone();
        let recognizer = crate::commands::CommandRecognizer::new(config)?;
        let mut wav = crate::wav::open(Path::new(&path))?;
        let audio: Vec<f32> = wav
            .samples::<i16>()
            .map(|s| s.map(|s| f32::from(s) / 32768.0))
            .collect::<Result<_, _>>()?;
        let worker = SpeechWorker::new(
            Path::new("models/whisper/ggml-base.bin"),
            Arc::new(AtomicBool::new(false)),
        )?;
        worker.submit(audio)?;
        let Transcription::Text(text) = worker.results.recv_timeout(Duration::from_secs(60))??
        else {
            return Err("The command recording did not produce speech text".into());
        };
        eprintln!("Test transcription: {text}");
        assert_eq!(
            recognizer
                .recognize(&text)
                .command
                .map(|c| c.action.as_str()),
            Some(expected_action.as_str())
        );
        worker.submit(vec![0.0; 2 * crate::command_audio::SAMPLE_RATE])?;
        let result = worker.results.recv_timeout(Duration::from_secs(60))??;
        assert_eq!(result, Transcription::NoSpeech);
        Ok(())
    }
}
