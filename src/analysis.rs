//! Turns the audio being played into levels for the visuals.

use std::sync::Arc;

use rustfft::num_complex::Complex;
use rustfft::{Fft, FftPlanner};

pub const BANDS: usize = 48;

const FFT_SIZE: usize = 2048;
const LOWEST_HZ: f32 = 40.0;
const HIGHEST_HZ: f32 = 16_000.0;
/// Upper limit of what counts as bass.
const BASS_HZ: f32 = 160.0;
/// Below this the track is treated as silent, so noise isn't amplified.
const SILENCE: f32 = 0.002;
/// Levels rise fast and fall slowly, which reads as rhythm rather than flicker.
const ATTACK_SECONDS: f32 = 0.02;
const RELEASE_SECONDS: f32 = 0.25;
/// How fast the automatic gain forgets a loud passage.
const GAIN_HALF_LIFE_SECONDS: f32 = 3.0;
/// Under 1 lifts quiet bands, so more than the loudest one moves.
const CONTRAST: f32 = 0.6;
const QUIET_LEVEL: f32 = 0.002;

pub struct Analyzer {
    fft: Arc<dyn Fft<f32>>,
    window: Vec<f32>,
    window_sum: f32,
    history: Vec<f32>,
    write: usize,
    buffer: Vec<Complex<f32>>,
    /// Bin range and tilt of each band.
    bands: Vec<(usize, usize, f32)>,
    bass_bands: usize,
    levels: [f32; BANDS],
    bass: f32,
    loudest: f32,
}

impl Analyzer {
    pub fn new(sample_rate: u32) -> Self {
        let window: Vec<f32> = (0..FFT_SIZE)
            .map(|i| {
                let phase = i as f32 / (FFT_SIZE - 1) as f32;
                0.5 - 0.5 * (std::f32::consts::TAU * phase).cos()
            })
            .collect();
        let bin_hz = sample_rate as f32 / FFT_SIZE as f32;
        let highest = HIGHEST_HZ.min(sample_rate as f32 * 0.45);
        let edge = |band: usize| LOWEST_HZ * (highest / LOWEST_HZ).powf(band as f32 / BANDS as f32);
        let bands: Vec<(usize, usize, f32)> = (0..BANDS)
            .map(|band| {
                let start = (edge(band) / bin_hz) as usize;
                let end = ((edge(band + 1) / bin_hz) as usize).max(start + 1);
                // Music has less energy up high: this makes the bands comparable.
                let tilt = ((edge(band) / 1000.0).sqrt()).clamp(0.25, 4.0);
                (start, end.min(FFT_SIZE / 2), tilt)
            })
            .collect();
        let bass_bands = (0..BANDS)
            .take_while(|&band| edge(band) < BASS_HZ)
            .count()
            .max(1);
        Self {
            fft: FftPlanner::new().plan_fft_forward(FFT_SIZE),
            window_sum: window.iter().sum(),
            window,
            history: vec![0.0; FFT_SIZE],
            write: 0,
            buffer: vec![Complex::default(); FFT_SIZE],
            bands,
            bass_bands,
            levels: [0.0; BANDS],
            bass: 0.0,
            loudest: 0.0,
        }
    }

    pub fn push(&mut self, sample: f32) {
        self.history[self.write] = sample;
        self.write = (self.write + 1) % FFT_SIZE;
    }

    /// Analyzes the latest audio and moves the levels towards it.
    pub fn update(&mut self, dt: f32) {
        for (index, slot) in self.buffer.iter_mut().enumerate() {
            let sample = self.history[(self.write + index) % FFT_SIZE];
            *slot = Complex::new(sample * self.window[index], 0.0);
        }
        self.fft.process(&mut self.buffer);

        let mut raw = [0.0_f32; BANDS];
        for (band, &(start, end, tilt)) in self.bands.iter().enumerate() {
            let peak = self.buffer[start..end]
                .iter()
                .map(|bin| bin.norm())
                .fold(0.0, f32::max);
            raw[band] = 2.0 * peak / self.window_sum * tilt;
        }
        let loudest_now = raw.iter().copied().fold(0.0, f32::max);
        let decay = 0.5_f32.powf(dt / GAIN_HALF_LIFE_SECONDS);
        self.loudest = loudest_now.max(self.loudest * decay);

        let mut targets = [0.0; BANDS];
        if self.loudest > SILENCE {
            for (target, value) in targets.iter_mut().zip(raw) {
                *target = (value / self.loudest).powf(CONTRAST).min(1.0);
            }
        }
        self.approach(targets, dt);
    }

