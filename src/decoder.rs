//! Reads an audio file into chunks of interleaved f32 samples.

use std::fs::File;
use std::path::PathBuf;

use anyhow::{Context, Result, anyhow};
use symphonia::core::codecs::audio::{AudioDecoder, AudioDecoderOptions};
use symphonia::core::errors::Error;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, FormatReader, SeekMode, SeekTo, TrackType};
use symphonia::core::io::{MediaSource, MediaSourceStream};
use symphonia::core::meta::MetadataOptions;
use symphonia::core::units::{Time, Timestamp};

use crate::http_source::HttpSource;

#[derive(Clone, Debug, PartialEq)]
pub enum AudioSource {
    File(PathBuf),
    Http {
        url: String,
        extension: Option<String>,
    },
}

pub struct Chunk {
    pub samples: Vec<f32>,
    pub channels: usize,
    pub rate: u32,
}

pub struct TrackDecoder {
    format: Box<dyn FormatReader>,
    decoder: Box<dyn AudioDecoder>,
    track_id: u32,
    skip_before: Option<Timestamp>,
}

impl TrackDecoder {
    pub fn open(source: &AudioSource) -> Result<Self> {
        let (media, extension): (Box<dyn MediaSource>, Option<String>) = match source {
            AudioSource::File(path) => (
                Box::new(
                    File::open(path).with_context(|| format!("ouverture de {}", path.display()))?,
                ),
                path.extension().map(|e| e.to_string_lossy().into_owned()),
            ),
            AudioSource::Http { url, extension } => (
                Box::new(HttpSource::open(url.clone()).context("connexion au serveur")?),
                extension.clone(),
            ),
        };
        let stream = MediaSourceStream::new(media, Default::default());
        let mut hint = Hint::new();
        if let Some(extension) = &extension {
            hint.with_extension(extension);
        }
        let format = symphonia::default::get_probe()
            .probe(
                &hint,
                stream,
                FormatOptions::default(),
                MetadataOptions::default(),
            )
            .context("format non reconnu")?;
        let track = format
            .default_track(TrackType::Audio)
            .ok_or_else(|| anyhow!("aucune piste audio"))?;
        let params = track
            .codec_params
            .as_ref()
            .and_then(|params| params.audio())
            .ok_or_else(|| anyhow!("paramètres audio manquants"))?;
        let decoder = symphonia::default::get_codecs()
            .make_audio_decoder(params, &AudioDecoderOptions::default())
            .context("codec non pris en charge")?;
        let track_id = track.id;
        Ok(Self {
            format,
            decoder,
            track_id,
            skip_before: None,
        })
    }

    /// Returns the next decoded chunk, or `None` at the end of the file.
    pub fn next_chunk(&mut self) -> Result<Option<Chunk>> {
        loop {
            let packet = match self.format.next_packet() {
                Ok(Some(packet)) => packet,
                Ok(None) | Err(Error::ResetRequired) => return Ok(None),
                Err(error) => return Err(error).context("lecture du fichier"),
            };
            if packet.track_id != self.track_id {
                continue;
            }
            let before_target = self.skip_before.is_some_and(|target| packet.pts < target);
            match self.decoder.decode(&packet) {
                Ok(buffer) => {
                    // Decoding what we skip keeps the decoder state valid after a seek.
                    if before_target || buffer.frames() == 0 {
                        continue;
                    }
                    self.skip_before = None;
                    let spec = buffer.spec();
                    let channels = spec.channels().count();
                    let rate = spec.rate();
                    let mut samples = Vec::new();
                    buffer.copy_to_vec_interleaved(&mut samples);
                    return Ok(Some(Chunk {
                        samples,
                        channels,
                        rate,
                    }));
                }
                Err(Error::DecodeError(_)) => continue,
                Err(error) => return Err(error).context("décodage"),
            }
        }
    }

