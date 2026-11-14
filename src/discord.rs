//! Shows what is playing on Discord, through the socket of the local client.
//! Everything happens on its own thread: a missing Discord never slows playback.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

use crate::cover_host::Links;
use crate::online_cover::{self, Picture};

const OP_HANDSHAKE: u32 = 0;
const OP_FRAME: u32 = 1;
const OP_CLOSE: u32 = 2;
const LONGEST_FRAME: u32 = 1 << 20;
const LONGEST_TEXT: usize = 128;
/// Discord refuses shorter texts.
const SHORTEST_TEXT: usize = 2;
/// Discord allows about five updates every 20 seconds.
const MINIMUM_GAP: Duration = if cfg!(test) {
    Duration::from_millis(50)
} else {
    Duration::from_secs(4)
};
/// Also notices when Discord has been closed or started.
const RETRY: Duration = Duration::from_secs(15);
const REPLY_WAIT: Duration = Duration::from_secs(2);
/// Discord's "listening" activity.
const LISTENING: u32 = 2;
const ACTIVITY_NAME: &str = "Onde";

#[derive(Clone, PartialEq, Debug)]
pub struct Activity {
    pub title: String,
    pub artist: String,
    pub album: String,
    /// Tracks of an album share a cover, so this is the album's.
    pub cover_key: String,
    pub position: f64,
    pub duration: f64,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Status {
    Searching,
    Connected,
    Refused,
}

enum Message {
    Show(Option<Activity>, Option<Picture>),
    /// The answer to a search, for the album it was made for.
    Cover(String, Option<String>),
    Stop,
}

type Lookup = fn(&str, &str, &str, Option<Picture>) -> Option<String>;

pub struct Presence {
    sender: Sender<Message>,
    status: Arc<Mutex<Status>>,
}

impl Presence {
    pub fn start(client_id: String, links: Arc<Links>) -> Self {
        let (sender, receiver) = channel();
        let status = Arc::new(Mutex::new(Status::Searching));
        let shared = status.clone();
        let answers = sender.clone();
        std::thread::spawn(move || {
            run(
                &client_id,
                &candidates(),
                &receiver,
                &answers,
                online_cover::find_or_upload,
                &links,
                &shared,
            );
        });
        Self { sender, status }
    }

    pub fn show(&self, activity: Option<Activity>, picture: Option<Picture>) {
        let _ = self.sender.send(Message::Show(activity, picture));
    }

    pub fn status(&self) -> Status {
        *self.status.lock().unwrap()
    }
}

impl Drop for Presence {
    /// Closing the connection also clears the activity.
    fn drop(&mut self) {
        let _ = self.sender.send(Message::Stop);
    }
}

fn candidates() -> Vec<PathBuf> {
    let mut bases = Vec::new();
    if let Some(runtime) = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from) {
        // Flatpak and Snap keep it in a folder of their own.
        bases.push(runtime.join("app/com.discordapp.Discord"));
        bases.push(runtime.join("snap.discord"));
        bases.push(runtime);
    }
    bases.push(PathBuf::from("/tmp"));
    bases
        .iter()
        .flat_map(|base| (0..10).map(move |n| base.join(format!("discord-ipc-{n}"))))
        .collect()
}

