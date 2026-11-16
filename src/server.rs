//! Subsonic-compatible server so mobile apps can stream the local library.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::thread;

use crate::catalog::{Album, Catalog, album_key};
use crate::library::{self, Location, Track};
use crate::sources::MusicSource;

const API_VERSION: &str = "1.16.1";
const SERVER_NAME: &str = "Onde";

pub struct SharedLibrary {
    pub tracks: Vec<Track>,
    pub catalog: Catalog,
    pub source: Option<Arc<dyn MusicSource>>,
}

impl Default for SharedLibrary {
    fn default() -> Self {
        Self {
            tracks: Vec::new(),
            catalog: Catalog::default(),
            source: None,
        }
    }
}

pub struct SubsonicServer {
    stop: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
}

impl SubsonicServer {
    pub fn start(
        port: u16,
        user: String,
        password: String,
        library: Arc<RwLock<SharedLibrary>>,
    ) -> std::io::Result<Self> {
        let addr: SocketAddr = ([0, 0, 0, 0], port).into();
        let listener = TcpListener::bind(addr)?;
        listener.set_nonblocking(false)?;
        let stop = Arc::new(AtomicBool::new(false));
        let stop_clone = stop.clone();
        let lib = library.clone();
        let handle = thread::Builder::new()
            .name("onde-server".into())
            .spawn(move || {
                listener
                    .set_nonblocking(true)
                    .expect("non-blocking listener");
                loop {
                    if stop_clone.load(Ordering::Relaxed) {
                        break;
                    }
                    match listener.accept() {
                        Ok((stream, _)) => {
                            let lib = lib.clone();
                            let user = user.clone();
                            let password = password.clone();
                            thread::spawn(move || {
                                let _ = handle_connection(stream, &lib, &user, &password);
                            });
                        }
                        Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            thread::sleep(std::time::Duration::from_millis(100));
                        }
                        Err(_) => {
                            thread::sleep(std::time::Duration::from_millis(50));
                        }
                    }
                }
            })?;
        Ok(Self {
            stop,
            handle: Some(handle),
        })
    }

    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }

}

impl Drop for SubsonicServer {
    fn drop(&mut self) {
        self.stop();
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

struct Request {
    path: String,
    params: HashMap<String, String>,
}

fn parse_request(stream: &mut TcpStream) -> Option<Request> {
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(10)))
        .ok()?;
    let mut reader = BufReader::new(stream.try_clone().ok()?);
    let mut line = String::new();
    reader.read_line(&mut line).ok()?;
    let parts: Vec<&str> = line.split_whitespace().collect();
    if parts.len() < 2 || parts[0] != "GET" {
        return None;
    }
    let full_path = parts[1];
    let (path, query) = match full_path.split_once('?') {
        Some((p, q)) => (p.to_owned(), q.to_owned()),
        None => (full_path.to_owned(), String::new()),
    };
    let params: HashMap<String, String> = query
        .split('&')
        .filter(|s| !s.is_empty())
        .filter_map(|pair| {
            let (key, value) = pair.split_once('=')?;
            Some((
                url_decode(key),
                url_decode(value),
            ))
        })
        .collect();
    // Drain headers.
    loop {
        let mut header = String::new();
        if reader.read_line(&mut header).ok()? == 0 || header.trim().is_empty() {
            break;
        }
    }
    Some(Request { path, params })
}

