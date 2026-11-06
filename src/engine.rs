//! Playback: a worker thread decodes into a ring buffer, the audio callback drains it.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample, Stream, StreamConfig};
use rtrb::{Consumer, Producer, RingBuffer};

use crate::decoder::{AudioSource, TrackDecoder};
use crate::dsp::{Resampler, remix};

enum Command {
    Load(AudioSource),
    Seek(f64),
    Stop,
}

#[derive(Default)]
struct Shared {
    paused: AtomicBool,
    volume: AtomicU32,
    played_frames: AtomicU64,
    base_seconds: AtomicU64,
    flush: AtomicBool,
    end_of_file: AtomicBool,
    finished: AtomicBool,
    error: Mutex<Option<String>>,
}

impl Shared {
    fn set_base(&self, seconds: f64) {
        self.base_seconds.store(seconds.to_bits(), Ordering::SeqCst);
        self.played_frames.store(0, Ordering::SeqCst);
    }

    /// Asks the callback to drop what is buffered and waits until it has.
    fn flush(&self) {
        self.flush.store(true, Ordering::SeqCst);
        let deadline = Instant::now() + Duration::from_millis(250);
        while self.flush.load(Ordering::SeqCst) && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(1));
        }
    }
}

pub struct Engine {
    commands: Sender<Command>,
    shared: Arc<Shared>,
    rate: u32,
    /// What is being played, mono and before the volume.
    tap: Mutex<Consumer<f32>>,
    _stream: Stream,
}

impl Engine {
    pub fn new() -> Result<Self> {
        let device = cpal::default_host()
            .default_output_device()
            .ok_or_else(|| anyhow!("aucune sortie audio"))?;
        let supported = device
            .default_output_config()
            .context("configuration de la sortie audio")?;
        let sample_format = supported.sample_format();
        let config: StreamConfig = supported.into();
        let rate = config.sample_rate;
        let channels = usize::from(config.channels);

        let shared = Arc::new(Shared::default());
        shared.volume.store(1.0_f32.to_bits(), Ordering::SeqCst);

        // Two seconds keeps the UI responsive; seeks and track changes flush it.
        let (producer, consumer) = RingBuffer::<f32>::new(rate as usize * channels * 2);
        let (tap_in, tap_out) = RingBuffer::<f32>::new(rate as usize);
        let stream = match sample_format {
            SampleFormat::F32 => build::<f32>(&device, config, consumer, tap_in, &shared, channels),
            SampleFormat::I16 => build::<i16>(&device, config, consumer, tap_in, &shared, channels),
            SampleFormat::I32 => build::<i32>(&device, config, consumer, tap_in, &shared, channels),
            SampleFormat::U16 => build::<u16>(&device, config, consumer, tap_in, &shared, channels),
            other => bail!("format de sortie non pris en charge : {other}"),
        }?;
        stream.play().context("démarrage de la sortie audio")?;

        let (commands, receiver) = mpsc::channel();
        let worker_shared = shared.clone();
        thread::Builder::new()
            .name("onde-decoder".into())
            .spawn(move || worker(receiver, producer, worker_shared, rate, channels))?;

        Ok(Self {
            commands,
            shared,
            rate,
            tap: Mutex::new(tap_out),
            _stream: stream,
        })
    }

    pub fn load(&self, source: AudioSource) {
        self.set_paused(false);
        let _ = self.commands.send(Command::Load(source));
    }

    pub fn seek(&self, seconds: f64) {
        let _ = self.commands.send(Command::Seek(seconds));
    }

    pub fn stop(&self) {
        let _ = self.commands.send(Command::Stop);
    }

    pub fn set_paused(&self, paused: bool) {
        self.shared.paused.store(paused, Ordering::SeqCst);
    }

    pub fn is_paused(&self) -> bool {
        self.shared.paused.load(Ordering::SeqCst)
    }

    pub fn set_volume(&self, volume: f32) {
        self.shared
            .volume
            .store(volume.clamp(0.0, 1.0).to_bits(), Ordering::SeqCst);
    }

    pub fn volume(&self) -> f32 {
        f32::from_bits(self.shared.volume.load(Ordering::SeqCst))
    }

    pub fn sample_rate(&self) -> u32 {
        self.rate
    }

    /// Hands over the audio played since the last call.
    pub fn drain_tap(&self, mut receive: impl FnMut(f32)) {
        if let Ok(mut tap) = self.tap.lock() {
            while let Ok(sample) = tap.pop() {
                receive(sample);
            }
        }
    }

    pub fn position(&self) -> f64 {
        let base = f64::from_bits(self.shared.base_seconds.load(Ordering::SeqCst));
        base + self.shared.played_frames.load(Ordering::SeqCst) as f64 / f64::from(self.rate)
    }

    /// True once, when the current track has been played to its end.
    pub fn take_finished(&self) -> bool {
        self.shared.finished.swap(false, Ordering::SeqCst)
    }

    pub fn take_error(&self) -> Option<String> {
        self.shared.error.lock().ok()?.take()
    }
}

