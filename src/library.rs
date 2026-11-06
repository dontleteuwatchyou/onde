//! Finds audio files in a folder and reads their tags.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use lofty::config::WriteOptions;
use lofty::picture::{MimeType, Picture, PictureType};
use lofty::prelude::*;
use walkdir::WalkDir;

// Symphonia has no Opus decoder, so `.opus` is left out on purpose.
const AUDIO_EXTENSIONS: &[&str] = &[
    "mp3", "flac", "ogg", "oga", "m4a", "mp4", "aac", "wav", "aif", "aiff", "caf",
];

pub const UNKNOWN_ARTIST: &str = "Artiste inconnu";

#[derive(Clone, Debug, PartialEq)]
pub enum Location {
    File(PathBuf),
    /// An identifier only its source understands.
    Remote(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Track {
    pub location: Location,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub disc: u32,
    pub number: u32,
    pub duration: f64,
    /// Helps recognise the format of a remote file.
    pub extension: Option<String>,
    /// How it is encoded, for display: « FLAC 24 bits 96 kHz », « M4A 128 kb/s ».
    pub format: String,
}

const COVER_NAMES: &[&str] = &[
    "cover.jpg",
    "cover.png",
    "folder.jpg",
    "folder.png",
    "front.jpg",
    "front.png",
];

pub fn scan(folder: &Path) -> Vec<Track> {
    let mut tracks: Vec<Track> = WalkDir::new(folder)
        .follow_links(true)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file() && is_audio(entry.path()))
        .map(|entry| read_track(entry.path()))
        .collect();
    tracks.sort_by_cached_key(|t| {
        (
            t.artist.to_lowercase(),
            t.album.to_lowercase(),
            t.disc,
            t.number,
            t.title.to_lowercase(),
        )
    });
    tracks
}

/// The image inside the file, otherwise one beside it.
pub fn read_cover(path: &Path) -> Option<Vec<u8>> {
    embedded_cover(path).or_else(|| sidecar_cover(path))
}

fn embedded_cover(path: &Path) -> Option<Vec<u8>> {
    let file = lofty::read_from_path(path).ok()?;
    file.tags()
        .iter()
        .flat_map(|tag| tag.pictures())
        .min_by_key(|picture| picture.pic_type() != PictureType::CoverFront)
        .map(|picture| picture.data().to_vec())
}

fn sidecar_cover(path: &Path) -> Option<Vec<u8>> {
    let folder = path.parent()?;
    let beside = ["jpg", "jpeg", "png", "webp"]
        .iter()
        .map(|extension| path.with_extension(extension));
    let shared = COVER_NAMES.iter().map(|name| folder.join(name));
    beside
        .chain(shared)
        .find_map(|image| std::fs::read(image).ok())
}

pub fn embed_cover(audio: &Path, image: &Path) -> Result<()> {
    let data = std::fs::read(image).context("lecture de l'image")?;
    let picture = Picture::unchecked(data)
        .pic_type(PictureType::CoverFront)
        .mime_type(MimeType::Jpeg)
        .build();
    let mut file = lofty::read_from_path(audio).context("lecture des tags")?;
    if file.primary_tag().is_none() {
        file.insert_tag(lofty::tag::Tag::new(file.primary_tag_type()));
    }
    let tag = file.primary_tag_mut().context("tags indisponibles")?;
    tag.remove_picture_type(PictureType::CoverFront);
    tag.push_picture(picture);
    tag.save_to_path(audio, WriteOptions::default())
        .context("écriture des tags")?;
    Ok(())
}

fn is_audio(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| AUDIO_EXTENSIONS.contains(&e.to_lowercase().as_str()))
}

