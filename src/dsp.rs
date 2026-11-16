//! Channel mapping and sample-rate conversion for interleaved f32 audio.

/// Converts interleaved audio from `from` channels to `to` channels.
pub fn remix(input: &[f32], from: usize, to: usize) -> Vec<f32> {
    if from == to {
        return input.to_vec();
    }
    let mut out = Vec::with_capacity(input.len() / from * to);
    for frame in input.chunks_exact(from) {
        if to == 1 {
            out.push(frame.iter().sum::<f32>() / from as f32);
        } else if from == 1 {
            out.extend(std::iter::repeat_n(frame[0], to));
        } else {
            out.extend((0..to).map(|ch| frame.get(ch).copied().unwrap_or(0.0)));
        }
    }
    out
}

/// Half-length of the sinc filter: 16 taps on each side = 32 total.
const HALF_TAPS: usize = 16;
/// Number of precomputed sub-sample phases for polyphase interpolation.
const PHASES: usize = 256;
/// Kaiser window parameter: controls sidelobe attenuation (~80 dB at beta=8).
const KAISER_BETA: f64 = 8.0;

fn bessel_i0(x: f64) -> f64 {
    let mut sum = 1.0;
    let mut term = 1.0;
    for k in 1..25 {
        term *= (x / (2.0 * k as f64)) * (x / (2.0 * k as f64));
        sum += term;
    }
    sum
}

fn build_sinc_table() -> Vec<[f32; HALF_TAPS * 2]> {
    let denom = bessel_i0(KAISER_BETA);
    let full = HALF_TAPS * 2;
    let mut table = vec![[0.0f32; HALF_TAPS * 2]; PHASES];
    for phase in 0..PHASES {
        let frac = phase as f64 / PHASES as f64;
        for tap in 0..full {
            let n = tap as f64 - (HALF_TAPS as f64 - 1.0) - frac;
            let sinc = if n.abs() < 1e-9 {
                1.0
            } else {
                (n * std::f64::consts::PI).sin() / (n * std::f64::consts::PI)
            };
            let window_pos = (tap as f64 + frac) / full as f64;
            let r = 2.0 * window_pos - 1.0;
            let kaiser = bessel_i0(KAISER_BETA * (1.0 - r * r).max(0.0).sqrt()) / denom;
            table[phase][tap] = (sinc * kaiser) as f32;
        }
        let sum: f32 = table[phase].iter().sum();
        if sum.abs() > 1e-6 {
            for coeff in &mut table[phase] {
                *coeff /= sum;
            }
        }
    }
    table
}

/// Streaming windowed-sinc resampler with polyphase decomposition.
pub struct Resampler {
    step: f64,
    channels: usize,
    position: f64,
    /// Ring buffer holding recent input frames (interleaved, channels * HISTORY_FRAMES).
    history: Vec<f32>,
    /// Write position in the ring buffer (in frames).
    write_pos: usize,
    /// How many frames have been written since reset (capped at capacity).
    filled: usize,
    table: &'static Vec<[f32; HALF_TAPS * 2]>,
}

use std::sync::LazyLock;

static SINC_TABLE: LazyLock<Vec<[f32; HALF_TAPS * 2]>> = LazyLock::new(build_sinc_table);

const HISTORY_FRAMES: usize = HALF_TAPS * 2 + 1;

impl Resampler {
    pub fn new(from_rate: u32, to_rate: u32, channels: usize) -> Self {
        Self {
            step: f64::from(from_rate) / f64::from(to_rate),
            channels,
            position: 0.0,
            history: vec![0.0; channels * HISTORY_FRAMES],
            write_pos: 0,
            filled: 0,
            table: &SINC_TABLE,
        }
    }

    pub fn reset(&mut self) {
        self.position = 0.0;
        self.history.fill(0.0);
        self.write_pos = 0;
        self.filled = 0;
    }

