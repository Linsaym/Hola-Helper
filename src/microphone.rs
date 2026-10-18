use crate::{
    DETECTION_THRESHOLD,
    resample::Resampler,
    wake_word::{FRAME_SAMPLES, WakeWordDetector},
};
use cpal::{
    ErrorKind, FromSample, SampleFormat, SizedSample,
    traits::{DeviceTrait, HostTrait, StreamTrait},
};
use std::{
    error::Error,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc::{self, RecvTimeoutError, SyncSender, TrySendError},
    },
    time::{Duration, Instant},
};

const PACKET_SAMPLES: usize = 512;
type Packet = [f32; PACKET_SAMPLES];

struct CaptureDiagnostics {
    first_samples: AtomicUsize,
    xruns: AtomicUsize,
    playback_active: AtomicBool,
    capture_epoch: AtomicUsize,
}

impl CaptureDiagnostics {
    fn new() -> Self {
        Self {
            first_samples: AtomicUsize::new(usize::MAX),
            xruns: AtomicUsize::new(0),
            playback_active: AtomicBool::new(false),
            capture_epoch: AtomicUsize::new(0),
        }
    }
}

pub fn run() -> Result<(), Box<dyn Error>> {
    let stopping = Arc::new(AtomicBool::new(false));
    let signal = Arc::clone(&stopping);
    ctrlc::set_handler(move || signal.store(true, Ordering::Relaxed))
        .map_err(|e| format!("Cannot install Ctrl+C handler: {e}"))?;
    let device = cpal::default_host()
        .default_input_device()
        .ok_or("No default input microphone is available")?;
    match device.description() {
        Ok(description) => eprintln!("[microphone] Default input device: {}", description.name()),
        Err(error) => eprintln!("[microphone] Cannot read device description: {error}"),
    }
    let supported = device
        .default_input_config()
        .map_err(|e| format!("Cannot get microphone configuration: {e}"))?;
    let format = supported.sample_format();
    eprintln!("[microphone] Default input configuration: {supported:?}");
    let config: cpal::StreamConfig = supported.into();
    eprintln!("[microphone] Requested stream configuration: {config:?}, format={format}");
    if config.channels == 0 || config.sample_rate == 0 {
        return Err("Microphone reported an invalid channel count or sample rate".into());
    }
    let mut detector = WakeWordDetector::new(Path::new("models/openwakeword"))?;
    let (sender, receiver) = mpsc::sync_channel::<Packet>(64);
    let (error_sender, errors) = mpsc::sync_channel(1);
    let overflow = Arc::new(AtomicBool::new(false));
    let diagnostics = Arc::new(CaptureDiagnostics::new());
    let stream = match format {
        SampleFormat::F32 => capture::<f32>(
            &device,
            config.clone(),
            sender,
            error_sender,
            Arc::clone(&overflow),
            Arc::clone(&stopping),
            Arc::clone(&diagnostics),
        ),
        SampleFormat::I16 => capture::<i16>(
            &device,
            config.clone(),
            sender,
            error_sender,
            Arc::clone(&overflow),
            Arc::clone(&stopping),
            Arc::clone(&diagnostics),
        ),
        SampleFormat::U16 => capture::<u16>(
            &device,
            config.clone(),
            sender,
            error_sender,
            Arc::clone(&overflow),
            Arc::clone(&stopping),
            Arc::clone(&diagnostics),
        ),
        other => {
            return Err(format!(
                "Unsupported microphone sample format: {other}; expected f32, i16, or u16"
            )
            .into());
        }
    }
    .map_err(|e| format!("Cannot initialize microphone capture: {e}"))?;
    eprintln!("[microphone] build_input_stream() completed successfully");
    if stopping.load(Ordering::Relaxed) {
        return Ok(());
    }
    stream
        .play()
        .map_err(|e| format!("Cannot start microphone: {e}"))?;
    eprintln!("[microphone] stream.play() returned successfully (WASAPI start is asynchronous)");
    println!("Listening for Hey Jarvis. Press Ctrl+C to stop.");
    // This consumer is the processing worker; CPAL owns the separate capture thread.
    let started = Instant::now();
    let mut resampler = Resampler::new(config.sample_rate);
    let mut resampled = Vec::with_capacity(PACKET_SAMPLES * 2);
    let mut frame = [0_i16; FRAME_SAMPLES];
    let mut filled = 0;
    let mut frame_number = 0_u64;
    let mut last_detection = None;
    let mut first_callback_logged = false;
    let mut xrun_logged = false;
    while !stopping.load(Ordering::Relaxed) {
        let first_samples = diagnostics.first_samples.load(Ordering::Relaxed);
        if !first_callback_logged && first_samples != usize::MAX {
            eprintln!(
                "[microphone] First audio callback invoked: {first_samples} interleaved samples ({} frames, {} channels)",
                first_samples / usize::from(config.channels),
                config.channels
            );
            first_callback_logged = true;
        }
        if !xrun_logged && diagnostics.xruns.load(Ordering::Relaxed) != 0 {
            eprintln!(
                "[microphone] CPAL backend warning (Xrun): audio discontinuity/underrun/overrun; continuing capture. Further Xrun warnings suppressed."
            );
            xrun_logged = true;
        }
        if overflow.load(Ordering::Relaxed) {
            return Err("Application microphone queue overflow (64 packets); capture stopped to avoid processing discontinuous audio. Try a release build (cargo run --release -- --mic).".into());
        }
        if let Ok(error) = errors.try_recv() {
            return Err(format!("CPAL backend stream error ({:?}): {error}", error.kind()).into());
        }
        let packet = match receiver.recv_timeout(Duration::from_millis(50)) {
            Ok(packet) => packet,
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => {
                return Err("Microphone capture disconnected".into());
            }
        };
        resampled.clear();
        resampler.process(&packet, &mut resampled);
        for &sample in &resampled {
            if stopping.load(Ordering::Relaxed) {
                break;
            }
            frame[filled] = (sample.clamp(-1.0, 1.0) * 32768.0)
                .round()
                .clamp(-32768.0, 32767.0) as i16;
            filled += 1;
            if filled == FRAME_SAMPLES {
                let score = detector.process(&frame)?;
                frame_number += 1;
                // 16 frames = 1.28 seconds of audio. Never reset feature history.
                if score >= DETECTION_THRESHOLD
                    && last_detection.is_none_or(|last| frame_number - last >= 16)
                {
                    println!(
                        "Hey Jarvis detected! score={score:.4} timestamp=+{:.3}s",
                        started.elapsed().as_secs_f64()
                    );
                    last_detection = Some(frame_number);
                    diagnostics.playback_active.store(true, Ordering::SeqCst);
                    crate::voice::reply_or_warn(&stopping);
                    // Keep capture muted while clearing every bit of stale
                    // audio, including the remainder of this resampled packet.
                    detector.reset()?;
                    resampler = Resampler::new(config.sample_rate);
                    while receiver.try_recv().is_ok() {}
                    filled = 0;
                    diagnostics.capture_epoch.fetch_add(1, Ordering::SeqCst);
                    diagnostics.playback_active.store(false, Ordering::SeqCst);
                    break;
                }
                filled = 0;
            }
        }
    }
    drop(stream); // Stop callbacks before dropping queued audio and ONNX sessions.
    Ok(())
}

