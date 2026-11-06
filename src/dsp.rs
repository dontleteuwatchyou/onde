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

/// Streaming linear resampler. Good enough for now; a band-limited one can
/// replace it behind the same interface.
pub struct Resampler {
    step: f64,
    channels: usize,
    position: f64,
    last: Vec<f32>,
}

impl Resampler {
    pub fn new(from_rate: u32, to_rate: u32, channels: usize) -> Self {
        Self {
            step: f64::from(from_rate) / f64::from(to_rate),
            channels,
            position: 0.0,
            last: vec![0.0; channels],
        }
    }

    pub fn reset(&mut self) {
        self.position = 0.0;
        self.last.fill(0.0);
    }

    pub fn process(&mut self, input: &[f32]) -> Vec<f32> {
        if self.step == 1.0 {
            return input.to_vec();
        }
        let frames = input.len() / self.channels;
        if frames == 0 {
            return Vec::new();
        }
        let mut out = Vec::with_capacity((frames as f64 / self.step) as usize * self.channels + 8);
        // `position` is measured with -1 meaning the last frame of the previous chunk.
        while self.position < frames as f64 - 1.0 {
            let whole = self.position.floor();
            let fraction = (self.position - whole) as f32;
            let index = whole as isize;
            for ch in 0..self.channels {
                let a = if index < 0 {
                    self.last[ch]
                } else {
                    input[index as usize * self.channels + ch]
                };
                let b = input[(index + 1) as usize * self.channels + ch];
                out.push(a + (b - a) * fraction);
            }
            self.position += self.step;
        }
        self.position -= frames as f64;
        self.last
            .copy_from_slice(&input[(frames - 1) * self.channels..]);
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
        assert!(biggest < 0.001, "discontinuity of {biggest}");
    }
}
