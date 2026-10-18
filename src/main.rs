mod microphone;
mod resample;
mod voice;
mod wake_word;
mod wav;

use std::{env, error::Error, path::Path, process::ExitCode};
use wake_word::{FRAME_SAMPLES, WakeWordDetector};

// Prototype threshold calibrated on samples/heyjarvis.wav (peak ~0.4042).
// Upstream recommends 0.5 by default; broader false-positive testing is needed.
const DETECTION_THRESHOLD: f32 = 0.25;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("Error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let mut args = env::args_os().skip(1);
    let filename = args
        .next()
        .ok_or("Usage: hola-helper <--mic | 16-kHz-mono.wav>")?;
    if args.next().is_some() {
        return Err("Usage: hola-helper <--mic | 16-kHz-mono.wav>".into());
    }
    if filename == "--mic" {
        return microphone::run();
    }
    let mut reader = wav::open(Path::new(&filename))?;
    let mut detector = WakeWordDetector::new(Path::new("models/openwakeword"))?;
    // Match openWakeWord predict_clip's one second of leading/trailing silence.
    let leading = std::iter::repeat_n(0_i16, 16_000).map(Ok);
    let trailing = std::iter::repeat_n(0_i16, 16_000).map(Ok);
    let samples = leading.chain(reader.samples::<i16>()).chain(trailing);
    let mut frame = [0_i16; FRAME_SAMPLES];
    let mut filled = 0;
    let mut detected = false;
    let mut cooldown = 0_usize;
    let mut peak_score = 0.0_f32;
    for sample in samples {
        frame[filled] = sample?;
        filled += 1;
        if filled == FRAME_SAMPLES {
            let score = detector.process(&frame)?;
            peak_score = peak_score.max(score);
            cooldown = cooldown.saturating_sub(1);
            if score >= DETECTION_THRESHOLD && cooldown == 0 {
                println!("Hey Jarvis detected!");
                detected = true;
                voice::reply_or_warn(&std::sync::atomic::AtomicBool::new(false));
                cooldown = 25; // Two seconds, without discarding feature history.
            }
            filled = 0;
        }
    }
    // Preserve the last partial frame rather than dropping samples at EOF.
    if filled != 0 {
        frame[filled..].fill(0);
        let score = detector.process(&frame)?;
        peak_score = peak_score.max(score);
        if score >= DETECTION_THRESHOLD && cooldown == 0 {
            println!("Hey Jarvis detected!");
            detected = true;
            voice::reply_or_warn(&std::sync::atomic::AtomicBool::new(false));
        }
    }
    if !detected {
        println!("No keyword detected.");
    }
    println!("Peak Hey Jarvis score: {peak_score:.4}");
    Ok(())
}