fn url_decode(s: &str) -> String {
    let mut out = Vec::new();
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(byte) = u8::from_str_radix(
                std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or(""),
                16,
            ) {
                out.push(byte);
                i += 3;
                continue;
            }
        } else if bytes[i] == b'+' {
            out.push(b' ');
            i += 1;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn handle_connection(
    mut stream: TcpStream,
    library: &Arc<RwLock<SharedLibrary>>,
    user: &str,
    password: &str,
) -> std::io::Result<()> {
    let Some(request) = parse_request(&mut stream) else {
        return send_error(&mut stream, 0, "Bad request");
    };
    if !authenticate(&request.params, user, password) {
        return send_xml(
            &mut stream,
            &subsonic_error(40, "Wrong username or password"),
        );
    }
    let lib = library.read().unwrap();
    let endpoint = request
        .path
        .trim_start_matches("/rest/")
        .trim_end_matches(".view");
    match endpoint {
        "ping" => send_xml(&mut stream, &ok_response("")),
        "getLicense" => send_xml(
            &mut stream,
            &ok_response(r#"<license valid="true" email="" licenseExpires="2099-12-31T00:00:00"/>"#),
        ),
        "getMusicFolders" => send_xml(
            &mut stream,
            &ok_response(r#"<musicFolders><musicFolder id="1" name="Musique"/></musicFolders>"#),
        ),
        "getIndexes" | "getArtists" => {
            send_xml(&mut stream, &artists_xml(&lib))
        }
        "getArtist" => {
            let id = request.params.get("id").map(String::as_str).unwrap_or("");
            send_xml(&mut stream, &artist_xml(&lib, id))
        }
        "getAlbum" => {
            let id = request.params.get("id").map(String::as_str).unwrap_or("");
            send_xml(&mut stream, &album_xml(&lib, id))
        }
        "getAlbumList" | "getAlbumList2" => {
            let size = request
                .params
                .get("size")
                .and_then(|s| s.parse().ok())
                .unwrap_or(20usize)
                .min(500);
            let offset = request
                .params
                .get("offset")
                .and_then(|s| s.parse().ok())
                .unwrap_or(0usize);
            send_xml(&mut stream, &album_list_xml(&lib, size, offset))
        }
        "getSong" => {
            let id = request.params.get("id").map(String::as_str).unwrap_or("");
            send_xml(&mut stream, &song_xml(&lib, id))
        }
        "search2" | "search3" => {
            let query = request.params.get("query").map(String::as_str).unwrap_or("");
            let count = request
                .params
                .get("songCount")
                .or(request.params.get("count"))
                .and_then(|s| s.parse().ok())
                .unwrap_or(20usize)
                .min(100);
            send_xml(&mut stream, &search_xml(&lib, query, count))
        }
        "stream" | "download" => {
            let id = request.params.get("id").map(String::as_str).unwrap_or("");
            stream_file(&mut stream, &lib, id)
        }
        "getCoverArt" => {
            let id = request.params.get("id").map(String::as_str).unwrap_or("");
            serve_cover(&mut stream, &lib, id)
        }
        "getPlaylists" => send_xml(
            &mut stream,
            &ok_response("<playlists/>"),
        ),
        "getStarred" | "getStarred2" => send_xml(
            &mut stream,
            &ok_response("<starred2/>"),
        ),
        "scrobble" => send_xml(&mut stream, &ok_response("")),
        _ => send_error(&mut stream, 0, "Not implemented"),
    }
}

fn authenticate(params: &HashMap<String, String>, user: &str, password: &str) -> bool {
    let req_user = params.get("u").map(String::as_str).unwrap_or("");
    if req_user != user {
        return false;
    }
    if let Some(token) = params.get("t") {
        let salt = params.get("s").map(String::as_str).unwrap_or("");
        let expected = format!("{:x}", md5::compute(format!("{password}{salt}")));
        return token == &expected;
    }
    if let Some(p) = params.get("p") {
        let plain = p.strip_prefix("enc:").map_or_else(
            || p.clone(),
            |hex| {
                let bytes: Vec<u8> = (0..hex.len())
                    .step_by(2)
                    .filter_map(|i| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok())
                    .collect();
                String::from_utf8_lossy(&bytes).into_owned()
            },
        );
        return plain == password;
    }
    false
}

fn ok_response(body: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<subsonic-response xmlns="http://subsonic.org/restapi" status="ok" version="{API_VERSION}" serverVersion="{SERVER_NAME}">
{body}
</subsonic-response>"#
    )
}

fn subsonic_error(code: u32, message: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<subsonic-response xmlns="http://subsonic.org/restapi" status="failed" version="{API_VERSION}">
<error code="{code}" message="{}"/>
</subsonic-response>"#,
        xml_escape(message)
    )
}

fn send_xml(stream: &mut TcpStream, body: &str) -> std::io::Result<()> {
    let header = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/xml; charset=UTF-8\r\nContent-Length: {}\r\nAccess-Control-Allow-Origin: *\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(header.as_bytes())?;
    stream.write_all(body.as_bytes())?;
    stream.flush()
}

fn send_error(stream: &mut TcpStream, code: u32, message: &str) -> std::io::Result<()> {
    send_xml(stream, &subsonic_error(code, message))
}

fn send_binary(stream: &mut TcpStream, data: &[u8], content_type: &str) -> std::io::Result<()> {
    let header = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nAccess-Control-Allow-Origin: *\r\nConnection: close\r\n\r\n",
        data.len()
    );
    stream.write_all(header.as_bytes())?;
    stream.write_all(data)?;
    stream.flush()
}