fn run(
    client_id: &str,
    sockets: &[PathBuf],
    receiver: &Receiver<Message>,
    answers: &Sender<Message>,
    lookup: Lookup,
    links: &Links,
    status: &Mutex<Status>,
) {
    // Empty while the search runs, and for good when it found nothing.
    let mut covers: HashMap<String, Option<String>> = HashMap::new();
    let mut send_name = true;
    // Set when it arrived: sending it again later must not shift the times.
    let mut wanted: Option<(Activity, u64)> = None;
    let mut wanted_picture: Option<Picture> = None;
    // Connects from the start, so that the settings tell the truth.
    let mut pending = true;
    let mut stream: Option<UnixStream> = None;
    let mut last_sent: Option<Instant> = None;
    loop {
        let wait = match last_sent {
            Some(sent) if pending && stream.is_some() => {
                MINIMUM_GAP.saturating_sub(sent.elapsed())
            }
            Some(sent) => RETRY.saturating_sub(sent.elapsed()),
            None if pending => Duration::ZERO,
            None => RETRY,
        };
        match receiver.recv_timeout(wait) {
            Ok(Message::Stop) => return,
            Ok(Message::Show(activity, picture)) => {
                wanted_picture = picture;
                wanted = activity.map(|activity| (activity, unix_ms()));
                pending = true;
                continue;
            }
            Ok(Message::Cover(key, cover)) => {
                let found = cover.is_some();
                if let Some(url) = cover.as_ref().filter(|url| online_cover::lasts(url)) {
                    links.insert(key.clone(), url.clone());
                }
                covers.insert(key.clone(), cover);
                if found && wanted.as_ref().is_some_and(|(activity, _)| activity.cover_key == key) {
                    pending = true;
                }
                continue;
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
        if last_sent.is_some_and(|sent| sent.elapsed() >= RETRY) {
            pending = true;
        }
        if !pending {
            continue;
        }
        last_sent = Some(Instant::now());
        if stream.is_none() {
            match connect(client_id, sockets) {
                Ok(connected) => {
                    stream = Some(connected);
                    *status.lock().unwrap() = Status::Connected;
                }
                Err(error) => {
                    let refused = error.downcast_ref::<Refused>().is_some();
                    *status.lock().unwrap() = if refused {
                        Status::Refused
                    } else {
                        Status::Searching
                    };
                    continue;
                }
            }
        }
        if let Some((activity, _)) = &wanted {
            let key = activity.cover_key.clone();
            if let Some(url) = links.get(&key) {
                covers.insert(key, Some(url));
            } else if !covers.contains_key(&key) {
                covers.insert(key.clone(), None);
                let answers = answers.clone();
                let activity = activity.clone();
                let picture = wanted_picture.take();
                std::thread::spawn(move || {
                    let cover = lookup(&activity.artist, &activity.album, &activity.title, picture);
                    let _ = answers.send(Message::Cover(key, cover));
                });
            }
        }
        let cover_used = wanted
            .as_ref()
            .and_then(|(activity, _)| covers.get(&activity.cover_key).cloned().flatten());
        let mut payload = match &wanted {
            Some((activity, received)) => {
                set_activity(Some(activity), cover_used.as_deref(), *received)
            }
            None => set_activity(None, None, unix_ms()),
        };
        if !send_name {
            if let Some(body) = payload["args"]["activity"].as_object_mut() {
                body.remove("name");
            }
        }
        let Some(connected) = stream.as_mut() else {
            continue;
        };
        let reply = write_frame(connected, OP_FRAME, &payload)
            .and_then(|()| read_frame(connected).map(|(_, reply)| reply));
        match reply {
            // The track still deserves to show, without what Discord refused.
            Ok(reply) if reply["evt"] == "ERROR" && cover_used.is_some() => {
                if let Some((activity, _)) = &wanted {
                    links.remove(&activity.cover_key);
                    covers.insert(activity.cover_key.clone(), None);
                }
            }
            Ok(reply) if reply["evt"] == "ERROR" && send_name && wanted.is_some() => {
                send_name = false;
            }
            Ok(_) => pending = false,
            Err(_) => {
                stream = None;
                *status.lock().unwrap() = Status::Searching;
            }
        }
    }
}

#[derive(Debug)]
struct Refused;

impl std::fmt::Display for Refused {
    fn fmt(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
        formatter.write_str("identifiant refusé par Discord")
    }
}

impl std::error::Error for Refused {}

fn connect(client_id: &str, sockets: &[PathBuf]) -> Result<UnixStream> {
    let mut stream = sockets
        .iter()
        .find_map(|path| open(path))
        .context("Discord n'est pas ouvert")?;
    stream.set_read_timeout(Some(REPLY_WAIT))?;
    stream.set_write_timeout(Some(REPLY_WAIT))?;
    write_frame(
        &mut stream,
        OP_HANDSHAKE,
        &json!({ "v": 1, "client_id": client_id }),
    )?;
    let (op, reply) = read_frame(&mut stream)?;
    if op == OP_CLOSE {
        return Err(Refused.into());
    }
    if reply["evt"] != "READY" {
        bail!("réponse inattendue de Discord");
    }
    Ok(stream)
}

fn open(path: &Path) -> Option<UnixStream> {
    UnixStream::connect(path).ok()
}

fn write_frame(stream: &mut UnixStream, op: u32, payload: &Value) -> Result<()> {
    let body = payload.to_string();
    let mut frame = Vec::with_capacity(body.len() + 8);
    frame.extend_from_slice(&op.to_le_bytes());
    frame.extend_from_slice(&(body.len() as u32).to_le_bytes());
    frame.extend_from_slice(body.as_bytes());
    stream.write_all(&frame)?;
    Ok(())
}

fn read_frame(stream: &mut UnixStream) -> Result<(u32, Value)> {
    let mut header = [0; 8];
    stream.read_exact(&mut header)?;
    let op = u32::from_le_bytes(header[..4].try_into()?);
    let length = u32::from_le_bytes(header[4..].try_into()?);
    if length > LONGEST_FRAME {
        bail!("réponse trop longue");
    }
    let mut body = vec![0; length as usize];
    stream.read_exact(&mut body)?;
    Ok((op, serde_json::from_slice(&body)?))
}

fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |age| age.as_millis() as u64)
}

