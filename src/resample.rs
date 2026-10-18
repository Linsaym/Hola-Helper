//! Bounded, continuous windowed-sinc resampling to 16 kHz.
use std::{collections::VecDeque, f64::consts::PI};

const OUTPUT_RATE: u64 = 16_000;
const HALF: i64 = 16;

pub struct Resampler {
    rate: u32,
    history: VecDeque<f32>,
    input_count: u64,
    next_output: u64,
}

impl Resampler {
    pub fn new(rate: u32) -> Self {
        assert!(rate > 0);
        Self {
            rate,
            history: VecDeque::with_capacity(32),
            input_count: 0,
            next_output: 0,
        }
    }

    pub fn process(&mut self, input: &[f32], output: &mut Vec<f32>) {
        if self.rate == OUTPUT_RATE as u32 {
            output.extend_from_slice(input);
            return;
        }
        let cutoff = 0.45 * (OUTPUT_RATE as f64 / f64::from(self.rate)).min(1.0);
        for &sample in input {
            if self.history.len() == 32 {
                self.history.pop_front();
            }
            self.history.push_back(sample);
            let newest = self.input_count as i64;
            self.input_count += 1;
            // Wait for the right side of the interpolation/filter window.
            while (self.next_output / OUTPUT_RATE) as i64 + HALF <= newest {
                let center = (self.next_output / OUTPUT_RATE) as i64;
                let fraction = (self.next_output % OUTPUT_RATE) as f64 / OUTPUT_RATE as f64;
                let mut value = 0.0;
                let mut weight_sum = 0.0;
                for index in center - HALF + 1..=center + HALF {
                    let distance = (index - center) as f64 - fraction;
                    let sinc = if distance.abs() < 1e-12 {
                        2.0 * cutoff
                    } else {
                        (2.0 * PI * cutoff * distance).sin() / (PI * distance)
                    };
                    let weight = sinc * (0.5 + 0.5 * (PI * distance / HALF as f64).cos());
                    let offset = index - (self.input_count as i64 - self.history.len() as i64);
                    if offset >= 0 && (offset as usize) < self.history.len() {
                        value += f64::from(self.history[offset as usize]) * weight;
                    }
                    weight_sum += weight;
                }
                output.push((value / weight_sum) as f32);
                // Integer phase accumulation avoids drift at e.g. 44100 Hz.
                self.next_output += u64::from(self.rate);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn callback_boundaries_preserve_signal_and_sample_count() {
        for rate in [16_000, 44_100, 48_000] {
            let input: Vec<f32> = (0..rate)
                .map(|i| (2.0 * PI * 1000.0 * f64::from(i) / f64::from(rate)).sin() as f32)
                .collect();
            let mut whole = Vec::new();
            Resampler::new(rate).process(&input, &mut whole);
            let mut split = Vec::new();
            let mut resampler = Resampler::new(rate);
            for chunk in input.chunks(137) {
                resampler.process(chunk, &mut split);
            }
            assert_eq!(whole, split);
            assert!((15_990..=16_000).contains(&split.len()));
            let rms = (split[100..].iter().map(|x| x * x).sum::<f32>()
                / (split.len() - 100) as f32)
                .sqrt();
            assert!((rms - 0.707).abs() < 0.02, "{rate}: {rms}");
        }
    }

    #[test]
    fn rejects_aliasing_above_output_nyquist() {
        let input: Vec<f32> = (0..48_000)
            .map(|i| (2.0 * PI * 12_000.0 * f64::from(i) / 48_000.0).sin() as f32)
            .collect();
        let mut output = Vec::new();
        Resampler::new(48_000).process(&input, &mut output);
        let rms =
            (output[100..].iter().map(|x| x * x).sum::<f32>() / (output.len() - 100) as f32).sqrt();
        assert!(rms < 0.02, "{rms}");
    }
}
