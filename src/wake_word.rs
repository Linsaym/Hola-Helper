//! Streaming openWakeWord v0.6.0 feature pipeline, using its v0.5.1 ONNX assets.
//! Reference: https://github.com/dscripka/openWakeWord/blob/v0.6.0/openwakeword/utils.py
use ort::{session::Session, value::Tensor};
use std::{error::Error, path::Path};

pub const FRAME_SAMPLES: usize = 1280; // 80 ms at 16 kHz
const MEL_BINS: usize = 32;
const MEL_FRAMES: usize = 76;
const EMBEDDING_SIZE: usize = 96;
const FEATURE_FRAMES: usize = 16;

type Result<T> = std::result::Result<T, Box<dyn Error>>;

pub struct WakeWordDetector {
    mel_model: Session,
    embedding_model: Session,
    classifier: Session,
    overlap: Vec<f32>,
    mel: Vec<f32>,
    features: Vec<f32>,
    frames: usize,
}

impl WakeWordDetector {
    pub fn new(model_dir: &Path) -> Result<Self> {
        let mut detector = Self {
            mel_model: load(&model_dir.join("melspectrogram.onnx"))?,
            embedding_model: load(&model_dir.join("embedding_model.onnx"))?,
            classifier: load(&model_dir.join("hey_jarvis_v0.1.onnx"))?,
            overlap: Vec::new(),
            mel: vec![1.0; MEL_FRAMES * MEL_BINS],
            features: Vec::new(),
            frames: 0,
        };
        detector.reset()?;
        Ok(detector)
    }

    /// Clear audio/feature history while keeping the loaded ONNX sessions.
    pub fn reset(&mut self) -> Result<()> {
        self.overlap.clear();
        self.mel.fill(1.0);
        self.features.clear();
        self.frames = 0;
        // Upstream seeds feature history with embeddings of four seconds of
        // low-amplitude random PCM. Use deterministic noise for repeatable tests.
        let mut seed = 1_u32;
        let noise: Vec<f32> = (0..64_000)
            .map(|_| {
                seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                ((seed >> 16) % 2000) as f32 - 1000.0
            })
            .collect();
        let spec = self.melspectrogram(noise)?;
        if spec.len() < MEL_FRAMES * MEL_BINS {
            return Err("Insufficient mel frames to initialize feature history".into());
        }
        for start in (0..=spec.len() - MEL_FRAMES * MEL_BINS).step_by(8 * MEL_BINS) {
            let embedding = self.embed(spec[start..start + MEL_FRAMES * MEL_BINS].to_vec())?;
            self.features.extend(embedding);
        }
        retain_last(&mut self.features, FEATURE_FRAMES * EMBEDDING_SIZE);
        if self.features.len() != FEATURE_FRAMES * EMBEDDING_SIZE {
            return Err("Insufficient initial speech embeddings".into());
        }
        Ok(())
    }

    /// Accept exactly 80 ms of signed 16-bit PCM, retaining bounded history.
    /// Returns a probability; callers decide how to report/debounce detections.
    pub fn process(&mut self, pcm: &[i16; FRAME_SAMPLES]) -> Result<f32> {
        let mut audio = std::mem::take(&mut self.overlap);
        // Do not normalize to [-1, 1]: upstream casts int16 PCM directly to f32.
        audio.extend(pcm.iter().map(|&sample| f32::from(sample)));
        self.overlap = audio[audio.len() - 480..].to_vec();
        let spec = self.melspectrogram(audio)?;
        self.mel.extend(spec);
        retain_last(&mut self.mel, MEL_FRAMES * MEL_BINS);
        let embedding = self.embed(self.mel.clone())?;
        self.features.extend(embedding);
        retain_last(&mut self.features, FEATURE_FRAMES * EMBEDDING_SIZE);
        let prediction = infer(
            &mut self.classifier,
            &[1, FEATURE_FRAMES, EMBEDDING_SIZE],
            self.features.clone(),
        )?;
        if prediction.len() != 1 || !(0.0..=1.0).contains(&prediction[0]) {
            return Err("Hey Jarvis model returned an invalid probability".into());
        }
        self.frames += 1;
        // Upstream suppresses its first five predictions during initialization.
        Ok(if self.frames <= 5 { 0.0 } else { prediction[0] })
    }

