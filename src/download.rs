//! Where music can be downloaded from, and how.

use std::fs::{self, File};
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use serde_json::Value;

use crate::library;

const RESULTS: usize = 15;
const API_TIMEOUT: Duration = Duration::from_secs(20);
const MAX_NAME_CHARS: usize = 120;
const CANCEL_CHECK: Duration = Duration::from_millis(100);

#[derive(Clone, Debug, PartialEq)]
pub struct Hit {
    pub title: String,
    pub artist: String,
    pub seconds: Option<f64>,
    /// What the source needs to fetch it again.
    pub reference: String,
}

/// Lets the screen follow a download and stop it.
#[derive(Clone, Default)]
pub struct Transfer {
    progress: Arc<AtomicU32>,
    cancelled: Arc<AtomicBool>,
}

impl Transfer {
    pub fn progress(&self) -> f32 {
        f32::from_bits(self.progress.load(Ordering::Relaxed))
    }

    fn set_progress(&self, done: f32) {
        self.progress.store(done.to_bits(), Ordering::Relaxed);
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed)
    }
}

pub trait DownloadSource: Send + Sync {
    fn name(&self) -> &'static str;
    /// Told to the user before they pick this source.
    fn quality(&self) -> &'static str;
    fn search(&self, query: &str) -> Result<Vec<Hit>>;
    /// Leaves nothing behind when cancelled.
    fn download(&self, hit: &Hit, folder: &Path, transfer: &Transfer) -> Result<()>;
}

pub fn sources() -> Vec<Arc<dyn DownloadSource>> {
    vec![
        Arc::new(YtDlp::new(
            "YouTube",
            "ytsearch",
            "Compressé, AAC 128 à 256 kb/s environ",
        )),
        Arc::new(YtDlp::new(
            "SoundCloud",
            "scsearch",
            "Compressé, selon le morceau",
        )),
        Arc::new(LiveMusicArchive),
    ]
}

/// YouTube and SoundCloud go through the external `yt-dlp` program, which is
/// kept outside the app so it can be updated on its own when those sites change.
struct YtDlp {
    program: String,
    name: &'static str,
    prefix: &'static str,
    quality: &'static str,
}

impl YtDlp {
    fn new(name: &'static str, prefix: &'static str, quality: &'static str) -> Self {
        Self {
            program: std::env::var("ONDE_YT_DLP").unwrap_or_else(|_| "yt-dlp".into()),
            name,
            prefix,
            quality,
        }
    }

    fn command(&self) -> Command {
        let mut command = Command::new(&self.program);
        command
            .arg("--ignore-config")
            .args(["--js-runtimes", "node"])
            .stdin(Stdio::null());
        command
    }
}

fn missing_program(error: std::io::Error) -> anyhow::Error {
    if error.kind() == std::io::ErrorKind::NotFound {
        anyhow!("yt-dlp n'est pas installé sur cette machine")
    } else {
        anyhow::Error::new(error).context("lancement de yt-dlp")
    }
}

/// The reason yt-dlp gives, without its noise.
fn failure_reason(stderr: &str) -> String {
    let line = stderr
        .lines()
        .rev()
        .find(|line| line.starts_with("ERROR:"))
        .or_else(|| stderr.lines().rev().find(|line| !line.trim().is_empty()))
        .unwrap_or("erreur inconnue");
    line.trim_start_matches("ERROR:")
        .trim()
        .chars()
        .take(200)
        .collect()
}

impl DownloadSource for YtDlp {
    fn name(&self) -> &'static str {
        self.name
    }

    fn quality(&self) -> &'static str {
        self.quality
    }

    fn search(&self, query: &str) -> Result<Vec<Hit>> {
        let output = self
            .command()
            .args(["--flat-playlist", "--dump-single-json", "--no-warnings"])
            .arg("--")
            .arg(format!("{}{RESULTS}:{query}", self.prefix))
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .map_err(missing_program)?;
        if !output.status.success() {
            bail!(
                "{}",
                failure_reason(&String::from_utf8_lossy(&output.stderr))
            );
        }
        let found: Value =
            serde_json::from_slice(&output.stdout).context("réponse de yt-dlp illisible")?;
        Ok(parse_ytdlp_hits(&found))
    }

    fn download(&self, hit: &Hit, folder: &Path, transfer: &Transfer) -> Result<()> {
        if !hit.reference.starts_with("https://") {
            bail!("adresse de morceau invalide");
        }
        // Work files stay out of the music folder, so a cancelled download
        // leaves nothing there and a rescan can't pick up half a file.
        let work = work_folder()?;
        let result = self.run_download(hit, folder, &work, transfer);
        let _ = fs::remove_dir_all(&work);
        if let Some(audio) = result? {
            attach_cover(&audio);
        }
        transfer.set_progress(1.0);
        Ok(())
    }
}