    fn push_frame(&mut self, frame: &[f32]) {
        let base = self.write_pos * self.channels;
        self.history[base..base + self.channels].copy_from_slice(frame);
        self.write_pos = (self.write_pos + 1) % HISTORY_FRAMES;
        if self.filled < HISTORY_FRAMES {
            self.filled += 1;
        }
    }

    fn read_history(&self, frame_offset: isize, channel: usize) -> f32 {
        let pos = (self.write_pos as isize + frame_offset)
            .rem_euclid(HISTORY_FRAMES as isize) as usize;
        self.history[pos * self.channels + channel]
    }

    pub fn process(&mut self, input: &[f32]) -> Vec<f32> {
        if self.step == 1.0 {
            return input.to_vec();
        }
        let in_frames = input.len() / self.channels;
        if in_frames == 0 {
            return Vec::new();
        }
        let est = (in_frames as f64 / self.step) as usize + 8;
        let mut out = Vec::with_capacity(est * self.channels);
        let full = HALF_TAPS * 2;

        for frame_idx in 0..in_frames {
            self.push_frame(&input[frame_idx * self.channels..(frame_idx + 1) * self.channels]);

            while self.position < 1.0 && self.filled >= full {
                let frac = self.position;
                let phase = ((frac * PHASES as f64) as usize).min(PHASES - 1);
                let coeffs = &self.table[phase];

                for ch in 0..self.channels {
                    let mut sample = 0.0f32;
                    for tap in 0..full {
                        let offset = tap as isize - full as isize;
                        sample += self.read_history(offset, ch) * coeffs[tap];
                    }
                    out.push(sample);
                }
                self.position += self.step;
            }
            self.position -= 1.0;
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mono_is_copied_to_every_channel() {
        assert_eq!(remix(&[0.5, -0.5], 1, 2), vec![0.5, 0.5, -0.5, -0.5]);
    }

    #[test]
    fn stereo_is_averaged_to_mono() {
        assert_eq!(remix(&[1.0, 0.0, 0.5, 0.5], 2, 1), vec![0.5, 0.5]);
    }

    #[test]
    fn surround_keeps_front_channels_for_stereo() {
        assert_eq!(remix(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0], 6, 2), vec![1.0, 2.0]);
    }

    #[test]
    fn same_rate_passes_through() {
        let mut r = Resampler::new(48_000, 48_000, 2);
        assert_eq!(r.process(&[0.1, 0.2]), vec![0.1, 0.2]);
    }

    #[test]
    fn resampling_keeps_duration_across_chunks() {
        let mut r = Resampler::new(44_100, 48_000, 1);
        let chunk = vec![0.25_f32; 4_410];
        let total: usize = (0..10).map(|_| r.process(&chunk).len()).sum();
        assert!((total as i64 - 48_000).abs() <= 2, "got {total} frames");
    }

    #[test]
    fn resampling_does_not_jump_between_chunks() {
        let ramp: Vec<f32> = (0..2_000).map(|i| i as f32 / 2_000.0).collect();
        let mut r = Resampler::new(44_100, 48_000, 1);
        let mut out = r.process(&ramp[..1_000]);
        out.extend(r.process(&ramp[1_000..]));
        let biggest = out
            .windows(2)
            .map(|w| (w[1] - w[0]).abs())
            .fold(0.0, f32::max);
        assert!(biggest < 0.002, "discontinuity of {biggest}");
    }

    #[test]
    fn sinc_table_is_normalized() {
        let table = build_sinc_table();
        let sum: f32 = table[0].iter().sum();
        assert!((sum - 1.0).abs() < 0.01, "phase 0 sum = {sum}");
    }

    #[test]
    fn downsampling_preserves_duration() {
        let mut r = Resampler::new(96_000, 48_000, 1);
        let chunk = vec![0.5_f32; 9_600];
        let total: usize = (0..10).map(|_| r.process(&chunk).len()).sum();
        assert!((total as i64 - 48_000).abs() <= 2, "got {total} frames");
    }
}
