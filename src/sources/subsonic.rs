//! Navidrome and other servers speaking the Subsonic API.

use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};

use super::MusicSource;
use crate::decoder::AudioSource;
use crate::library::{Location, Track, UNKNOWN_ARTIST, format_label};
use crate::lyrics::source::Origin;
use crate::lyrics::{Line, Lyrics};

const API_VERSION: &str = "1.16.1";
const CLIENT_NAME: &str = "onde";
const PAGE_SIZE: usize = 500;
const TIMEOUT: Duration = Duration::from_secs(30);
const LAST_LINE_MS: u64 = 5_000;

/// Only the salted hash is kept, never the password.
#[derive(Clone, Serialize, Deserialize, PartialEq, Debug)]
pub struct Credentials {
    pub url: String,
    pub user: String,
    pub salt: String,
    pub token: String,
}

impl Credentials {
    pub fn new(url: &str, user: &str, password: &str) -> Result<Self> {
        let url = url.trim().trim_end_matches('/');
        if !(url.starts_with("http://") || url.starts_with("https://")) {
            bail!("l'adresse doit commencer par http:// ou https://");
        }
        let mut bytes = [0_u8; 16];
        getrandom::fill(&mut bytes).map_err(|_| anyhow!("générateur aléatoire indisponible"))?;
        let salt: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
        let token = hash(password, &salt);
        Ok(Self {
            url: url.to_owned(),
            user: user.to_owned(),
            salt,
            token,
        })
    }
}

/// The token Subsonic expects: md5(password + salt).
fn hash(password: &str, salt: &str) -> String {
    format!("{:x}", md5::compute(format!("{password}{salt}")))
}

/// Keeps unreserved characters as they are, as RFC 3986 asks.
fn encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

const COVER_PIXELS: &str = "300";
const COVER_MAX_BYTES: u64 = 20 * 1024 * 1024;
const RESCAN_INTERVAL: std::time::Duration = std::time::Duration::from_secs(1);
const RESCAN_CHECKS: usize = 120;

pub struct Subsonic {
    credentials: Credentials,
    agent: ureq::Agent,
}