fn fit(text: &str) -> String {
    let mut fitted: String = text.chars().take(LONGEST_TEXT).collect();
    while fitted.chars().count() < SHORTEST_TEXT {
        fitted.push(' ');
    }
    fitted
}

fn set_activity(activity: Option<&Activity>, cover: Option<&str>, now_ms: u64) -> Value {
    let body = activity.map(|activity| {
        let start = now_ms.saturating_sub((activity.position * 1000.0) as u64);
        let mut timestamps = json!({ "start": start });
        if activity.duration > 0.0 {
            timestamps["end"] = json!(start + (activity.duration * 1000.0) as u64);
        }
        let mut body = json!({
            // Discord's own client takes it from the application. Plume does not.
            "name": ACTIVITY_NAME,
            "type": LISTENING,
            "details": fit(&activity.title),
            "state": fit(&activity.artist),
            "timestamps": timestamps,
        });
        if let Some(cover) = cover {
            body["assets"] = json!({ "large_image": cover });
            if !activity.album.is_empty() {
                body["assets"]["large_text"] = json!(fit(&activity.album));
            }
        }
        body
    });
    json!({
        "cmd": "SET_ACTIVITY",
        "args": { "pid": std::process::id(), "activity": body },
        "nonce": now_ms.to_string(),
    })
}

