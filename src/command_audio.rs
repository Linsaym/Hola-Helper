//! Bounded command capture at 16 kHz, using simple energy-based endpointing.
use std::time::{Duration, Instant};

pub const SAMPLE_RATE: usize = 16_000;
pub const MAX_DURATION: Duration = Duration::from_secs(3);
const MAX_SAMPLES: usize = MAX_DURATION.as_secs() as usize * SAMPLE_RATE;
const END_SILENCE_SAMPLES: usize = SAMPLE_RATE * 8 / 10;
const MIN_SPEECH_SAMPLES: usize = SAMPLE_RATE / 5;
const MIN_RMS: f32 = 0.01;

fn above_speech_threshold(packet: &[f32]) -> bool {
    !packet.is_empty()
        && packet.iter().map(|s| s * s).sum::<f32>() / packet.len() as f32 >= MIN_RMS * MIN_RMS
}

/// A final guard for STT callers, including recordings supplied without capture.
pub fn contains_speech(audio: &[f32]) -> bool {
    audio
        .chunks(SAMPLE_RATE / 50)
        .filter(|packet| above_speech_threshold(packet))
        .map(|packet| packet.len())
        .sum::<usize>()
        >= MIN_SPEECH_SAMPLES
}

pub struct CommandRecording {
    started: Instant,
    samples: Vec<f32>,
    speech_samples: usize,
    silence_samples: usize,
}

impl CommandRecording {
    pub fn new() -> Self {
        Self {
            started: Instant::now(),
            samples: Vec::with_capacity(MAX_SAMPLES),
            speech_samples: 0,
            silence_samples: 0,
        }
    }

    /// Feed a short resampled packet. Returns true when recording is complete.
    pub fn push(&mut self, packet: &[f32]) -> bool {
        if packet.is_empty() {
            return self.is_complete();
        }
        let count = packet.len().min(MAX_SAMPLES - self.samples.len());
        let packet = &packet[..count];
        if !packet.is_empty() {
            if above_speech_threshold(packet) {
                self.speech_samples += packet.len();
                self.silence_samples = 0;
            } else {
                self.silence_samples += packet.len();
            }
            self.samples.extend_from_slice(packet);
        }
        self.is_complete()
    }

    pub fn is_complete(&self) -> bool {
        self.is_complete_at(self.started.elapsed())
    }

    fn is_complete_at(&self, elapsed: Duration) -> bool {
        self.samples.len() >= MAX_SAMPLES
            || elapsed >= MAX_DURATION
            || (self.speech_samples >= MIN_SPEECH_SAMPLES
                && self.silence_samples >= END_SILENCE_SAMPLES)
    }

    /// Silence and brief clicks never reach the speech recognizer.
    pub fn into_audio(self) -> Option<Vec<f32>> {
        (self.speech_samples >= MIN_SPEECH_SAMPLES).then_some(self.samples)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stt_guard_rejects_silence_low_energy_and_clicks() {
        assert!(!contains_speech(&[]));
        assert!(!contains_speech(&vec![0.0; 2 * SAMPLE_RATE]));
        assert!(!contains_speech(&vec![0.001; 2 * SAMPLE_RATE]));
        assert!(!contains_speech(&[0.5; 160]));
        assert!(contains_speech(&vec![0.05; SAMPLE_RATE]));
    }

    #[test]
    fn silence_and_brief_clicks_do_not_trigger_stt() {
        let mut recording = CommandRecording::new();
        recording.push(&[0.3; 160]);
        assert!(recording.push(&vec![0.0; 3 * SAMPLE_RATE]));
        assert!(recording.into_audio().is_none());
        let recording = CommandRecording::new();
        assert!(recording.is_complete_at(Duration::from_secs(3)));
        assert!(recording.into_audio().is_none());
    }

    #[test]
    fn speech_ends_after_a_pause_and_next_recording_starts_clean() {
        let mut recording = CommandRecording::new();
        assert!(!recording.push(&vec![0.05; SAMPLE_RATE]));
        assert!(recording.push(&vec![0.0; END_SILENCE_SAMPLES]));
        assert_eq!(
            recording.into_audio().unwrap().len(),
            SAMPLE_RATE + END_SILENCE_SAMPLES
        );
        assert!(CommandRecording::new().into_audio().is_none());
    }

    #[test]
    fn continuous_audio_is_bounded_even_without_a_pause() {
        let mut recording = CommandRecording::new();
        assert!(recording.push(&vec![0.1; MAX_SAMPLES + 1000]));
        assert_eq!(recording.into_audio().unwrap().len(), 3 * SAMPLE_RATE);
    }

    #[test]
    fn wall_clock_timeout_is_three_seconds_even_if_audio_stops() {
        let mut recording = CommandRecording::new();
        assert!(!recording.is_complete_at(Duration::from_millis(2_999)));
        assert!(recording.is_complete_at(Duration::from_secs(3)));
        recording.push(&vec![0.05; SAMPLE_RATE]);
        assert!(!recording.is_complete_at(Duration::from_millis(2_999)));
        assert!(recording.is_complete_at(Duration::from_secs(3)));
    }
}
