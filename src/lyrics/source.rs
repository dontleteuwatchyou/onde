//! Finds lyrics for a track: files beside it, its tags, the cache, then LRCLIB.

use std::collections::hash_map::DefaultHasher;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::Path;
#[cfg(test)]
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result};
use lofty::prelude::*;
use lofty::tag::ItemKey;
use serde::Deserialize;

use super::{Lyrics, parse};
use crate::library::{Track, UNKNOWN_ARTIST};
use crate::sources::MusicSource;

const SEARCH_URL: &str = "https://lrclib.net/api/search";
const TIMEOUT: Duration = Duration::from_secs(8);
/// Results further than this from the track's duration are other recordings.
const DURATION_TOLERANCE: f64 = 3.0;
/// LRCLIB grows, so "not found" is only remembered for a while.
const NOT_FOUND_TTL: Duration = Duration::from_secs(7 * 24 * 3600);

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Origin {
    File,
    Tags,
    Lrclib,
    Server,
}

impl Origin {
    pub fn label(self) -> &'static str {
        match self {
            Origin::File => "Fichier local",
            Origin::Tags => "Tags du fichier",
            Origin::Lrclib => "LRCLIB",
            Origin::Server => "Serveur",
        }
    }
}

/// Blocking: call it off the UI thread. `Ok(None)` means none exist.
pub fn load(track: &Track, music: &dyn MusicSource) -> Result<Option<(Lyrics, Origin)>> {
    // A failure here must not prevent the online search.
    if let Ok(Some(found)) = music.lyrics(track) {
        return Ok(Some(found));
    }
    if track.artist == UNKNOWN_ARTIST {
        return Ok(None);
    }
    let Some(cache) = dirs::cache_dir().map(|dir| dir.join("onde/lyrics")) else {
        return fetch(track).map(|found| found.and_then(|l| parsed(&l)));
    };
    load_cached_or_fetch(track, &cache, fetch)
}

/// Files beside the track, then its tags.
pub fn load_local(path: &Path) -> Option<(Lyrics, Origin)> {
    for extension in ["ttml", "lrc"] {
        let sidecar = path.with_extension(extension);
        if let Some(lyrics) = fs::read_to_string(&sidecar).ok().and_then(|c| parse(&c)) {
            return Some((lyrics, Origin::File));
        }
    }
    embedded(path).map(|lyrics| (lyrics, Origin::Tags))
}

fn embedded(path: &Path) -> Option<Lyrics> {
    let file = lofty::read_from_path(path).ok()?;
    file.tags()
        .iter()
        .find_map(|tag| tag.get_string(ItemKey::Lyrics))
        .and_then(parse)
}

/// What LRCLIB said about a track, in the form the cache stores.
#[derive(Debug, PartialEq)]
enum Remote {
    Text(String),
    Instrumental,
}

fn parsed(remote: &Remote) -> Option<(Lyrics, Origin)> {
    match remote {
        Remote::Text(text) => parse(text).map(|lyrics| (lyrics, Origin::Lrclib)),
        Remote::Instrumental => Some((Lyrics::Instrumental, Origin::Lrclib)),
    }
}

fn load_cached_or_fetch(
    track: &Track,
    cache: &Path,
    fetch: impl FnOnce(&Track) -> Result<Option<Remote>>,
) -> Result<Option<(Lyrics, Origin)>> {
    let base = cache.join(cache_key(track));
    let (text, instrumental, none) = (
        base.with_extension("lrc"),
        base.with_extension("instrumental"),
        base.with_extension("none"),
    );
    if let Some(found) = fs::read_to_string(&text)
        .ok()
        .and_then(|content| parsed(&Remote::Text(content)))
    {
        return Ok(Some(found));
    }
    if instrumental.exists() {
        return Ok(parsed(&Remote::Instrumental));
    }
    if is_recent(&none) {
        return Ok(None);
    }
    let remote = fetch(track)?;
    // A cache that can't be written only costs a new request next time.
    let _ = fs::create_dir_all(cache);
    match &remote {
        Some(Remote::Text(content)) => drop(fs::write(&text, content)),
        Some(Remote::Instrumental) => drop(fs::write(&instrumental, "")),
        None => drop(fs::write(&none, "")),
    }
    Ok(remote.as_ref().and_then(parsed))
}