fn build<T>(
    device: &cpal::Device,
    config: StreamConfig,
    mut consumer: Consumer<f32>,
    mut tap: Producer<f32>,
    shared: &Arc<Shared>,
    channels: usize,
) -> Result<Stream>
where
    T: SizedSample + FromSample<f32> + Send + 'static,
{
    let shared = shared.clone();
    let stream = device.build_output_stream::<T, _, _>(
        config,
        move |output: &mut [T], _| {
            if shared.flush.load(Ordering::SeqCst) {
                while consumer.pop().is_ok() {}
                shared.flush.store(false, Ordering::SeqCst);
            }
            if shared.paused.load(Ordering::SeqCst) {
                output.fill(T::EQUILIBRIUM);
                return;
            }
            let volume = f32::from_bits(shared.volume.load(Ordering::SeqCst));
            let mut written = 0;
            let mut mix = 0.0;
            for slot in output.iter_mut() {
                *slot = match consumer.pop() {
                    Ok(sample) => {
                        written += 1;
                        mix += sample;
                        if written % channels == 0 {
                            // When nobody reads the tap it is full: dropping is fine.
                            let _ = tap.push(mix / channels as f32);
                            mix = 0.0;
                        }
                        T::from_sample(sample * volume)
                    }
                    Err(_) => T::EQUILIBRIUM,
                };
            }
            shared
                .played_frames
                .fetch_add((written / channels) as u64, Ordering::SeqCst);
            if written == 0 && shared.end_of_file.swap(false, Ordering::SeqCst) {
                shared.finished.store(true, Ordering::SeqCst);
            }
        },
        |error| eprintln!("sortie audio : {error}"),
        None,
    )?;
    Ok(stream)
}

struct Active {
    decoder: TrackDecoder,
    resampler: Option<Resampler>,
    pending: Vec<f32>,
    written: usize,
    done: bool,
}

fn worker(
    commands: Receiver<Command>,
    mut producer: Producer<f32>,
    shared: Arc<Shared>,
    rate: u32,
    channels: usize,
) {
    let mut active: Option<Active> = None;
    loop {
        let idle = active.as_ref().is_none_or(|a| a.done);
        let command = if idle {
            match commands.recv_timeout(Duration::from_millis(100)) {
                Ok(command) => Some(command),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => return,
            }
        } else {
            commands.try_recv().ok()
        };
        if let Some(command) = command {
            handle(command, &mut active, &shared);
            continue;
        }
        let Some(track) = active.as_mut() else {
            continue;
        };
        if track.done {
            continue;
        }
        if !fill(track, &mut producer, &shared, rate, channels) {
            // The ring buffer is full, the callback needs time to drain it.
            thread::sleep(Duration::from_millis(10));
        }
    }
}

fn handle(command: Command, active: &mut Option<Active>, shared: &Shared) {
    shared.flush();
    shared.end_of_file.store(false, Ordering::SeqCst);
    shared.finished.store(false, Ordering::SeqCst);
    match command {
        Command::Stop => {
            *active = None;
            shared.set_base(0.0);
        }
        Command::Load(source) => {
            shared.set_base(0.0);
            *active = match TrackDecoder::open(&source) {
                Ok(decoder) => Some(Active {
                    decoder,
                    resampler: None,
                    pending: Vec::new(),
                    written: 0,
                    done: false,
                }),
                Err(error) => {
                    report(shared, format!("{error:#}"));
                    None
                }
            };
        }
        Command::Seek(seconds) => {
            let Some(track) = active.as_mut() else { return };
            match track.decoder.seek(seconds) {
                Ok(()) => {
                    track.pending.clear();
                    track.written = 0;
                    track.done = false;
                    if let Some(resampler) = track.resampler.as_mut() {
                        resampler.reset();
                    }
                    shared.set_base(seconds);
                }
                Err(error) => report(shared, format!("{error:#}")),
            }
        }
    }
}

/// Moves audio towards the ring buffer. Returns false when it is full.
fn fill(
    track: &mut Active,
    producer: &mut Producer<f32>,
    shared: &Shared,
    rate: u32,
    channels: usize,
) -> bool {
    if track.written == track.pending.len() {
        track.written = 0;
        track.pending.clear();
        match track.decoder.next_chunk() {
            Ok(Some(chunk)) => {
                let mixed = remix(&chunk.samples, chunk.channels, channels);
                let resampler = track
                    .resampler
                    .get_or_insert_with(|| Resampler::new(chunk.rate, rate, channels));
                track.pending = resampler.process(&mixed);
            }
            Ok(None) => {
                track.done = true;
                shared.end_of_file.store(true, Ordering::SeqCst);
            }
            Err(error) => {
                report(shared, format!("{error:#}"));
                track.done = true;
                shared.end_of_file.store(true, Ordering::SeqCst);
            }
        }
        return true;
    }
    while track.written < track.pending.len() {
        if producer.push(track.pending[track.written]).is_err() {
            return false;
        }
        track.written += 1;
    }
    true
}

fn report(shared: &Shared, message: String) {
    if let Ok(mut error) = shared.error.lock() {
        *error = Some(message);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decoder::tests::write_wav;

    /// Needs a real audio device, and plays silence so it stays inaudible.
    /// Run with `cargo test -- --ignored`.
    #[test]
    #[ignore = "nécessite une sortie audio"]
    fn plays_seeks_pauses_and_reaches_the_end() {
        let path = write_wav("engine.wav", 44_100, 2, 2.0, false);
        let engine = Engine::new().unwrap();
        engine.load(AudioSource::File(path));
        thread::sleep(Duration::from_millis(500));
        let first = engine.position();
        assert!(first > 0.2, "la lecture n'avance pas ({first}s)");

        engine.set_paused(true);
        thread::sleep(Duration::from_millis(100));
        let held = engine.position();
        thread::sleep(Duration::from_millis(200));
        assert_eq!(engine.position(), held, "la pause n'arrête pas la position");
        engine.set_paused(false);

        engine.seek(1.5);
        thread::sleep(Duration::from_millis(150));
        assert!(engine.position() >= 1.5, "le déplacement est ignoré");

        let deadline = Instant::now() + Duration::from_secs(3);
        while !engine.take_finished() {
            assert!(Instant::now() < deadline, "la fin n'est jamais signalée");
            thread::sleep(Duration::from_millis(20));
        }
        assert!(engine.take_error().is_none());
    }
}