    /// Lets the levels fall when nothing is playing.
    pub fn release(&mut self, dt: f32) {
        self.approach([0.0; BANDS], dt);
    }

    fn approach(&mut self, targets: [f32; BANDS], dt: f32) {
        for (level, target) in self.levels.iter_mut().zip(targets) {
            let seconds = if target > *level {
                ATTACK_SECONDS
            } else {
                RELEASE_SECONDS
            };
            *level += (target - *level) * (1.0 - (-dt / seconds).exp());
        }
        self.bass = self.levels[..self.bass_bands].iter().sum::<f32>() / self.bass_bands as f32;
    }

    pub fn levels(&self) -> &[f32; BANDS] {
        &self.levels
    }

    pub fn bass(&self) -> f32 {
        self.bass
    }

    pub fn is_quiet(&self) -> bool {
        self.levels.iter().all(|level| *level < QUIET_LEVEL)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 48_000;
    const DT: f32 = 1.0 / 30.0;

    fn feed_sine(analyzer: &mut Analyzer, hz: f32, amplitude: f32, seconds: f32) {
        let per_update = (RATE as f32 * DT) as usize;
        let total = (RATE as f32 * seconds) as usize;
        for start in (0..total).step_by(per_update) {
            for i in start..start + per_update {
                let t = i as f32 / RATE as f32;
                analyzer.push((t * hz * std::f32::consts::TAU).sin() * amplitude);
            }
            analyzer.update(DT);
        }
    }

    /// The band whose level is highest.
    fn loudest_band(analyzer: &Analyzer) -> usize {
        let levels = analyzer.levels();
        (0..BANDS)
            .max_by(|&a, &b| levels[a].total_cmp(&levels[b]))
            .unwrap()
    }

    #[test]
    fn a_low_note_lights_the_low_bands_and_the_bass() {
        let mut analyzer = Analyzer::new(RATE);
        feed_sine(&mut analyzer, 80.0, 0.5, 0.5);
        assert!(
            loudest_band(&analyzer) < analyzer.bass_bands,
            "bande {}",
            loudest_band(&analyzer)
        );
        assert!(analyzer.bass() > 0.5, "basse {}", analyzer.bass());
    }

    #[test]
    fn a_high_note_lights_the_high_bands_and_not_the_bass() {
        let mut analyzer = Analyzer::new(RATE);
        feed_sine(&mut analyzer, 6_000.0, 0.5, 0.5);
        assert!(
            loudest_band(&analyzer) > 32,
            "bande {}",
            loudest_band(&analyzer)
        );
        assert!(analyzer.bass() < 0.2, "basse {}", analyzer.bass());
    }

    #[test]
    fn silence_stays_at_zero() {
        let mut analyzer = Analyzer::new(RATE);
        feed_sine(&mut analyzer, 440.0, 0.0, 0.5);
        assert!(analyzer.is_quiet());
    }

    #[test]
    fn inaudible_noise_is_not_amplified() {
        let mut analyzer = Analyzer::new(RATE);
        feed_sine(&mut analyzer, 440.0, 0.0002, 0.5);
        assert!(analyzer.is_quiet());
    }

    #[test]
    fn a_quiet_track_moves_as_much_as_a_loud_one() {
        let mut quiet = Analyzer::new(RATE);
        feed_sine(&mut quiet, 440.0, 0.05, 0.5);
        let mut loud = Analyzer::new(RATE);
        feed_sine(&mut loud, 440.0, 0.8, 0.5);
        let peak = |a: &Analyzer| a.levels().iter().copied().fold(0.0, f32::max);
        assert!((peak(&quiet) - peak(&loud)).abs() < 0.05);
    }

    #[test]
    fn levels_fall_back_after_the_music_stops() {
        let mut analyzer = Analyzer::new(RATE);
        feed_sine(&mut analyzer, 440.0, 0.5, 0.5);
        assert!(!analyzer.is_quiet());
        analyzer.release(DT);
        assert!(!analyzer.is_quiet(), "doit retomber progressivement");
        for _ in 0..90 {
            analyzer.release(DT);
        }
        assert!(analyzer.is_quiet());
    }

    #[test]
    fn low_sample_rates_keep_valid_bands() {
        let mut analyzer = Analyzer::new(8_000);
        feed_sine(&mut analyzer, 300.0, 0.5, 0.2);
        assert!(!analyzer.is_quiet());
    }
}