#[derive(Deserialize)]
struct Envelope {
    #[serde(rename = "subsonic-response")]
    response: Response,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct Response {
    status: String,
    error: Option<ApiError>,
    search_result3: Option<SearchResult>,
    lyrics_list: Option<LyricsList>,
    scan_status: Option<ScanStatus>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct ScanStatus {
    scanning: bool,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct ApiError {
    message: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct SearchResult {
    song: Vec<Song>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct Song {
    id: String,
    title: String,
    artist: Option<String>,
    album: String,
    disc_number: u32,
    track: u32,
    duration: f64,
    suffix: Option<String>,
    bit_rate: Option<u32>,
    bit_depth: Option<u32>,
    sampling_rate: Option<u32>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct LyricsList {
    structured_lyrics: Vec<StructuredLyrics>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct StructuredLyrics {
    synced: bool,
    line: Vec<LyricLine>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct LyricLine {
    start: Option<u64>,
    value: String,
}

impl Subsonic {
    pub fn new(credentials: Credentials) -> Self {
        let agent = ureq::Agent::new_with_config(
            ureq::Agent::config_builder()
                .timeout_global(Some(TIMEOUT))
                .http_status_as_error(false)
                .build(),
        );
        Self { credentials, agent }
    }

    /// Checks the address and the credentials.
    pub fn ping(&self) -> Result<()> {
        self.call("ping", &[]).map(drop)
    }

    fn url(&self, method: &str, params: &[(&str, &str)]) -> String {
        let c = &self.credentials;
        let mut url = format!(
            "{}/rest/{method}?u={}&t={}&s={}&v={API_VERSION}&c={CLIENT_NAME}&f=json",
            c.url,
            encode(&c.user),
            c.token,
            c.salt
        );
        for (key, value) in params {
            url.push_str(&format!("&{key}={}", encode(value)));
        }
        url
    }

    /// Errors never quote the URL: it carries the credentials.
    fn call(&self, method: &str, params: &[(&str, &str)]) -> Result<Response> {
        let mut reply = self
            .agent
            .get(&self.url(method, params))
            .call()
            .map_err(|_| anyhow!("serveur injoignable"))?;
        let status = reply.status().as_u16();
        if status != 200 {
            bail!("le serveur a répondu {status}");
        }
        let envelope: Envelope = reply
            .body_mut()
            .read_json()
            .map_err(|_| anyhow!("ce serveur ne ressemble pas à une API Subsonic"))?;
        let response = envelope.response;
        if response.status != "ok" {
            let message = response.error.map_or(String::new(), |error| error.message);
            bail!("le serveur a refusé : {message}");
        }
        Ok(response)
    }
}

fn to_track(song: Song) -> Track {
    Track {
        location: Location::Remote(song.id),
        title: song.title,
        artist: song
            .artist
            .filter(|artist| !artist.trim().is_empty())
            .unwrap_or_else(|| UNKNOWN_ARTIST.into()),
        album: song.album,
        disc: song.disc_number,
        number: song.track,
        duration: song.duration,
        format: format_label(
            song.suffix.as_deref().unwrap_or_default(),
            song.bit_rate,
            song.bit_depth,
            song.sampling_rate,
        ),
        extension: song.suffix,
    }
}

fn to_lyrics(list: LyricsList) -> Option<Lyrics> {
    let usable = |lyrics: &&StructuredLyrics| !lyrics.line.is_empty();
    let chosen = list
        .structured_lyrics
        .iter()
        .filter(usable)
        .find(|lyrics| lyrics.synced)
        .or_else(|| list.structured_lyrics.iter().find(|l| !l.line.is_empty()))?;
    if !chosen.synced {
        return Some(Lyrics::Plain(
            chosen.line.iter().map(|line| line.value.clone()).collect(),
        ));
    }
    let starts: Vec<u64> = chosen.line.iter().map(|l| l.start.unwrap_or(0)).collect();
    let lines = chosen
        .line
        .iter()
        .enumerate()
        .map(|(index, line)| Line {
            start_ms: starts[index],
            end_ms: starts
                .get(index + 1)
                .copied()
                .unwrap_or(starts[index] + LAST_LINE_MS),
            text: line.value.clone(),
            words: Vec::new(),
        })
        .collect();
    Some(Lyrics::Synced(lines))
}

impl MusicSource for Subsonic {
    fn list_tracks(&self) -> Result<Vec<Track>> {
        let mut tracks = Vec::new();
        loop {
            let offset = tracks.len().to_string();
            let count = PAGE_SIZE.to_string();
            let page = self
                .call(
                    "search3",
                    &[
                        ("query", ""),
                        ("artistCount", "0"),
                        ("albumCount", "0"),
                        ("songCount", &count),
                        ("songOffset", &offset),
                    ],
                )
                .context("lecture de la bibliothèque")?;
            let songs = page.search_result3.unwrap_or_default().song;
            let received = songs.len();
            tracks.extend(songs.into_iter().map(to_track));
            if received < PAGE_SIZE {
                break;
            }
        }
        tracks.sort_by_cached_key(|t| {
            (
                t.artist.to_lowercase(),
                t.album.to_lowercase(),
                t.disc,
                t.number,
                t.title.to_lowercase(),
            )
        });
        Ok(tracks)
    }

    fn audio(&self, track: &Track) -> Result<AudioSource> {
        let Location::Remote(id) = &track.location else {
            bail!("ce morceau ne vient pas du serveur");
        };
        // `raw` keeps the original file: no transcoding, and ranges work.
        Ok(AudioSource::Http {
            url: self.url("stream", &[("id", id), ("format", "raw")]),
            extension: track.extension.clone(),
        })
    }

    fn cover(&self, track: &Track) -> Result<Option<Vec<u8>>> {
        let Location::Remote(id) = &track.location else {
            return Ok(None);
        };
        let mut reply = self
            .agent
            .get(&self.url("getCoverArt", &[("id", id), ("size", COVER_PIXELS)]))
            .call()
            .map_err(|_| anyhow!("serveur injoignable"))?;
        let is_image = reply
            .headers()
            .get("content-type")
            .and_then(|kind| kind.to_str().ok())
            .is_some_and(|kind| kind.starts_with("image/"));
        if reply.status().as_u16() != 200 || !is_image {
            return Ok(None);
        }
        let image = reply
            .body_mut()
            .with_config()
            .limit(COVER_MAX_BYTES)
            .read_to_vec()
            .map_err(|_| anyhow!("pochette illisible"))?;
        Ok(Some(image))
    }

    /// Needs an admin account, otherwise the server refuses and scans on its own schedule.
    fn rescan(&self) -> Result<()> {
        self.call("startScan", &[])?;
        for _ in 0..RESCAN_CHECKS {
            std::thread::sleep(RESCAN_INTERVAL);
            let status = self.call("getScanStatus", &[])?;
            if !status.scan_status.is_some_and(|scan| scan.scanning) {
                break;
            }
        }
        Ok(())
    }

    fn lyrics(&self, track: &Track) -> Result<Option<(Lyrics, Origin)>> {
        let Location::Remote(id) = &track.location else {
            return Ok(None);
        };
        // Not every server has this OpenSubsonic extension: a refusal means none.
        let Ok(response) = self.call("getLyricsBySongId", &[("id", id)]) else {
            return Ok(None);
        };
        Ok(response
            .lyrics_list
            .and_then(to_lyrics)
            .map(|lyrics| (lyrics, Origin::Server)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_the_example_from_the_subsonic_documentation() {
        assert_eq!(hash("sesame", "c19b2d"), "26719a1196d2a940705a59634eb18eab");
    }

    #[test]
    fn credentials_never_contain_the_password() {
        let c = Credentials::new("https://music.test/", "moi", "motdepasse").unwrap();
        assert_eq!(c.url, "https://music.test");
        assert_eq!(c.salt.len(), 32);
        assert_eq!(c.token, hash("motdepasse", &c.salt));
        assert!(!format!("{c:?}").contains("motdepasse"));
    }

    #[test]
    fn each_login_gets_a_new_salt() {
        let a = Credentials::new("http://a.test", "u", "p").unwrap();
        let b = Credentials::new("http://a.test", "u", "p").unwrap();
        assert_ne!(a.salt, b.salt);
    }

    #[test]
    fn rejects_an_address_without_scheme() {
        assert!(Credentials::new("music.test", "u", "p").is_err());
    }

    #[test]
    fn encodes_query_values() {
        assert_eq!(encode("a b&c=é"), "a%20b%26c%3D%C3%A9");
        assert_eq!(encode("abc-_.~9"), "abc-_.~9");
    }

    fn lyric_list(json: &str) -> LyricsList {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn prefers_synced_lyrics_and_derives_line_ends() {
        let list = lyric_list(
            r#"{"structuredLyrics":[
                {"synced":false,"line":[{"value":"texte"}]},
                {"synced":true,"line":[{"start":1000,"value":"a"},{"start":3000,"value":"b"}]}
            ]}"#,
        );
        let Some(Lyrics::Synced(lines)) = to_lyrics(list) else {
            panic!()
        };
        assert_eq!((lines[0].start_ms, lines[0].end_ms), (1000, 3000));
        assert_eq!(lines[1].end_ms, 8000);
    }

    #[test]
    fn falls_back_to_plain_and_to_nothing() {
        let plain = lyric_list(r#"{"structuredLyrics":[{"synced":false,"line":[{"value":"x"}]}]}"#);
        assert_eq!(to_lyrics(plain), Some(Lyrics::Plain(vec!["x".into()])));
        let empty = lyric_list(r#"{"structuredLyrics":[{"synced":true,"line":[]}]}"#);
        assert_eq!(to_lyrics(empty), None);
    }

    /// New file on a real server: `ONDE_TEST_SERVER`, `ONDE_TEST_USER`,
    /// `ONDE_TEST_PASSWORD`, and `ONDE_TEST_NEW_TITLE` the title dropped in just before.
    #[test]
    #[ignore = "nécessite un serveur Navidrome de test"]
    fn real_rescan_finds_a_new_file_and_its_cover() {
        let env = |name: &str| std::env::var(name).unwrap();
        let server = Subsonic::new(
            Credentials::new(
                &env("ONDE_TEST_SERVER"),
                &env("ONDE_TEST_USER"),
                &env("ONDE_TEST_PASSWORD"),
            )
            .unwrap(),
        );
        server.rescan().unwrap();
        let tracks = server.list_tracks().unwrap();
        let track = tracks
            .iter()
            .find(|track| track.title == env("ONDE_TEST_NEW_TITLE"))
            .expect("nouveau morceau introuvable");
        let image = server.cover(track).unwrap().expect("pochette absente");
        assert!(image.len() > 1000, "{} octets", image.len());
    }

    /// Runs against a real server, using the saved login. Decodes only: no window, no sound.
    #[test]
    #[ignore = "nécessite un serveur Navidrome configuré"]
    fn plays_every_track_of_a_real_server() {
        use crate::decoder::TrackDecoder;
        let credentials = crate::config::load()
            .navidrome
            .expect("aucun serveur configuré");
        let server = Subsonic::new(credentials);
        server.ping().unwrap();
        let tracks = server.list_tracks().unwrap();
        assert!(!tracks.is_empty());
        for track in &tracks {
            let mut decoder = TrackDecoder::open(&server.audio(track).unwrap())
                .unwrap_or_else(|e| panic!("{} : {e:#}", track.title));
            let first = decoder.next_chunk().unwrap().expect("aucun audio");
            assert!(first.channels > 0 && first.rate > 0);
            decoder
                .seek(track.duration - 2.0)
                .unwrap_or_else(|e| panic!("{} : {e:#}", track.title));
            let mut samples = 0;
            while let Some(chunk) = decoder.next_chunk().unwrap() {
                samples += chunk.samples.len() / chunk.channels;
            }
            let remaining = samples as f64 / f64::from(first.rate);
            println!(
                "{} ({:?}) : {remaining:.2} s après déplacement",
                track.title, track.extension
            );
            assert!(
                (remaining - 2.0).abs() < 0.5,
                "{} : reste {remaining}s",
                track.title
            );
        }
        let with_lyrics: Vec<_> = tracks
            .iter()
            .filter_map(|t| {
                server
                    .lyrics(t)
                    .unwrap()
                    .map(|(l, o)| (t.title.clone(), l, o))
            })
            .collect();
        println!(
            "paroles : {:?}",
            with_lyrics
                .iter()
                .map(|(t, _, o)| (t, o))
                .collect::<Vec<_>>()
        );
        assert_eq!(with_lyrics.len(), 1);
        assert!(matches!(&with_lyrics[0].1, Lyrics::Synced(lines) if lines[0].start_ms == 1000));
    }

    #[test]
    fn a_song_without_artist_is_marked_unknown() {
        let track = to_track(serde_json::from_str(r#"{"id":"1","title":"T"}"#).unwrap());
        assert_eq!(track.artist, UNKNOWN_ARTIST);
        assert_eq!(track.location, Location::Remote("1".into()));
    }
}