    fn melspectrogram(&mut self, audio: Vec<f32>) -> Result<Vec<f32>> {
        let samples = audio.len();
        let mut spec = infer(&mut self.mel_model, &[1, samples], audio)?;
        if spec.len() < MEL_BINS || spec.len() % MEL_BINS != 0 {
            return Err("Unexpected mel spectrogram output shape".into());
        }
        for value in &mut spec {
            *value = *value / 10.0 + 2.0;
        }
        Ok(spec)
    }

    fn embed(&mut self, mel: Vec<f32>) -> Result<Vec<f32>> {
        let embedding = infer(
            &mut self.embedding_model,
            &[1, MEL_FRAMES, MEL_BINS, 1],
            mel,
        )?;
        if embedding.len() != EMBEDDING_SIZE {
            return Err("Expected a 96-value speech embedding".into());
        }
        Ok(embedding)
    }
}

fn load(path: &Path) -> Result<Session> {
    Session::builder()?
        .with_intra_threads(1)?
        .with_inter_threads(1)?
        .commit_from_file(path)
        .map_err(|e| format!("Cannot load ONNX model '{}': {e}", path.display()).into())
}

fn infer(session: &mut Session, shape: &[usize], data: Vec<f32>) -> Result<Vec<f32>> {
    let tensor = Tensor::from_array((shape.to_vec(), data))?;
    let output = session.run(ort::inputs![tensor])?;
    let (_, values) = output[0].try_extract_tensor::<f32>()?;
    Ok(values.to_vec())
}

fn retain_last(buffer: &mut Vec<f32>, count: usize) {
    if buffer.len() > count {
        buffer.drain(..buffer.len() - count);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn peak(samples: &[i16]) -> Result<f32> {
        let mut detector = WakeWordDetector::new(Path::new("models/openwakeword"))?;
        let padding = [0_i16; 16_000];
        let mut audio = Vec::new();
        audio.extend_from_slice(&padding);
        audio.extend_from_slice(samples);
        audio.extend_from_slice(&padding);
        let mut peak = 0.0_f32;
        for chunk in audio.chunks(FRAME_SAMPLES) {
            let mut frame = [0; FRAME_SAMPLES];
            frame[..chunk.len()].copy_from_slice(chunk);
            peak = peak.max(detector.process(&frame)?);
        }
        assert!(detector.overlap.len() <= 480);
        assert_eq!(detector.mel.len(), MEL_FRAMES * MEL_BINS);
        assert_eq!(detector.features.len(), FEATURE_FRAMES * EMBEDDING_SIZE);
        Ok(peak)
    }

    #[test]
    #[ignore = "requires downloaded models and the user's samples/heyjarvis.wav recording"]
    fn recorded_phrase_after_silence_matches_reference() -> Result<()> {
        let mut wav = crate::wav::open(Path::new("samples/heyjarvis.wav"))?;
        let samples: Vec<i16> = wav.samples().collect::<std::result::Result<_, _>>()?;
        let short_peak = peak(&samples)?;
        // Measured with upstream v0.6.0 AudioFeatures + official ONNX classifier.
        assert!((short_peak - 0.40415376).abs() < 0.0001, "{short_peak}");
        let mut delayed = vec![0; 4 * 16_000];
        delayed.extend_from_slice(&samples);
        let delayed_peak = peak(&delayed)?;
        assert!(delayed_peak >= crate::DETECTION_THRESHOLD, "{delayed_peak}");
        assert!((short_peak - delayed_peak).abs() < 0.001);
        assert!(peak(&[0; 20 * 16_000])? < crate::DETECTION_THRESHOLD);
        Ok(())
    }
}
