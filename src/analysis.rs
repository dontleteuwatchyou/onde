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
/// An attack stands out from the recent average by this much.
const ONSET_RATIO: f32 = 1.5;
const ONSET_MINIMUM: f32 = 0.12;
const ONSET_MEMORY_SECONDS: f32 = 1.5;
/// Two attacks of the same kind can't be closer than this.
const ONSET_GAP_SECONDS: f32 = 0.09;
const KICK_FALL_SECONDS: f32 = 0.11;
const ENERGY_SECONDS: f32 = 4.0;
const SONG_ENERGY_SECONDS: f32 = 30.0;
/// A part must last this long before the next one counts.
const PART_MINIMUM_SECONDS: f32 = 4.0;
const LIFT_FALL_SECONDS: f32 = 0.8;
const FADE_SECONDS: f32 = 0.6;

/// Which part of a song this sounds like, from its energy: a guess, not a fact.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Part {
    Calm,
    Verse,
    Chorus,
    Bridge,
}

#[derive(Default)]
struct Onset {
    average: f32,
    since_last: f32,
    count: u64,
}

impl Onset {
    /// Returns whether this rise in level is an attack.
    fn feed(&mut self, flux: f32, dt: f32) -> bool {
        self.since_last += dt;
        let hit = flux > ONSET_MINIMUM
            && flux > self.average * ONSET_RATIO
            && self.since_last > ONSET_GAP_SECONDS;
        self.average += (flux - self.average) * (1.0 - (-dt / ONSET_MEMORY_SECONDS).exp());
        if hit {
            self.since_last = 0.0;
            self.count += 1;
        }
        hit
    }
}

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
    previous: [f32; BANDS],
    /// Bands that hear each kind of attack.
    kick_bands: usize,
    snare_bands: Vec<usize>,
    hat_bands: Vec<usize>,
    kick: Onset,
    snare: Onset,
    hat: Onset,
    kick_envelope: f32,
    mid: f32,
    high: f32,
    energy: f32,
    song_energy: f32,
    part: Part,
    part_age: f32,
    lift: f32,
    fade: f32,
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
        let bands_between = |low: f32, high: f32| -> Vec<usize> {
            (0..BANDS)
                .filter(|&band| edge(band) >= low && edge(band) < high)
                .collect()
        };
        let mut snare_bands = bands_between(180.0, 260.0);
        snare_bands.extend(bands_between(2_000.0, 6_000.0));
        let hat_bands = bands_between(8_000.0, 20_000.0);
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
            previous: [0.0; BANDS],
            kick_bands: bass_bands.min(8),
            snare_bands,
            hat_bands,
            kick: Onset::default(),
            snare: Onset::default(),
            hat: Onset::default(),
            kick_envelope: 0.0,
            mid: 0.0,
            high: 0.0,
            energy: 0.0,
            song_energy: 0.0,
            part: Part::Calm,
            part_age: 0.0,
            lift: 0.0,
            fade: 0.0,
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
        self.detect(&targets, dt);
        self.approach(targets, dt);
        self.fade += (1.0 - self.fade) * (1.0 - (-dt / FADE_SECONDS).exp());
    }

    /// Lets the levels fall when nothing is playing.
    pub fn release(&mut self, dt: f32) {
        self.approach([0.0; BANDS], dt);
        self.previous = [0.0; BANDS];
        self.kick_envelope *= (-dt / KICK_FALL_SECONDS).exp();
        self.fade *= (-dt / FADE_SECONDS).exp();
        self.lift *= (-dt / LIFT_FALL_SECONDS).exp();
    }

    fn detect(&mut self, targets: &[f32; BANDS], dt: f32) {
        let rise = |bands: &mut dyn Iterator<Item = usize>, previous: &[f32; BANDS]| {
            let (mut total, mut count) = (0.0, 0);
            for band in bands {
                total += (targets[band] - previous[band]).max(0.0);
                count += 1;
            }
            if count == 0 {
                0.0
            } else {
                total / count as f32
            }
        };
        let kick = rise(&mut (0..self.kick_bands), &self.previous);
        let snare = rise(&mut self.snare_bands.iter().copied(), &self.previous);
        let hat = rise(&mut self.hat_bands.iter().copied(), &self.previous);
        self.previous = *targets;
        self.kick_envelope *= (-dt / KICK_FALL_SECONDS).exp();
        if self.kick.feed(kick, dt) {
            self.kick_envelope = 1.0;
        }
        self.snare.feed(snare, dt);
        self.hat.feed(hat, dt);

        let now = targets.iter().sum::<f32>() / BANDS as f32;
        self.energy += (now - self.energy) * (1.0 - (-dt / ENERGY_SECONDS).exp());
        self.song_energy += (now - self.song_energy) * (1.0 - (-dt / SONG_ENERGY_SECONDS).exp());
        self.lift *= (-dt / LIFT_FALL_SECONDS).exp();
        self.part_age += dt;
        let next = if self.energy < 0.05 {
            Part::Calm
        } else if self.energy > self.song_energy * 1.12 {
            Part::Chorus
        } else if self.energy < self.song_energy * 0.72 {
            Part::Bridge
        } else {
            Part::Verse
        };
        if next != self.part && self.part_age > PART_MINIMUM_SECONDS {
            if next == Part::Chorus {
                self.lift = 1.0;
            }
            self.part = next;
            self.part_age = 0.0;
        }
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
        let average = |bands: &[f32]| bands.iter().sum::<f32>() / bands.len() as f32;
        self.mid = (average(&self.levels[14..31]) * 1.5).min(1.0);
        self.high = (average(&self.levels[34..]) * 1.8).min(1.0);
    }

    pub fn levels(&self) -> &[f32; BANDS] {
        &self.levels
    }

    #[cfg(test)]
    pub fn bass(&self) -> f32 {
        self.bass
    }

    /// The kick's envelope at most: the visuals hit as hard as the drum does.
    pub fn punch(&self) -> f32 {
        self.bass.max(self.kick_envelope * 0.95)
    }

    pub fn kick_envelope(&self) -> f32 {
        self.kick_envelope
    }

    pub fn mid(&self) -> f32 {
        self.mid
    }

    pub fn high(&self) -> f32 {
        self.high
    }

    /// How many attacks so far: a visual reacts when the number changes.
    pub fn attacks(&self) -> (u64, u64, u64) {
        (self.kick.count, self.snare.count, self.hat.count)
    }

    pub fn part(&self) -> Part {
        self.part
    }

    pub fn energy(&self) -> f32 {
        self.energy
    }

    /// Goes to 1 when a chorus starts, then falls back.
    pub fn lift(&self) -> f32 {
        self.lift
    }

    /// 0 when stopped, 1 when playing: motion eases in and out with it.
    pub fn activity(&self) -> f32 {
        self.fade
    }

    /// A recent sample of the sound, from -1 to 1.
    pub fn wave(&self, index: usize) -> f32 {
        self.history[(self.write + index) % FFT_SIZE]
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

    /// A bass drum: short bursts of low tone, with silence between them.
    fn feed_kicks(analyzer: &mut Analyzer, count: usize) {
        let per_update = (RATE as f32 * DT) as usize;
        let mut i = 0_usize;
        for beat in 0..count * 15 {
            let on = beat % 15 < 2;
            for _ in 0..per_update {
                let t = i as f32 / RATE as f32;
                let value = if on {
                    (t * 60.0 * std::f32::consts::TAU).sin() * 0.7
                } else {
                    0.0
                };
                analyzer.push(value);
                i += 1;
            }
            analyzer.update(DT);
        }
    }

    #[test]
    fn counts_drum_hits_one_by_one() {
        let mut analyzer = Analyzer::new(RATE);
        feed_kicks(&mut analyzer, 8);
        let (kicks, _, _) = analyzer.attacks();
        assert!((6..=9).contains(&kicks), "{kicks} coups");
    }

    #[test]
    fn a_steady_note_is_not_a_drum() {
        let mut analyzer = Analyzer::new(RATE);
        feed_sine(&mut analyzer, 60.0, 0.7, 4.0);
        let (kicks, _, _) = analyzer.attacks();
        assert!(kicks <= 1, "{kicks} coups");
    }

    #[test]
    fn activity_eases_in_and_out() {
        let mut analyzer = Analyzer::new(RATE);
        assert_eq!(analyzer.activity(), 0.0);
        feed_sine(&mut analyzer, 440.0, 0.5, 3.0);
        assert!(analyzer.activity() > 0.9);
        for _ in 0..120 {
            analyzer.release(DT);
        }
        assert!(analyzer.activity() < 0.05);
    }

    #[test]
    fn low_sample_rates_keep_valid_bands() {
        let mut analyzer = Analyzer::new(8_000);
        feed_sine(&mut analyzer, 300.0, 0.5, 0.2);
        assert!(!analyzer.is_quiet());
    }
}