/// A file with unreadable tags stays listed, named after the file.
fn read_track(path: &Path) -> Track {
    let mut track = Track {
        location: Location::File(path.to_path_buf()),
        title: path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default(),
        artist: UNKNOWN_ARTIST.into(),
        album: String::new(),
        disc: 0,
        number: 0,
        duration: 0.0,
        extension: None,
        format: String::new(),
    };
    let Ok(file) = lofty::read_from_path(path) else {
        return track;
    };
    let properties = file.properties();
    track.duration = properties.duration().as_secs_f64();
    track.format = format_label(
        &path
            .extension()
            .map(|e| e.to_string_lossy().into_owned())
            .unwrap_or_default(),
        properties.audio_bitrate(),
        properties.bit_depth().map(u32::from),
        properties.sample_rate(),
    );
    if let Some(tag) = file.primary_tag().or_else(|| file.first_tag()) {
        if let Some(title) = tag.title().filter(|s| !s.trim().is_empty()) {
            track.title = title.into_owned();
        }
        if let Some(artist) = tag.artist().filter(|s| !s.trim().is_empty()) {
            track.artist = artist.into_owned();
        }
        if let Some(album) = tag.album() {
            track.album = album.into_owned();
        }
        track.disc = tag.disk().unwrap_or(0);
        track.number = tag.track().unwrap_or(0);
    }
    track
}

const LOSSLESS: &[&str] = &["flac", "wav", "aif", "aiff", "alac", "ape", "wv"];

/// Lossless files are told apart by resolution, the others by bitrate.
pub fn format_label(
    extension: &str,
    kbps: Option<u32>,
    bits: Option<u32>,
    hertz: Option<u32>,
) -> String {
    let name = extension.to_uppercase();
    if LOSSLESS.contains(&extension.to_lowercase().as_str()) {
        let mut label = name;
        if let Some(bits) = bits.filter(|bits| *bits > 0) {
            label.push_str(&format!(" {bits} bits"));
        }
        if let Some(hertz) = hertz.filter(|hertz| *hertz > 0) {
            let khz = format!("{:.1}", f64::from(hertz) / 1000.0).replace(".0", "");
            label.push_str(&format!(" {} kHz", khz.replace('.', ",")));
        }
        label
    } else {
        match kbps.filter(|kbps| *kbps > 0) {
            Some(kbps) => format!("{name} {kbps} kb/s"),
            None => name,
        }
    }
}

pub fn format_time(seconds: f64) -> String {
    let total = seconds.max(0.0) as u64;
    let (hours, minutes, secs) = (total / 3600, total / 60 % 60, total % 60);
    if hours > 0 {
        format!("{hours}:{minutes:02}:{secs:02}")
    } else {
        format!("{minutes}:{secs:02}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_formats_by_what_matters_for_each() {
        let label = format_label;
        assert_eq!(
            label("flac", Some(900), Some(24), Some(96_000)),
            "FLAC 24 bits 96 kHz"
        );
        assert_eq!(
            label("FLAC", None, Some(16), Some(44_100)),
            "FLAC 16 bits 44,1 kHz"
        );
        assert_eq!(label("m4a", Some(128), None, Some(44_100)), "M4A 128 kb/s");
        assert_eq!(label("mp3", None, None, None), "MP3");
        assert_eq!(label("flac", None, None, None), "FLAC");
        assert_eq!(label("mp3", Some(0), None, None), "MP3");
    }
    use crate::decoder::tests::write_wav;

    #[test]
    fn recognises_audio_extensions_in_any_case() {
        assert!(is_audio(Path::new("a/b.FLAC")));
        assert!(!is_audio(Path::new("a/cover.jpg")));
        assert!(!is_audio(Path::new("a/sans-extension")));
    }

    #[test]
    fn scan_finds_nested_files_and_falls_back_to_the_file_name() {
        let wav = write_wav("library-source.wav", 8_000, 1, 1.0, false);
        let folder = PathBuf::from("target/library-test/album");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::copy(wav, folder.join("Mon morceau.wav")).unwrap();
        std::fs::write(folder.join("notes.txt"), "pas de l'audio").unwrap();

        let tracks = scan(Path::new("target/library-test"));
        assert_eq!(tracks.len(), 1);
        assert_eq!(tracks[0].title, "Mon morceau");
        assert_eq!(tracks[0].artist, "Artiste inconnu");
        assert!((tracks[0].duration - 1.0).abs() < 0.01);
    }

    #[test]
    fn formats_durations() {
        assert_eq!(format_time(0.0), "0:00");
        assert_eq!(format_time(65.9), "1:05");
        assert_eq!(format_time(3_725.0), "1:02:05");
    }
}