fn track_id(index: usize) -> String {
    format!("tr-{index}")
}

fn album_id(key: &str) -> String {
    format!("al-{}", hex_encode(key))
}

fn artist_id(name: &str) -> String {
    format!("ar-{}", hex_encode(&name.to_lowercase()))
}

fn hex_encode(s: &str) -> String {
    s.bytes().map(|b| format!("{b:02x}")).collect()
}

fn hex_decode(s: &str) -> String {
    let bytes: Vec<u8> = (0..s.len())
        .step_by(2)
        .filter_map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok())
        .collect();
    String::from_utf8_lossy(&bytes).into_owned()
}

fn parse_track_id(id: &str) -> Option<usize> {
    id.strip_prefix("tr-")?.parse().ok()
}

fn parse_album_id(id: &str) -> Option<String> {
    id.strip_prefix("al-").map(hex_decode)
}

fn parse_artist_id(id: &str) -> Option<String> {
    id.strip_prefix("ar-").map(hex_decode)
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn song_tag(track: &Track, index: usize, album: Option<&Album>) -> String {
    let id = track_id(index);
    let album_id = album.map(|a| album_id(&a.key)).unwrap_or_default();
    let artist_id = artist_id(&track.artist);
    let ext = match &track.location {
        Location::File(path) => path
            .extension()
            .map(|e| e.to_string_lossy().into_owned())
            .unwrap_or_default(),
        _ => track.extension.clone().unwrap_or_default(),
    };
    let content_type = match ext.as_str() {
        "mp3" => "audio/mpeg",
        "flac" => "audio/flac",
        "ogg" | "oga" => "audio/ogg",
        "m4a" | "mp4" | "aac" => "audio/mp4",
        "wav" => "audio/wav",
        _ => "audio/mpeg",
    };
    format!(
        r#"<song id="{id}" title="{title}" album="{album}" artist="{artist}" track="{number}" discNumber="{disc}" duration="{duration}" albumId="{album_id}" artistId="{artist_id}" coverArt="{cover}" contentType="{content_type}" suffix="{ext}" isDir="false" type="music"/>"#,
        title = xml_escape(&track.title),
        album = xml_escape(&track.album),
        artist = xml_escape(&track.artist),
        number = track.number,
        disc = track.disc,
        duration = track.duration as u64,
        cover = album_id,
        ext = xml_escape(&ext),
    )
}

fn artists_xml(lib: &SharedLibrary) -> String {
    let mut by_letter: HashMap<char, Vec<&str>> = HashMap::new();
    for artist in &lib.catalog.artists {
        let letter = artist
            .name
            .chars()
            .next()
            .unwrap_or('#')
            .to_uppercase()
            .next()
            .unwrap_or('#');
        let letter = if letter.is_alphanumeric() { letter } else { '#' };
        by_letter.entry(letter).or_default().push(&artist.name);
    }
    let mut letters: Vec<char> = by_letter.keys().copied().collect();
    letters.sort();
    let mut body = String::from("<artists>");
    for letter in letters {
        body.push_str(&format!(r#"<index name="{letter}">"#));
        let names = by_letter.get(&letter).unwrap();
        for name in names {
            body.push_str(&format!(
                r#"<artist id="{}" name="{}" albumCount="{}"/>"#,
                artist_id(name),
                xml_escape(name),
                lib.catalog
                    .artist(name)
                    .map(|a| a.albums.len())
                    .unwrap_or(0),
            ));
        }
        body.push_str("</index>");
    }
    body.push_str("</artists>");
    ok_response(&body)
}

fn artist_xml(lib: &SharedLibrary, id: &str) -> String {
    let Some(name) = parse_artist_id(id) else {
        return subsonic_error(0, "Invalid artist ID");
    };
    let Some(artist) = lib.catalog.artist(&name) else {
        return subsonic_error(70, "Artist not found");
    };
    let mut body = format!(
        r#"<artist id="{}" name="{}" albumCount="{}">"#,
        artist_id(&name),
        xml_escape(&artist.name),
        artist.albums.len(),
    );
    for &album_idx in &artist.albums {
        let album = &lib.catalog.albums[album_idx];
        body.push_str(&format!(
            r#"<album id="{}" name="{}" artist="{}" artistId="{}" songCount="{}" duration="{}" coverArt="{}"/>"#,
            album_id(&album.key),
            xml_escape(&album.title),
            xml_escape(&album.artist),
            artist_id(&album.artist),
            album.tracks.len(),
            album.seconds as u64,
            album_id(&album.key),
        ));
    }
    body.push_str("</artist>");
    ok_response(&body)
}

fn album_xml(lib: &SharedLibrary, id: &str) -> String {
    let Some(key) = parse_album_id(id) else {
        return subsonic_error(0, "Invalid album ID");
    };
    let Some(album) = lib.catalog.album(&key) else {
        return subsonic_error(70, "Album not found");
    };
    let mut body = format!(
        r#"<album id="{}" name="{}" artist="{}" artistId="{}" songCount="{}" duration="{}" coverArt="{}">"#,
        album_id(&album.key),
        xml_escape(&album.title),
        xml_escape(&album.artist),
        artist_id(&album.artist),
        album.tracks.len(),
        album.seconds as u64,
        album_id(&album.key),
    );
    for &track_idx in &album.tracks {
        if let Some(track) = lib.tracks.get(track_idx) {
            body.push_str(&song_tag(track, track_idx, Some(album)));
        }
    }
    body.push_str("</album>");
    ok_response(&body)
}

fn album_list_xml(lib: &SharedLibrary, size: usize, offset: usize) -> String {
    let total = lib.catalog.albums.len();
    let end = (offset + size).min(total);
    let mut body = String::from("<albumList2>");
    for i in offset..end {
        let album = &lib.catalog.albums[i];
        body.push_str(&format!(
            r#"<album id="{}" name="{}" artist="{}" artistId="{}" songCount="{}" duration="{}" coverArt="{}"/>"#,
            album_id(&album.key),
            xml_escape(&album.title),
            xml_escape(&album.artist),
            artist_id(&album.artist),
            album.tracks.len(),
            album.seconds as u64,
            album_id(&album.key),
        ));
    }
    body.push_str("</albumList2>");
    ok_response(&body)
}

fn song_xml(lib: &SharedLibrary, id: &str) -> String {
    let Some(index) = parse_track_id(id) else {
        return subsonic_error(0, "Invalid song ID");
    };
    let Some(track) = lib.tracks.get(index) else {
        return subsonic_error(70, "Song not found");
    };
    let key = album_key(track);
    let album = lib.catalog.album(&key);
    ok_response(&song_tag(track, index, album))
}

fn search_xml(lib: &SharedLibrary, query: &str, count: usize) -> String {
    let lower = query.to_lowercase();
    let mut songs = Vec::new();
    let mut albums = Vec::new();
    let mut artists = Vec::new();
    for (index, track) in lib.tracks.iter().enumerate() {
        if songs.len() >= count {
            break;
        }
        if track.title.to_lowercase().contains(&lower)
            || track.artist.to_lowercase().contains(&lower)
            || track.album.to_lowercase().contains(&lower)
        {
            songs.push(index);
        }
    }
    for (i, album) in lib.catalog.albums.iter().enumerate() {
        if albums.len() >= count {
            break;
        }
        if album.title.to_lowercase().contains(&lower)
            || album.artist.to_lowercase().contains(&lower)
        {
            albums.push(i);
        }
    }
    for artist in &lib.catalog.artists {
        if artists.len() >= count {
            break;
        }
        if artist.name.to_lowercase().contains(&lower) {
            artists.push(artist.name.clone());
        }
    }
    let mut body = String::from("<searchResult3>");
    for name in &artists {
        body.push_str(&format!(
            r#"<artist id="{}" name="{}"/>"#,
            artist_id(name),
            xml_escape(name),
        ));
    }
    for &i in &albums {
        let album = &lib.catalog.albums[i];
        body.push_str(&format!(
            r#"<album id="{}" name="{}" artist="{}" coverArt="{}"/>"#,
            album_id(&album.key),
            xml_escape(&album.title),
            xml_escape(&album.artist),
            album_id(&album.key),
        ));
    }
    for &index in &songs {
        let track = &lib.tracks[index];
        let key = album_key(track);
        let album = lib.catalog.album(&key);
        body.push_str(&song_tag(track, index, album));
    }
    body.push_str("</searchResult3>");
    ok_response(&body)
}

fn stream_file(stream: &mut TcpStream, lib: &SharedLibrary, id: &str) -> std::io::Result<()> {
    let Some(index) = parse_track_id(id) else {
        return send_error(stream, 0, "Invalid song ID");
    };
    let Some(track) = lib.tracks.get(index) else {
        return send_error(stream, 70, "Song not found");
    };
    let Location::File(path) = &track.location else {
        return send_error(stream, 0, "Remote tracks cannot be streamed");
    };
    let data = match std::fs::read(path) {
        Ok(data) => data,
        Err(_) => return send_error(stream, 0, "File not readable"),
    };
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().into_owned())
        .unwrap_or_default();
    let content_type = match ext.as_str() {
        "mp3" => "audio/mpeg",
        "flac" => "audio/flac",
        "ogg" | "oga" => "audio/ogg",
        "m4a" | "mp4" | "aac" => "audio/mp4",
        "wav" => "audio/wav",
        _ => "application/octet-stream",
    };
    send_binary(stream, &data, content_type)
}

fn serve_cover(stream: &mut TcpStream, lib: &SharedLibrary, id: &str) -> std::io::Result<()> {
    // Cover art ID is the album ID.
    let key = if let Some(key) = parse_album_id(id) {
        key
    } else if let Some(index) = parse_track_id(id) {
        lib.tracks
            .get(index)
            .map(album_key)
            .unwrap_or_default()
    } else {
        return send_error(stream, 0, "Invalid cover art ID");
    };
    let album = lib.catalog.album(&key);
    let first_track = album
        .and_then(|a| a.tracks.first().copied())
        .and_then(|i| lib.tracks.get(i));
    let cover = first_track.and_then(|track| {
        if let Some(source) = &lib.source {
            source.cover(track).ok().flatten()
        } else if let Location::File(path) = &track.location {
            library::read_cover(path)
        } else {
            None
        }
    });
    match cover {
        Some(data) => {
            let content_type = if data.starts_with(b"\x89PNG") {
                "image/png"
            } else {
                "image/jpeg"
            };
            send_binary(stream, &data, content_type)
        }
        None => {
            let header = "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
            stream.write_all(header.as_bytes())?;
            stream.flush()
        }
    }
}