    pub fn seek(&mut self, seconds: f64) -> Result<()> {
        let seconds = seconds.max(0.0);
        let whole = seconds.trunc();
        let nanos = (seconds.fract() * 1e9) as u32;
        let time =
            Time::try_new(whole as i64, nanos).ok_or_else(|| anyhow!("position invalide"))?;
        let seeked = self
            .format
            .seek(
                SeekMode::Accurate,
                SeekTo::Time {
                    time,
                    track_id: Some(self.track_id),
                },
            )
            .context("déplacement dans le fichier")?;
        self.decoder.reset();
        self.skip_before = Some(seeked.required_ts);
        Ok(())
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use std::path::PathBuf;

    /// Writes a 16-bit PCM WAV under `target/`, a sine or silence.
    pub fn write_wav(name: &str, rate: u32, channels: u16, seconds: f32, tone: bool) -> PathBuf {
        let frames = (rate as f32 * seconds) as u32;
        let bytes = frames * u32::from(channels) * 2;
        let mut out = Vec::with_capacity(44 + bytes as usize);
        out.extend(b"RIFF");
        out.extend((36 + bytes).to_le_bytes());
        out.extend(b"WAVEfmt ");
        out.extend(16u32.to_le_bytes());
        out.extend(1u16.to_le_bytes());
        out.extend(channels.to_le_bytes());
        out.extend(rate.to_le_bytes());
        out.extend((rate * u32::from(channels) * 2).to_le_bytes());
        out.extend((channels * 2).to_le_bytes());
        out.extend(16u16.to_le_bytes());
        out.extend(b"data");
        out.extend(bytes.to_le_bytes());
        for frame in 0..frames {
            let t = frame as f32 / rate as f32;
            let value = if tone {
                ((t * 440.0 * std::f32::consts::TAU).sin() * 8_000.0) as i16
            } else {
                0
            };
            for _ in 0..channels {
                out.extend(value.to_le_bytes());
            }
        }
        let path = PathBuf::from("target").join(name);
        std::fs::write(&path, out).unwrap();
        path
    }

    fn decode_all(decoder: &mut TrackDecoder) -> (usize, usize, u32) {
        let (mut samples, mut channels, mut rate) = (0, 0, 0);
        while let Some(chunk) = decoder.next_chunk().unwrap() {
            samples += chunk.samples.len();
            channels = chunk.channels;
            rate = chunk.rate;
        }
        (samples, channels, rate)
    }

    #[test]
    fn decodes_every_sample_of_a_wav() {
        let path = write_wav("decode.wav", 44_100, 2, 1.0, true);
        let mut decoder = TrackDecoder::open(&AudioSource::File(path)).unwrap();
        assert_eq!(decode_all(&mut decoder), (88_200, 2, 44_100));
    }

    #[test]
    fn seeking_skips_the_start() {
        let path = write_wav("seek.wav", 44_100, 1, 2.0, true);
        let mut decoder = TrackDecoder::open(&AudioSource::File(path)).unwrap();
        decoder.seek(1.5).unwrap();
        let (samples, _, _) = decode_all(&mut decoder);
        let remaining = samples as f64 / 44_100.0;
        assert!((remaining - 0.5).abs() < 0.1, "reste {remaining}s");
    }

    #[test]
    fn decodes_and_seeks_over_http() {
        let wav = std::fs::read(write_wav("http.wav", 44_100, 2, 4.0, true)).unwrap();
        let url = crate::http_source::tests::serve(wav, true);
        let source = AudioSource::Http {
            url,
            extension: Some("wav".into()),
        };
        let mut decoder = TrackDecoder::open(&source).unwrap();
        decoder.seek(3.0).unwrap();
        let (samples, channels, _) = decode_all(&mut decoder);
        let remaining = samples as f64 / f64::from(channels as u32) / 44_100.0;
        assert!((remaining - 1.0).abs() < 0.1, "reste {remaining}s");
    }

    #[test]
    fn rejects_a_file_that_is_not_audio() {
        let path = PathBuf::from("target/not-audio.mp3");
        std::fs::write(&path, b"ceci n'est pas de l'audio").unwrap();
        assert!(TrackDecoder::open(&AudioSource::File(path)).is_err());
    }
}