fn capture<T>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    sender: SyncSender<Packet>,
    errors: SyncSender<cpal::Error>,
    overflow: Arc<AtomicBool>,
    stopping: Arc<AtomicBool>,
    diagnostics: Arc<CaptureDiagnostics>,
) -> Result<cpal::Stream, cpal::Error>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    let mut mono = MonoPacket::new(usize::from(config.channels));
    let mut capture_epoch = 0;
    let callback_diagnostics = Arc::clone(&diagnostics);
    device.build_input_stream(
        config,
        move |data: &[T], _| {
            let _ = callback_diagnostics.first_samples.compare_exchange(
                usize::MAX,
                data.len(),
                Ordering::Relaxed,
                Ordering::Relaxed,
            );
            if stopping.load(Ordering::Relaxed) || overflow.load(Ordering::Relaxed) {
                return;
            }
            for &sample in data {
                let epoch = callback_diagnostics.capture_epoch.load(Ordering::SeqCst);
                if epoch != capture_epoch {
                    mono = MonoPacket::new(mono.channels);
                    capture_epoch = epoch;
                }
                if callback_diagnostics.playback_active.load(Ordering::SeqCst) {
                    mono = MonoPacket::new(mono.channels);
                    return;
                }
                if let Some(packet) = mono.push(sample.to_sample::<f32>()) {
                    match sender.try_send(packet) {
                        Ok(()) => {}
                        Err(TrySendError::Full(_)) => {
                            overflow.store(true, Ordering::Relaxed);
                            return;
                        }
                        Err(TrySendError::Disconnected(_)) => return,
                    }
                }
            }
        },
        move |error| {
            report_stream_error(error, &errors, &diagnostics);
        },
        None,
    )
}

fn report_stream_error(
    error: cpal::Error,
    fatal_errors: &SyncSender<cpal::Error>,
    diagnostics: &CaptureDiagnostics,
) {
    if error.kind() == ErrorKind::Xrun {
        // WASAPI reports a discontinuity but continues delivering audio. Do not
        // let recoverable glitches fill the fatal-error channel or stop capture.
        diagnostics.xruns.fetch_add(1, Ordering::Relaxed);
    } else {
        let _ = fatal_errors.try_send(error);
    }
}

