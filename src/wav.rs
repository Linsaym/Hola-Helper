use std::{error::Error, fs::File, io::BufReader, path::Path};

pub fn open(path: &Path) -> Result<hound::WavReader<BufReader<File>>, Box<dyn Error>> {
    let reader = hound::WavReader::open(path)
        .map_err(|e| format!("Cannot read WAV '{}': {e}", path.display()))?;
    let spec = reader.spec();
    if spec.sample_rate != 16_000 || spec.channels != 1 {
        return Err(format!(
            "Expected 16000 Hz mono WAV; got {} Hz, {} channels",
            spec.sample_rate, spec.channels
        )
        .into());
    }
    if spec.sample_format != hound::SampleFormat::Int || spec.bits_per_sample != 16 {
        return Err("Expected 16-bit PCM WAV audio".into());
    }
    if reader.len() == 0 {
        return Err("WAV file contains no audio samples".into());
    }
    Ok(reader)
}