impl YtDlp {
    /// Returns the finished file, when yt-dlp names it.
    fn run_download(
        &self,
        hit: &Hit,
        folder: &Path,
        work: &Path,
        transfer: &Transfer,
    ) -> Result<Option<PathBuf>> {
        let mut child = self
            .command()
            // Onde has no Opus decoder: prefer what it can read as is.
            .args(["-f", "bestaudio[ext=m4a]/bestaudio[ext=mp3]/bestaudio"])
            .args(["-x", "--audio-format", "mp3>mp3/aac>m4a/m4a"])
            .args(["--audio-quality", "0"])
            .args(["--embed-metadata", "--no-playlist"])
            // Onde embeds the cover itself: yt-dlp would need an extra program
            // for that, and a missing one would fail the whole download.
            .args(["--write-thumbnail", "--convert-thumbnails", "jpg"])
            .args(["--ppa", SQUARE_COVER])
            .args([
                "--parse-metadata",
                "%(artist,uploader,channel)s:%(meta_artist)s",
            ])
            .args(["--newline", "--no-colors", "--no-warnings", "--progress"])
            .args(["--print", "after_move:onde-file %(filepath)s"])
            .args(["--progress-template", PROGRESS_TEMPLATE])
            .arg("-P")
            .arg(format!("home:{}", folder.display()))
            .arg("-P")
            .arg(format!("temp:{}", work.display()))
            .args(["-o", "%(artist,uploader,channel)s/%(title)s.%(ext)s"])
            .arg("--")
            .arg(&hit.reference)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            // Its own group, so cancelling also stops what it started (ffmpeg).
            .process_group(0)
            .spawn()
            .map_err(missing_program)?;
        let stdout = child.stdout.take().context("sortie de yt-dlp")?;
        let mut stderr = child.stderr.take().context("sortie de yt-dlp")?;
        // Read in parallel: a full pipe would block yt-dlp.
        let errors = std::thread::spawn(move || {
            let mut text = String::new();
            let _ = stderr.read_to_string(&mut text);
            text
        });
        let finished = Arc::new(AtomicBool::new(false));
        let watcher = {
            let (finished, transfer, group) = (finished.clone(), transfer.clone(), child.id());
            std::thread::spawn(move || {
                while !finished.load(Ordering::Relaxed) {
                    if transfer.is_cancelled() {
                        let _ = Command::new("kill")
                            .args(["-TERM", "--", &format!("-{group}")])
                            .stderr(Stdio::null())
                            .status();
                        return;
                    }
                    std::thread::sleep(CANCEL_CHECK);
                }
            })
        };
        let mut file = None;
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if let Some(done) = parse_progress(&line) {
                transfer.set_progress(done);
            } else if let Some(path) = line.strip_prefix("onde-file ") {
                file = Some(PathBuf::from(path));
            }
        }
        // The output closed: the watcher must stop before the process is reaped,
        // or its number could be given to another one.
        finished.store(true, Ordering::Relaxed);
        let _ = watcher.join();
        let status = child.wait().context("attente de yt-dlp")?;
        let errors = errors.join().unwrap_or_default();
        if transfer.is_cancelled() {
            bail!("annulé");
        }
        if !status.success() {
            bail!("{}", failure_reason(&errors));
        }
        Ok(file)
    }
}

/// Crops video thumbnails to the center square.
const SQUARE_COVER: &str = "ThumbnailsConvertor+ffmpeg_o:-c:v mjpeg -qmin 1 -qscale:v 1 -vf crop=\"'if(gt(ih,iw),iw,ih)':'if(gt(iw,ih),ih,iw)'\"";