// Fixed storage: no allocation, inference, blocking send, or logging in callback.
struct MonoPacket {
    channels: usize,
    channel: usize,
    sum: f32,
    packet: Packet,
    filled: usize,
}

impl MonoPacket {
    fn new(channels: usize) -> Self {
        Self {
            channels,
            channel: 0,
            sum: 0.0,
            packet: [0.0; PACKET_SAMPLES],
            filled: 0,
        }
    }

    fn push(&mut self, sample: f32) -> Option<Packet> {
        self.sum += if sample.is_finite() {
            sample.clamp(-1.0, 1.0)
        } else {
            0.0
        };
        self.channel += 1;
        if self.channel == self.channels {
            self.packet[self.filled] = self.sum / self.channels as f32;
            self.filled += 1;
            self.channel = 0;
            self.sum = 0.0;
            if self.filled == PACKET_SAMPLES {
                self.filled = 0;
                return Some(self.packet);
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cpal::Sample;

    #[test]
    fn xruns_do_not_terminate_capture_or_hide_fatal_backend_errors() {
        let diagnostics = CaptureDiagnostics::new();
        let (sender, receiver) = mpsc::sync_channel(1);
        for _ in 0..10 {
            report_stream_error(ErrorKind::Xrun.into(), &sender, &diagnostics);
        }
        assert_eq!(diagnostics.xruns.load(Ordering::Relaxed), 10);
        assert!(matches!(
            receiver.try_recv(),
            Err(mpsc::TryRecvError::Empty)
        ));
        report_stream_error(ErrorKind::DeviceNotAvailable.into(), &sender, &diagnostics);
        assert_eq!(
            receiver.try_recv().unwrap().kind(),
            ErrorKind::DeviceNotAvailable
        );
    }

    #[test]
    fn interleaved_channels_and_partial_packets_survive_callbacks() {
        let mut mono = MonoPacket::new(2);
        // An odd callback size splits a stereo sample across callbacks.
        let input = [1.0_f32, -0.5].repeat(PACKET_SAMPLES * 2);
        let mut packets = Vec::new();
        for callback in input.chunks(137) {
            for &sample in callback {
                if let Some(packet) = mono.push(sample) {
                    packets.push(packet);
                }
            }
        }
        assert_eq!(packets.len(), 2);
        assert!(packets.iter().flatten().all(|&sample| sample == 0.25));
    }

    #[test]
    fn integer_formats_are_normalized() {
        assert_eq!(i16::MIN.to_sample::<f32>(), -1.0);
        assert_eq!(0_i16.to_sample::<f32>(), 0.0);
        assert_eq!(0_u16.to_sample::<f32>(), -1.0);
        assert_eq!(32768_u16.to_sample::<f32>(), 0.0);
    }

    #[test]
    #[ignore = "requires downloaded models and samples/heyjarvis.wav; simulates capture, not hardware"]
    fn recorded_audio_survives_stereo_capture_and_resampling() -> Result<(), Box<dyn Error>> {
        let mut wav = crate::wav::open(Path::new("samples/heyjarvis.wav"))?;
        let mut audio = vec![0.0_f32; 16_000];
        for sample in wav.samples::<i16>() {
            audio.push(sample?.to_sample::<f32>());
        }
        audio.extend_from_slice(&[0.0; 16_000]);
        for rate in [44_100, 48_000] {
            let mut mono = MonoPacket::new(2);
            let mut resampler = Resampler::new(rate);
            let mut detector = WakeWordDetector::new(Path::new("models/openwakeword"))?;
            let mut output = Vec::new();
            let mut frame = [0_i16; FRAME_SAMPLES];
            let mut filled = 0;
            let mut peak = 0.0_f32;
            // Synthesize device-rate stereo from the real recording for a
            // reproducible capture-path test, without opening any microphone.
            for index in 0..audio.len() * rate as usize / 16_000 {
                let position = index as f64 * 16_000.0 / f64::from(rate);
                let left = position as usize;
                let fraction = (position - left as f64) as f32;
                let sample = audio[left] * (1.0 - fraction)
                    + audio.get(left + 1).copied().unwrap_or(0.0) * fraction;
                for _ in 0..2 {
                    if let Some(packet) = mono.push(sample) {
                        output.clear();
                        resampler.process(&packet, &mut output);
                        for &sample in &output {
                            frame[filled] = (sample.clamp(-1.0, 1.0) * 32768.0)
                                .round()
                                .clamp(-32768.0, 32767.0)
                                as i16;
                            filled += 1;
                            if filled == FRAME_SAMPLES {
                                peak = peak.max(detector.process(&frame)?);
                                filled = 0;
                            }
                        }
                    }
                }
            }
            assert!(peak >= DETECTION_THRESHOLD, "{rate} Hz: peak {peak}");
        }
        Ok(())
    }
}