fn is_recent(path: &Path) -> bool {
    fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|modified| SystemTime::now().duration_since(modified).ok())
        .is_some_and(|age| age < NOT_FOUND_TTL)
}

fn cache_key(track: &Track) -> String {
    let mut hasher = DefaultHasher::new();
    track.artist.to_lowercase().hash(&mut hasher);
    track.title.to_lowercase().hash(&mut hasher);
    (track.duration as u64).hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
struct Candidate {
    #[serde(default)]
    duration: f64,
    #[serde(default)]
    instrumental: bool,
    plain_lyrics: Option<String>,
    synced_lyrics: Option<String>,
}

fn fetch(track: &Track) -> Result<Option<Remote>> {
    let agent = ureq::Agent::new_with_config(
        ureq::Agent::config_builder()
            .timeout_global(Some(TIMEOUT))
            .user_agent(concat!("onde/", env!("CARGO_PKG_VERSION")))
            .build(),
    );
    let candidates: Vec<Candidate> = agent
        .get(SEARCH_URL)
        .query("track_name", &track.title)
        .query("artist_name", &track.artist)
        .call()
        .context("LRCLIB injoignable")?
        .body_mut()
        .read_json()
        .context("réponse LRCLIB illisible")?;
    Ok(choose(candidates, track.duration))
}

/// Prefers word timing, then line timing, then plain text, then "instrumental",
/// and keeps only results that match the recording's duration.
fn choose(candidates: Vec<Candidate>, duration: f64) -> Option<Remote> {
    let matching = |candidate: &Candidate| {
        duration <= 0.0 || (candidate.duration - duration).abs() <= DURATION_TOLERANCE
    };
    let rank = |candidate: &Candidate| {
        let synced = candidate
            .synced_lyrics
            .as_deref()
            .filter(|s| !s.trim().is_empty());
        match synced {
            Some(text) if has_word_timing(text) => 3,
            Some(_) => 2,
            None if candidate
                .plain_lyrics
                .as_deref()
                .is_some_and(|p| !p.trim().is_empty()) =>
            {
                1
            }
            None => 0,
        }
    };
    let best = candidates
        .into_iter()
        .filter(matching)
        .filter(|c| c.instrumental || rank(c) > 0)
        .max_by_key(rank)?;
    Some(match best {
        Candidate {
            synced_lyrics: Some(text),
            ..
        } if !text.trim().is_empty() => Remote::Text(text),
        Candidate {
            plain_lyrics: Some(text),
            ..
        } if !text.trim().is_empty() => Remote::Text(text),
        _ => Remote::Instrumental,
    })
}

fn has_word_timing(lrc: &str) -> bool {
    matches!(parse(lrc), Some(Lyrics::Synced(lines)) if lines.iter().any(|l| !l.words.is_empty()))
}

/// For tests: a fresh folder under `target/`.
#[cfg(test)]
fn test_dir(name: &str) -> PathBuf {
    let dir = PathBuf::from("target/lyrics-tests").join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(path: PathBuf) -> Track {
        Track {
            location: crate::library::Location::File(path),
            title: "Titre".into(),
            artist: "Artiste".into(),
            album: String::new(),
            disc: 0,
            number: 0,
            duration: 200.0,
            extension: None,
            format: String::new(),
            added: 0,
        }
    }

    fn candidate(duration: f64, synced: Option<&str>, plain: Option<&str>) -> Candidate {
        Candidate {
            duration,
            instrumental: false,
            plain_lyrics: plain.map(str::to_owned),
            synced_lyrics: synced.map(str::to_owned),
        }
    }

    #[test]
    fn a_ttml_sidecar_beats_an_lrc_one() {
        let dir = test_dir("sidecar");
        let audio = dir.join("a.mp3");
        fs::write(dir.join("a.lrc"), "[00:01.00]depuis lrc").unwrap();
        fs::write(
            dir.join("a.ttml"),
            r#"<tt><body><div><p begin="1s" end="2s">depuis ttml</p></div></body></tt>"#,
        )
        .unwrap();
        let (lyrics, origin) = load_local(&audio).unwrap();
        assert_eq!(origin, Origin::File);
        let Lyrics::Synced(lines) = lyrics else {
            panic!()
        };
        assert_eq!(lines[0].text, "depuis ttml");
    }

    #[test]
    fn fetches_once_then_serves_from_the_cache() {
        let dir = test_dir("cache");
        let t = track(dir.join("a.mp3"));
        let first = load_cached_or_fetch(&t, &dir, |_| {
            Ok(Some(Remote::Text("[00:01.00]Salut".into())))
        })
        .unwrap();
        assert!(matches!(first, Some((Lyrics::Synced(_), Origin::Lrclib))));
        let second = load_cached_or_fetch(&t, &dir, |_| panic!("ne doit pas refaire la requête"));
        assert!(matches!(second, Ok(Some((Lyrics::Synced(_), _)))));
    }

    #[test]
    fn remembers_not_found_and_instrumental() {
        let dir = test_dir("markers");
        let t = track(dir.join("a.mp3"));
        assert!(
            load_cached_or_fetch(&t, &dir, |_| Ok(None))
                .unwrap()
                .is_none()
        );
        let again = load_cached_or_fetch(&t, &dir, |_| panic!("ne doit pas refaire la requête"));
        assert!(again.unwrap().is_none());

        let mut other = track(dir.join("b.mp3"));
        other.title = "Autre".into();
        load_cached_or_fetch(&other, &dir, |_| Ok(Some(Remote::Instrumental))).unwrap();
        let cached =
            load_cached_or_fetch(&other, &dir, |_| panic!("ne doit pas refaire la requête"));
        assert!(matches!(cached, Ok(Some((Lyrics::Instrumental, _)))));
    }

    #[test]
    fn a_network_error_is_not_cached() {
        let dir = test_dir("errors");
        let t = track(dir.join("a.mp3"));
        assert!(load_cached_or_fetch(&t, &dir, |_| anyhow::bail!("réseau")).is_err());
        let retry = load_cached_or_fetch(&t, &dir, |_| Ok(Some(Remote::Instrumental)));
        assert!(matches!(retry, Ok(Some((Lyrics::Instrumental, _)))));
    }

    #[test]
    fn choose_rejects_another_recording() {
        assert_eq!(
            choose(vec![candidate(260.0, Some("[00:01.00]a"), None)], 200.0),
            None
        );
    }

    #[test]
    fn choose_prefers_the_most_precise_timing() {
        let plain = candidate(200.0, None, Some("texte"));
        let line = candidate(201.0, Some("[00:01.00]ligne"), None);
        let word = candidate(199.0, Some("[00:01.00]<00:01.00>mot<00:02.00>"), None);
        let best = choose(vec![plain, word, line], 200.0).unwrap();
        assert_eq!(
            best,
            Remote::Text("[00:01.00]<00:01.00>mot<00:02.00>".into())
        );
    }

    #[test]
    fn choose_uses_lyrics_over_an_instrumental_result() {
        let mut instrumental = candidate(200.0, None, None);
        instrumental.instrumental = true;
        let plain = candidate(200.0, None, Some("texte"));
        assert_eq!(
            choose(vec![instrumental, plain], 200.0),
            Some(Remote::Text("texte".into()))
        );
    }

    #[test]
    fn choose_reports_instrumental_when_that_is_all_there_is() {
        let mut instrumental = candidate(200.0, None, None);
        instrumental.instrumental = true;
        assert_eq!(
            choose(vec![instrumental], 200.0),
            Some(Remote::Instrumental)
        );
    }

    #[test]
    fn choose_ignores_empty_results() {
        assert_eq!(
            choose(vec![candidate(200.0, Some("  "), Some(""))], 200.0),
            None
        );
    }

    /// Calls the real service. Run with `cargo test -- --ignored`.
    #[test]
    #[ignore = "nécessite le réseau"]
    fn finds_a_known_instrumental_on_lrclib() {
        let mut t = track(PathBuf::from("x.mp3"));
        t.title = "Clair De Lune".into();
        t.artist = "Debussy".into();
        t.duration = 310.0;
        assert!(fetch(&t).unwrap().is_some());
    }
}