fn work_folder() -> Result<PathBuf> {
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    let folder = dirs::cache_dir()
        .context("dossier de cache introuvable")?
        .join(format!(
            "onde/telechargement-{}-{unique}",
            std::process::id()
        ));
    fs::create_dir_all(&folder).context("création du dossier de travail")?;
    Ok(folder)
}

/// Moves the image beside the file into its tags. A file that can't take it
/// keeps its image beside it, where Onde also looks.
fn attach_cover(audio: &Path) {
    let image = audio.with_extension("jpg");
    if library::embed_cover(audio, &image).is_ok() {
        let _ = fs::remove_file(&image);
    }
}

const PROGRESS_TEMPLATE: &str = "download:onde %(progress.downloaded_bytes)s %(progress.total_bytes,progress.total_bytes_estimate)s";

fn parse_progress(line: &str) -> Option<f32> {
    let mut parts = line.strip_prefix("onde ")?.split_whitespace();
    let done: f64 = parts.next()?.parse().ok()?;
    let total: f64 = parts.next()?.parse().ok()?;
    (total > 0.0).then(|| (done / total).clamp(0.0, 1.0) as f32)
}

fn parse_ytdlp_hits(found: &Value) -> Vec<Hit> {
    let Some(entries) = found["entries"].as_array() else {
        return Vec::new();
    };
    entries
        .iter()
        .filter_map(|entry| {
            let reference = entry["webpage_url"]
                .as_str()
                .or_else(|| entry["url"].as_str())?;
            if !reference.starts_with("https://") {
                return None;
            }
            let artist = ["artist", "uploader", "channel"]
                .iter()
                .find_map(|key| entry[*key].as_str())
                .unwrap_or_default();
            Some(Hit {
                title: entry["title"].as_str()?.to_owned(),
                artist: artist.to_owned(),
                seconds: entry["duration"].as_f64(),
                reference: reference.to_owned(),
            })
        })
        .collect()
}

/// Concerts in real FLAC, freely shared with the artists' agreement.
struct LiveMusicArchive;

impl DownloadSource for LiveMusicArchive {
    fn name(&self) -> &'static str {
        "Live Music Archive"
    }

    fn quality(&self) -> &'static str {
        "FLAC sans perte, concerts entiers (souvent plusieurs centaines de Mo)"
    }

    fn search(&self, query: &str) -> Result<Vec<Hit>> {
        let words = search_words(query);
        if words.is_empty() {
            return Ok(Vec::new());
        }
        let found: Value = api_agent()
            .get("https://archive.org/advancedsearch.php")
            .query(
                "q",
                format!("collection:etree AND (creator:({words}) OR title:({words}))"),
            )
            .query("fl[]", "identifier")
            .query("fl[]", "title")
            .query("fl[]", "creator")
            .query("rows", RESULTS.to_string())
            .query("output", "json")
            .call()
            .context("recherche sur archive.org")?
            .body_mut()
            .read_json()
            .context("réponse d'archive.org illisible")?;
        Ok(parse_archive_hits(&found))
    }

    fn download(&self, hit: &Hit, folder: &Path, transfer: &Transfer) -> Result<()> {
        if !valid_identifier(&hit.reference) {
            bail!("référence de concert invalide");
        }
        let metadata: Value = api_agent()
            .get(format!("https://archive.org/metadata/{}", hit.reference))
            .call()
            .context("lecture du concert sur archive.org")?
            .body_mut()
            .read_json()
            .context("réponse d'archive.org illisible")?;
        let files = flac_files(&metadata);
        if files.is_empty() {
            bail!("ce concert n'a pas de fichier FLAC");
        }
        let target = folder
            .join(safe_name(&hit.artist))
            .join(safe_name(&hit.title));
        fs::create_dir_all(&target).context("création du dossier de destination")?;
        let total: u64 = files.iter().map(|(_, size)| size).sum::<u64>().max(1);
        // The API's timeouts would cut a long download: only connecting is limited.
        let agent = ureq::Agent::new_with_config(
            ureq::Agent::config_builder()
                .timeout_connect(Some(API_TIMEOUT))
                .build(),
        );
        let mut done = 0;
        let mut written_files = Vec::new();
        let forget_all = |files: &[PathBuf]| {
            for file in files {
                let _ = fs::remove_file(file);
            }
            // Only succeeds when nothing else is in there.
            let _ = fs::remove_dir(&target);
        };
        for (name, size) in &files {
            let path = target.join(safe_name(name.rsplit('/').next().unwrap_or(name)));
            if !path.exists() {
                let url = format!(
                    "https://archive.org/download/{}/{}",
                    hit.reference,
                    encode_path(name)
                );
                let mut response = agent
                    .get(&url)
                    .call()
                    .context("téléchargement du fichier")?;
                let mut reader = response.body_mut().with_config().limit(u64::MAX).reader();
                let partial = path.with_extension("part");
                let mut file = File::create(&partial).context("écriture du fichier")?;
                let mut buffer = vec![0; 64 * 1024];
                let mut written = 0;
                loop {
                    if transfer.is_cancelled() {
                        drop(file);
                        let _ = fs::remove_file(&partial);
                        forget_all(&written_files);
                        bail!("annulé");
                    }
                    let read = reader
                        .read(&mut buffer)
                        .context("téléchargement du fichier")?;
                    if read == 0 {
                        break;
                    }
                    file.write_all(&buffer[..read])
                        .context("écriture du fichier")?;
                    written += read as u64;
                    transfer.set_progress((done + written) as f32 / total as f32);
                }
                fs::rename(&partial, &path).context("écriture du fichier")?;
                written_files.push(path);
            }
            done += size;
            transfer.set_progress(done as f32 / total as f32);
        }
        transfer.set_progress(1.0);
        Ok(())
    }
}