/// Only digits, as Discord writes them: anything else is not an identifier.
pub fn valid_client_id(text: &str) -> Option<String> {
    let text = text.trim();
    (text.len() >= 15 && text.chars().all(|c| c.is_ascii_digit())).then(|| text.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixListener;

    fn activity() -> Activity {
        Activity {
            title: "Titre".into(),
            artist: "Artiste".into(),
            album: "Album".into(),
            cover_key: "artiste\0album".into(),
            position: 10.0,
            duration: 100.0,
        }
    }

    #[test]
    fn the_end_time_comes_from_the_position_and_the_duration() {
        let payload = set_activity(Some(&activity()), None, 1_000_000);
        let times = &payload["args"]["activity"]["timestamps"];
        assert_eq!(times["start"], 990_000);
        assert_eq!(times["end"], 1_090_000);
        assert_eq!(payload["args"]["activity"]["type"], 2);
    }

    #[test]
    fn an_unknown_duration_has_no_end() {
        let mut unknown = activity();
        unknown.duration = 0.0;
        let payload = set_activity(Some(&unknown), None, 1_000_000);
        assert!(payload["args"]["activity"]["timestamps"].get("end").is_none());
    }

    #[test]
    fn nothing_playing_clears_the_activity() {
        assert!(set_activity(None, None, 1)["args"]["activity"].is_null());
    }

    #[test]
    fn texts_stay_within_what_discord_accepts() {
        assert_eq!(fit("a").chars().count(), SHORTEST_TEXT);
        assert_eq!(fit(&"é".repeat(500)).chars().count(), LONGEST_TEXT);
    }

    #[test]
    fn checks_what_was_pasted_as_an_identifier() {
        assert_eq!(
            valid_client_id(" 123456789012345678 "),
            Some("123456789012345678".into())
        );
        assert_eq!(valid_client_id("123"), None);
        assert_eq!(valid_client_id("abc456789012345678"), None);
    }

    /// A socket of its own for each test, removed at the end.
    struct Folder(PathBuf);

    impl Drop for Folder {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn fake_discord(reply_op: u32) -> (Folder, PathBuf, Receiver<Value>) {
        static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let number = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let folder = std::env::temp_dir().join(format!(
            "onde-discord-{}-{number}",
            std::process::id()
        ));
        std::fs::create_dir_all(&folder).unwrap();
        let path = folder.join("discord-ipc-0");
        let listener = UnixListener::bind(&path).unwrap();
        let folder = Folder(folder);
        let (sender, frames) = channel();
        std::thread::spawn(move || {
            let (mut client, _) = listener.accept().unwrap();
            let (_, hello) = read_frame(&mut client).unwrap();
            sender.send(hello).unwrap();
            let reply = if reply_op == OP_CLOSE {
                json!({ "code": 4000 })
            } else {
                json!({ "evt": "READY" })
            };
            write_frame(&mut client, reply_op, &reply).unwrap();
            while let Ok((_, frame)) = read_frame(&mut client) {
                let shown = &frame["args"]["activity"];
                let refuse = shown["details"] == "Rejet" && !shown["assets"].is_null();
                let evt = if refuse { json!("ERROR") } else { json!(null) };
                write_frame(&mut client, OP_FRAME, &json!({ "evt": evt })).unwrap();
                sender.send(frame).unwrap();
            }
        });
        (folder, path, frames)
    }

    #[test]
    fn says_hello_then_sends_the_activity() {
        let (_folder, path, frames) = fake_discord(OP_FRAME);
        let (sender, receiver) = channel();
        let status = Arc::new(Mutex::new(Status::Searching));
        let shared = status.clone();
        sender.send(Message::Show(Some(activity()), None)).unwrap();
        let answers = sender.clone();
        std::thread::spawn(move || {
            run("42", &[path], &receiver, &answers, no_cover, &Links::in_memory(), &shared);
        });
        let wait = Duration::from_secs(5);
        assert_eq!(frames.recv_timeout(wait).unwrap()["client_id"], "42");
        let frame = frames.recv_timeout(wait).unwrap();
        assert_eq!(frame["args"]["activity"]["details"], "Titre");
        assert_eq!(*status.lock().unwrap(), Status::Connected);
    }

    fn no_cover(_: &str, _: &str, _: &str, _: Option<Picture>) -> Option<String> {
        None
    }

    fn some_cover(_: &str, _: &str, _: &str, _: Option<Picture>) -> Option<String> {
        Some("https://image".into())
    }

    /// The frames until one satisfies the check, which is what the test waits for.
    fn wait_for(frames: &Receiver<Value>, check: impl Fn(&Value) -> bool) {
        loop {
            let frame = frames.recv_timeout(Duration::from_secs(5)).unwrap();
            if check(&frame) {
                return;
            }
        }
    }

    #[test]
    fn the_cover_follows_once_found_and_goes_if_discord_refuses_it() {
        let (_folder, path, frames) = fake_discord(OP_FRAME);
        let (sender, receiver) = channel();
        let status = Arc::new(Mutex::new(Status::Searching));
        let answers = sender.clone();
        std::thread::spawn(move || {
            run("42", &[path], &receiver, &answers, some_cover, &Links::in_memory(), &status);
        });
        sender.send(Message::Show(Some(activity()), None)).unwrap();
        wait_for(&frames, |frame| {
            frame["args"]["activity"]["assets"]["large_image"] == "https://image"
        });
        let mut refused = activity();
        refused.title = "Rejet".into();
        refused.album = "Autre".into();
        refused.cover_key = "autre".into();
        sender.send(Message::Show(Some(refused), None)).unwrap();
        wait_for(&frames, |frame| {
            frame["args"]["activity"]["assets"]["large_image"] == "https://image"
                && frame["args"]["activity"]["details"] == "Rejet"
        });
        wait_for(&frames, |frame| {
            frame["args"]["activity"]["details"] == "Rejet"
                && frame["args"]["activity"]["assets"].is_null()
        });
    }

    #[test]
    fn a_known_link_goes_out_with_the_first_frame_without_searching() {
        fn must_not_search(_: &str, _: &str, _: &str, _: Option<Picture>) -> Option<String> {
            panic!("déjà connue");
        }
        let (_folder, path, frames) = fake_discord(OP_FRAME);
        let links = Links::in_memory();
        links.insert("artiste\0album".into(), "https://hote/a.jpg".into());
        let (sender, receiver) = channel();
        let status = Arc::new(Mutex::new(Status::Searching));
        let answers = sender.clone();
        sender.send(Message::Show(Some(activity()), None)).unwrap();
        std::thread::spawn(move || {
            run("42", &[path], &receiver, &answers, must_not_search, &links, &status);
        });
        let wait = Duration::from_secs(5);
        frames.recv_timeout(wait).unwrap();
        let first = frames.recv_timeout(wait).unwrap();
        assert_eq!(first["args"]["activity"]["assets"]["large_image"], "https://hote/a.jpg");
    }

    #[test]
    fn a_refused_identifier_is_reported() {
        let (_folder, path, _frames) = fake_discord(OP_CLOSE);
        let error = connect("42", &[path]).unwrap_err();
        assert!(error.downcast_ref::<Refused>().is_some());
    }

    #[test]
    fn no_socket_means_discord_is_not_open() {
        let error = connect("42", &[PathBuf::from("/nulle/part")]).unwrap_err();
        assert!(error.downcast_ref::<Refused>().is_none());
    }
}
