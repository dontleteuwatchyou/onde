//! Local integrity check and measured spectrum. Never certifies source provenance.
use crate::decoder::{AudioSource, TrackDecoder};
use anyhow::{Result, bail};
use rustfft::{FftPlanner, num_complex::Complex};
use std::{
    fs::File,
    io::Read,
    path::Path,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

pub struct Report {
    pub summary: String,
    pub rate: u32,
    pub seconds: f64,
    pub spectrum: Vec<f32>,
    pub history: Vec<Vec<f32>>,
}

pub fn inspect(path: &Path) -> Result<Report> {
    let mut magic = [0; 4];
    File::open(path)?.read_exact(&mut magic)?;
    let integrity = if &magic == b"fLaC" {
        let mut child = Command::new("flac")
            .args(["--test", "--silent", "--"])
            .arg(path)
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()?;
        let mut stderr = child.stderr.take().unwrap();
        let reader = std::thread::spawn(move || {
            let mut text = Vec::new();
            let mut block = [0u8; 4096];
            while let Ok(count) = stderr.read(&mut block) {
                if count == 0 {
                    break;
                }
                let retain = count.min(8192usize.saturating_sub(text.len()));
                text.extend_from_slice(&block[..retain]);
            }
            String::from_utf8_lossy(&text).trim().to_owned()
        });
        let start = Instant::now();
        let status = loop {
            if let Some(status) = child.try_wait()? {
                break status;
            }
            if start.elapsed() > Duration::from_secs(120) {
                let _ = child.kill();
                let _ = child.wait();
                bail!("Test FLAC interrompu après 120 secondes.")
            }
            std::thread::sleep(Duration::from_millis(50));
        };
        let diagnostic = reader.join().unwrap_or_default();
        if !status.success() {
            bail!("FLAC : erreur réelle signalée par le décodeur : {diagnostic}");
        }
        if diagnostic.contains("MD5") && diagnostic.contains("unset") {
            "FLAC : décodage complet réussi. Empreinte MD5 absente : intégrité MD5 non vérifiable."
                .to_owned()
        } else if !diagnostic.is_empty() {
            format!("FLAC : décodage terminé avec avertissement : {diagnostic}")
        } else {
            "FLAC natif : test complet flac -t réussi.".to_owned()
        }
    } else {
        "Ce fichier n’est pas un FLAC natif (signature différente).".to_owned()
    };
    let mut decoder = TrackDecoder::open(&AudioSource::File(path.to_owned()))?;
    const N: usize = 4096;
    const BANDS: usize = 128;
    let fft = FftPlanner::<f32>::new().plan_fft_forward(N);
    let mut window = Vec::with_capacity(N);
    let mut buffer = vec![Complex::default(); N];
    let mut sum = vec![0f64; BANDS];
    let mut history = Vec::new();
    let mut rate = 0;
    let mut frames = 0u64;
    let mut windows = 0;
    // First channel avoids phase cancellation from stereo downmixing.
    while let Some(chunk) = decoder.next_chunk()? {
        if rate != 0 && rate != chunk.rate {
            bail!("Fréquence variable non prise en charge")
        }
        rate = chunk.rate;
        for frame in chunk.samples.chunks_exact(chunk.channels) {
            frames += 1;
            window.push(frame[0]);
            if window.len() == N {
                for i in 0..N {
                    buffer[i] = Complex::new(
                        window[i]
                            * (0.5
                                - 0.5 * (std::f32::consts::TAU * i as f32 / (N - 1) as f32).cos()),
                        0.,
                    );
                }
                fft.process(&mut buffer);
                let mut column = vec![0f32; BANDS];
                for (band, value) in column.iter_mut().enumerate() {
                    let first = band * (N / 2) / BANDS;
                    let last = (band + 1) * (N / 2) / BANDS;
                    let power: f32 = buffer[first..last]
                        .iter()
                        .map(|v| v.norm_sqr() / (N * N) as f32)
                        .sum();
                    sum[band] += power as f64;
                    *value = power;
                }
                windows += 1;
                // Retain one window per second, at most 300 columns.
                if history.len() < (frames / rate as u64) as usize {
                    history.push(column);
                }
                window.clear();
            }
        }
        if frames >= rate as u64 * 300 {
            break;
        }
    }
    if windows == 0 {
        bail!("Audio trop court pour l’analyse spectrale")
    }
    let db = |power: f64| (10. * power.max(1e-12).log10()).clamp(-120., 0.) as f32;
    let spectrum = sum.iter().map(|p| db(p / windows as f64)).collect();
    for column in &mut history {
        for p in column {
            *p = db(*p as f64);
        }
    }
    Ok(Report {
        summary: format!(
            "{integrity} L’origine sans perte reste indéterminée. Une coupure spectrale peut aussi venir du mastering ou de l’enregistrement."
        ),
        rate,
        seconds: frames as f64 / rate as f64,
        spectrum,
        history,
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn verifies_flac_and_rejects_corruption() {
        let wav = crate::decoder::tests::write_wav("quality-flac", 44100, 1, 1., true);
        let path = wav.with_extension("flac");
        let status = std::process::Command::new("flac")
            .args(["--silent", "--force", "-o"])
            .arg(&path)
            .arg(wav)
            .status()
            .unwrap();
        assert!(status.success());
        assert!(
            super::inspect(&path)
                .unwrap()
                .summary
                .contains("test complet")
        );
        let mut bytes = std::fs::read(&path).unwrap();
        bytes[26..42].fill(0); // STREAMINFO MD5: absent, not corrupted audio.
        std::fs::write(&path, bytes).unwrap();
        let report = super::inspect(&path).unwrap();
        assert!(report.summary.contains("MD5 absente"));
        assert!(!report.spectrum.is_empty());
        std::fs::write(&path, b"fLaCinvalid").unwrap();
        assert!(super::inspect(&path).is_err());
    }

    #[test]
    fn wav_is_not_mistaken_for_flac_and_tone_is_visible() {
        let path = crate::decoder::tests::write_wav("quality-tone", 44100, 1, 2., true);
        let report = super::inspect(&path).unwrap();
        assert!(report.summary.contains("n’est pas un FLAC"));
        assert_eq!(report.spectrum.len(), 128);
        assert!(!report.history.is_empty());
        assert!(report.spectrum.iter().any(|x| *x > -60.));
    }
}