fn api_agent() -> ureq::Agent {
    ureq::Agent::new_with_config(
        ureq::Agent::config_builder()
            .timeout_global(Some(API_TIMEOUT))
            .build(),
    )
}

/// Keeps what is a word, so a search can't change the meaning of the query.
fn search_words(query: &str) -> String {
    query
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn parse_archive_hits(found: &Value) -> Vec<Hit> {
    let Some(docs) = found["response"]["docs"].as_array() else {
        return Vec::new();
    };
    docs.iter()
        .filter_map(|doc| {
            let reference = doc["identifier"].as_str()?;
            if !valid_identifier(reference) {
                return None;
            }
            let artist = doc["creator"]
                .as_str()
                .or_else(|| doc["creator"][0].as_str())
                .unwrap_or_default();
            Some(Hit {
                title: doc["title"].as_str().unwrap_or(reference).to_owned(),
                artist: artist.to_owned(),
                seconds: None,
                reference: reference.to_owned(),
            })
        })
        .collect()
}

fn flac_files(metadata: &Value) -> Vec<(String, u64)> {
    let Some(files) = metadata["files"].as_array() else {
        return Vec::new();
    };
    files
        .iter()
        .filter_map(|file| {
            let name = file["name"].as_str()?;
            let is_flac = file["format"]
                .as_str()
                .is_some_and(|format| format.to_lowercase().contains("flac"))
                && name.to_lowercase().ends_with(".flac");
            let size = file["size"].as_str().and_then(|size| size.parse().ok());
            is_flac.then(|| (name.to_owned(), size.unwrap_or(0)))
        })
        .collect()
}

fn valid_identifier(identifier: &str) -> bool {
    !identifier.is_empty()
        && !identifier.starts_with('.')
        && identifier
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "._-".contains(c))
}

/// Names come from the internet: they must stay a single name inside the folder.
fn safe_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c == '/' || c == '\\' || c.is_control() {
                '_'
            } else {
                c
            }
        })
        .take(MAX_NAME_CHARS)
        .collect();
    let cleaned = cleaned.trim().trim_start_matches('.').trim();
    if cleaned.is_empty() {
        "Inconnu".to_owned()
    } else {
        cleaned.to_owned()
    }
}

fn encode_path(path: &str) -> String {
    let mut encoded = String::new();
    for byte in path.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                encoded.push(byte as char);
            }
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;

    fn fake_ytdlp(name: &str, script: &str) -> YtDlp {
        let folder = PathBuf::from("target").join("fake-ytdlp");
        fs::create_dir_all(&folder).unwrap();
        let path = folder.join(name);
        fs::write(&path, format!("#!/bin/sh\n{script}\n")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        YtDlp {
            program: path.display().to_string(),
            name: "faux",
            prefix: "ytsearch",
            quality: "",
        }
    }

    #[test]
    fn reads_ytdlp_results_and_skips_bad_addresses() {
        let hits = parse_ytdlp_hits(&json!({"entries": [
            {"title": "A", "uploader": "U", "duration": 61.0, "url": "https://x/a"},
            {"title": "B", "artist": "Art", "uploader": "U", "webpage_url": "https://x/b"},
            {"title": "C", "url": "file:///etc/passwd"},
            {"url": "https://x/sans-titre"},
        ]}));
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].seconds, Some(61.0));
        assert_eq!(hits[1].artist, "Art");
        assert_eq!(hits[1].reference, "https://x/b");
    }

    #[test]
    fn progress_needs_a_known_total() {
        assert_eq!(parse_progress("onde 50 200"), Some(0.25));
        assert_eq!(parse_progress("onde 50 NA"), None);
        assert_eq!(parse_progress("onde 50 0"), None);
        assert_eq!(parse_progress("[download] truc"), None);
    }

    #[test]
    fn search_passes_the_query_as_one_argument() {
        let source = fake_ytdlp(
            "search",
            r#"for last; do :; done; printf '{"entries":[{"title":"%s","url":"https://x/1"}]}' "$last""#,
        );
        let hits = source.search("--exec rm -rf").unwrap();
        assert_eq!(hits[0].title, "ytsearch15:--exec rm -rf");
    }

    #[test]
    fn download_reports_progress_and_errors() {
        let source = fake_ytdlp("ok", "echo 'onde 1 4'; echo 'onde 4 4'");
        let hit = Hit {
            title: "T".into(),
            artist: "A".into(),
            seconds: None,
            reference: "https://x/1".into(),
        };
        let transfer = Transfer::default();
        source.download(&hit, Path::new("/tmp"), &transfer).unwrap();
        assert_eq!(transfer.progress(), 1.0);

        let source = fake_ytdlp(
            "ko",
            "echo 'WARNING: x' >&2; echo 'ERROR: Video unavailable' >&2; exit 1",
        );
        let error = source
            .download(&hit, Path::new("/tmp"), &Transfer::default())
            .unwrap_err();
        assert_eq!(error.to_string(), "Video unavailable");
    }

    #[test]
    fn cancelling_stops_the_program_and_what_it_started() {
        // `sleep` keeps the output open: stopping only the shell would hang here.
        let source = fake_ytdlp("long", "sleep 30; echo fini");
        let hit = Hit {
            title: "T".into(),
            artist: String::new(),
            seconds: None,
            reference: "https://x/1".into(),
        };
        let transfer = Transfer::default();
        let canceller = transfer.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(200));
            canceller.cancel();
        });
        let started = std::time::Instant::now();
        let error = source
            .download(&hit, Path::new("/tmp"), &transfer)
            .unwrap_err();
        assert_eq!(error.to_string(), "annulé");
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn refuses_to_download_anything_but_https() {
        let source = fake_ytdlp("never", "exit 0");
        let hit = Hit {
            title: "T".into(),
            artist: String::new(),
            seconds: None,
            reference: "--exec=rm".into(),
        };
        assert!(
            source
                .download(&hit, Path::new("/tmp"), &Transfer::default())
                .is_err()
        );
    }

    #[test]
    fn says_when_ytdlp_is_missing() {
        let mut source = fake_ytdlp("absent", "");
        source.program = "/nulle/part/yt-dlp".into();
        let error = source.search("x").unwrap_err();
        assert!(error.to_string().contains("pas installé"));
    }

    #[test]
    fn names_from_the_internet_cannot_leave_the_folder() {
        assert_eq!(safe_name("../../.ssh/id"), "_.._.ssh_id");
        assert_eq!(safe_name(".."), "Inconnu");
        assert_eq!(safe_name("  "), "Inconnu");
        assert_eq!(safe_name("Été 1977"), "Été 1977");
        assert!(safe_name(&"x".repeat(500)).chars().count() <= MAX_NAME_CHARS);
        assert!(valid_identifier("gd1977-05-08.sbd"));
        assert!(!valid_identifier("../secret"));
        assert!(!valid_identifier(""));
    }

    #[test]
    fn keeps_only_flac_files_of_a_concert() {
        let files = flac_files(&json!({"files": [
            {"name": "d1t01.flac", "format": "Flac", "size": "100"},
            {"name": "d1t02.flac", "format": "24bit Flac", "size": "200"},
            {"name": "d1t01.mp3", "format": "VBR MP3", "size": "10"},
            {"name": "x_files.xml", "format": "Metadata"},
        ]}));
        assert_eq!(
            files,
            [
                ("d1t01.flac".to_owned(), 100),
                ("d1t02.flac".to_owned(), 200)
            ]
        );
    }

    #[test]
    fn search_words_cannot_change_the_query() {
        assert_eq!(search_words("a) OR (collection:x"), "a OR collection x");
        assert_eq!(search_words("  Été \"77\" "), "Été 77");
    }

    #[test]
    fn encodes_file_names_for_addresses() {
        assert_eq!(encode_path("d 1/é.flac"), "d%201/%C3%A9.flac");
    }

    /// Needs the network and yt-dlp: `cargo test -- --ignored real_`.
    #[test]
    #[ignore]
    fn real_youtube_search_and_download_gives_a_readable_file() {
        let source = YtDlp::new("YouTube", "ytsearch", "");
        assert!(!source.search("blender open movie").unwrap().is_empty());
        let folder = PathBuf::from("target/vraie-recherche");
        let _ = fs::remove_dir_all(&folder);
        let hit = Hit {
            title: "T".into(),
            artist: String::new(),
            seconds: None,
            // The first video ever posted on YouTube: 19 seconds.
            reference: "https://www.youtube.com/watch?v=jNQXAC9IVRw".into(),
        };
        source
            .download(&hit, &folder, &Transfer::default())
            .unwrap();
        let tracks = crate::library::scan(&folder);
        assert_eq!(tracks.len(), 1, "{tracks:?}");
        println!("{tracks:?}");
        let crate::library::Location::File(path) = &tracks[0].location else {
            panic!("pas un fichier");
        };
        let audio = crate::decoder::AudioSource::File(path.clone());
        let mut decoder = crate::decoder::TrackDecoder::open(&audio).unwrap();
        assert!(decoder.next_chunk().unwrap().is_some());
        assert!(
            crate::library::read_cover(path).is_some(),
            "pochette absente"
        );
        assert!(!path.with_extension("jpg").exists(), "image restée à côté");
    }

    #[test]
    #[ignore]
    fn real_youtube_cancel_leaves_nothing_behind() {
        let source = YtDlp::new("YouTube", "ytsearch", "");
        let hit = source.search("blender open movie full").unwrap().remove(0);
        let folder = PathBuf::from("target/vraie-annulation");
        let _ = fs::remove_dir_all(&folder);
        fs::create_dir_all(&folder).unwrap();
        let transfer = Transfer::default();
        let canceller = transfer.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(2500));
            canceller.cancel();
        });
        let error = source.download(&hit, &folder, &transfer).unwrap_err();
        assert_eq!(error.to_string(), "annulé");
        let left: Vec<_> = walkdir::WalkDir::new(&folder)
            .into_iter()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_type().is_file())
            .map(|entry| entry.path().display().to_string())
            .collect();
        assert!(left.is_empty(), "{left:?}");
        fs::remove_dir_all(&folder).unwrap();
    }

    #[test]
    #[ignore]
    fn real_archive_search_finds_concerts_with_flac() {
        let hits = LiveMusicArchive.search("grateful dead 1977").unwrap();
        assert!(!hits.is_empty());
        println!("{:?}", hits[0]);
        let metadata: Value = api_agent()
            .get(format!(
                "https://archive.org/metadata/{}",
                hits[0].reference
            ))
            .call()
            .unwrap()
            .body_mut()
            .read_json()
            .unwrap();
        println!("{} FLAC", flac_files(&metadata).len());
    }

    #[test]
    fn reads_archive_results() {
        let hits = parse_archive_hits(&json!({"response": {"docs": [
            {"identifier": "gd77", "title": "Concert", "creator": ["Band", "Autre"]},
            {"identifier": "../mauvais", "title": "X"},
        ]}}));
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].artist, "Band");
    }
}
