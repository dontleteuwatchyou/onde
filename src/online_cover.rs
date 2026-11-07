//! Finds a public address for a cover, because Discord only shows images it can
//! fetch itself. The artist and the album or title go to Apple's search and then
//! to Deezer's. When they know nothing of the track, the cover alone goes to a
//! temporary image host: reduced, so nothing else from the file comes along.

use std::io::Cursor;
use std::time::Duration;

use image::ImageFormat;
use image::imageops::FilterType;
use serde_json::Value;

use crate::library::UNKNOWN_ARTIST;

const TIMEOUT: Duration = Duration::from_secs(6);
/// Discord refuses longer addresses.
const LONGEST_URL: usize = 256;
const RESULTS: &str = "25";
const UPLOAD_TIMEOUT: Duration = Duration::from_secs(30);
const TEMPORARY_HOST: &str = "https://litterbox.catbox.moe/resources/internals/api.php";
const TEMPORARY_LINKS: &str = "litter.catbox.moe";
const LASTING_HOST: &str = "https://catbox.moe/user/api.php";
const UPLOAD_LIFETIME: &str = "24h";
const UPLOADED_SIDE: u32 = 512;
const BOUNDARY: &str = "----OndeCoverBoundary";
const COUNTRY: &str = "FR";
/// What separates artists in a tag, in lower case.
const SEPARATORS: [&str; 10] = [
    " & ", ", ", " feat. ", " feat ", " ft. ", " ft ", " x ", " / ", "; ", " et ",
];

#[derive(Clone, Copy)]
enum Kind {
    Album,
    Song,
}

/// The cover's bytes, read only if the searches find nothing.
pub type Picture = Box<dyn FnOnce() -> Option<Vec<u8>> + Send>;

pub fn find_or_upload(
    artist: &str,
    album: &str,
    title: &str,
    picture: Option<Picture>,
) -> Option<String> {
    find(artist, album, title).or_else(|| upload(TEMPORARY_HOST, Some(UPLOAD_LIFETIME), &reduced(&picture?()?)?))
}

/// A square JPEG: small to send, and without what the original carried.
fn reduced(bytes: &[u8]) -> Option<Vec<u8>> {
    let picture = image::load_from_memory(bytes)
        .ok()?
        .resize_to_fill(UPLOADED_SIDE, UPLOADED_SIDE, FilterType::Triangle)
        .into_rgb8();
    let mut out = Cursor::new(Vec::new());
    picture.write_to(&mut out, ImageFormat::Jpeg).ok()?;
    Some(out.into_inner())
}

/// For hosting ahead of time: no expiry, and no way to remove it afterwards.
pub fn upload_lasting(bytes: &[u8]) -> Option<String> {
    upload(LASTING_HOST, None, &reduced(bytes)?)
}

/// A link that stays valid is worth remembering. A temporary one is not.
pub fn lasts(url: &str) -> bool {
    !url.contains(TEMPORARY_LINKS)
}

fn upload_body(lifetime: Option<&str>, jpeg: &[u8]) -> Vec<u8> {
    let mut body = Vec::with_capacity(jpeg.len() + 512);
    let time = lifetime.map(|value| ("time", value));
    for (name, value) in [Some(("reqtype", "fileupload")), time].into_iter().flatten() {
        body.extend_from_slice(
            format!(
                "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n"
            )
            .as_bytes(),
        );
    }
    body.extend_from_slice(
        format!(
            "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"fileToUpload\"; filename=\"cover.jpg\"\r\nContent-Type: image/jpeg\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(jpeg);
    body.extend_from_slice(format!("\r\n--{BOUNDARY}--\r\n").as_bytes());
    body
}

fn upload(address: &str, lifetime: Option<&str>, jpeg: &[u8]) -> Option<String> {
    let agent = ureq::Agent::new_with_config(
        ureq::Agent::config_builder()
            .timeout_global(Some(UPLOAD_TIMEOUT))
            .build(),
    );
    let reply = agent
        .post(address)
        .header(
            "Content-Type",
            format!("multipart/form-data; boundary={BOUNDARY}"),
        )
        .send(&upload_body(lifetime, jpeg)[..])
        .ok()?
        .body_mut()
        .read_to_string()
        .ok()?;
    usable(reply.trim().to_owned())
}

fn find(artist: &str, album: &str, title: &str) -> Option<String> {
    if artist == UNKNOWN_ARTIST || simplified(artist).is_empty() {
        return None;
    }
    let agent = ureq::Agent::new_with_config(
        ureq::Agent::config_builder()
            .timeout_global(Some(TIMEOUT))
            .build(),
    );
    let mut searches = Vec::new();
    if !album.is_empty() {
        searches.push((Kind::Album, album));
    }
    if !title.is_empty() {
        searches.push((Kind::Song, title));
    }
    searches.into_iter().find_map(|(kind, name)| {
        search_apple(&agent, kind, artist, name)
            .or_else(|| search_deezer(&agent, kind, artist, name))
    })
}

fn search_apple(agent: &ureq::Agent, kind: Kind, artist: &str, name: &str) -> Option<String> {
    let entity = match kind {
        Kind::Album => "album",
        Kind::Song => "song",
    };
    let reply: Value = agent
        .get("https://itunes.apple.com/search")
        .query("term", search_text(artist, name))
        .query("entity", entity)
        .query("country", COUNTRY)
        .query("limit", RESULTS)
        .call()
        .ok()?
        .body_mut()
        .read_json()
        .ok()?;
    pick_apple(&reply, kind, artist, name)
}

fn search_deezer(agent: &ureq::Agent, kind: Kind, artist: &str, name: &str) -> Option<String> {
    let address = match kind {
        Kind::Album => "https://api.deezer.com/search/album",
        Kind::Song => "https://api.deezer.com/search/track",
    };
    let reply: Value = agent
        .get(address)
        .query("q", search_text(artist, name))
        .query("limit", RESULTS)
        .call()
        .ok()?
        .body_mut()
        .read_json()
        .ok()?;
    pick_deezer(&reply, kind, artist, name)
}

fn pick_apple(reply: &Value, kind: Kind, artist: &str, name: &str) -> Option<String> {
    let field = match kind {
        Kind::Album => "collectionName",
        Kind::Song => "trackName",
    };
    reply["results"]
        .as_array()?
        .iter()
        .filter(|result| {
            same_artist(result["artistName"].as_str(), artist)
                && same_name(result[field].as_str(), name)
        })
        .find_map(|result| usable(result["artworkUrl100"].as_str()?.replace("100x100bb", "512x512bb")))
}

fn pick_deezer(reply: &Value, kind: Kind, artist: &str, name: &str) -> Option<String> {
    reply["data"]
        .as_array()?
        .iter()
        .filter(|result| {
            same_artist(result["artist"]["name"].as_str(), artist)
                && same_name(result["title"].as_str(), name)
        })
        .find_map(|result| {
            let cover = match kind {
                Kind::Album => &result["cover_big"],
                Kind::Song => &result["album"]["cover_big"],
            };
            usable(cover.as_str()?.to_owned())
        })
}

fn usable(url: String) -> Option<String> {
    (url.starts_with("https://") && url.len() <= LONGEST_URL).then_some(url)
}

/// Services find little with every guest in the query, or "(feat. Someone)".
fn search_text(artist: &str, name: &str) -> String {
    let artist = artists(artist).into_iter().next().unwrap_or_default();
    let name = name.split(['(', '[']).next().unwrap_or(name).trim();
    format!("{artist} {name}")
}

/// Each artist of a tag, in lower case.
fn artists(tag: &str) -> Vec<String> {
    let mut parts = vec![tag.to_lowercase()];
    for separator in SEPARATORS {
        parts = parts
            .iter()
            .flat_map(|part| part.split(separator))
            .map(str::to_owned)
            .collect();
    }
    parts
        .iter()
        .map(|part| part.trim().to_owned())
        .filter(|part| !part.is_empty())
        .collect()
}

fn without_accent(c: char) -> char {
    match c {
        'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' => 'a',
        'ç' => 'c',
        'è' | 'é' | 'ê' | 'ë' => 'e',
        'ì' | 'í' | 'î' | 'ï' => 'i',
        'ñ' => 'n',
        'ò' | 'ó' | 'ô' | 'õ' | 'ö' => 'o',
        'ù' | 'ú' | 'û' | 'ü' => 'u',
        'ý' | 'ÿ' => 'y',
        other => other,
    }
}

fn simplified(text: &str) -> String {
    text.chars()
        .flat_map(char::to_lowercase)
        .map(without_accent)
        .filter(|c| c.is_alphanumeric())
        .collect()
}

/// Strict, as the wrong cover is worse than none: "Data Punk" is not "Daft Punk".
/// One artist in common is enough: credits differ between a tag and a service.
fn same_artist(found: Option<&str>, artist: &str) -> bool {
    let found: Vec<String> = artists(found.unwrap_or_default())
        .iter()
        .map(|name| simplified(name))
        .collect();
    artists(artist)
        .iter()
        .map(|name| simplified(name))
        .any(|name| !name.is_empty() && found.contains(&name))
}

/// Allows for "Discovery (Deluxe Edition)".
fn same_name(found: Option<&str>, name: &str) -> bool {
    let (found, name) = (simplified(found.unwrap_or_default()), simplified(name));
    !found.is_empty() && (found.starts_with(&name) || name.starts_with(&found))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn apple_gives_the_large_image_of_the_right_album() {
        let reply = json!({ "results": [
            { "artistName": "Autre", "collectionName": "Discovery", "artworkUrl100": "https://a/100x100bb.jpg" },
            { "artistName": "Daft Punk", "collectionName": "Homework", "artworkUrl100": "https://b/100x100bb.jpg" },
            { "artistName": "Daft Punk", "collectionName": "Discovery (Deluxe)", "artworkUrl100": "https://c/100x100bb.jpg" },
        ]});
        assert_eq!(
            pick_apple(&reply, Kind::Album, "daft punk", "Discovery").as_deref(),
            Some("https://c/512x512bb.jpg")
        );
    }

    #[test]
    fn what_is_sent_is_a_small_square_and_nothing_else() {
        let picture = image::RgbImage::from_pixel(900, 500, image::Rgb([200, 40, 20]));
        let mut png = Cursor::new(Vec::new());
        picture.write_to(&mut png, ImageFormat::Png).unwrap();
        let jpeg = reduced(&png.into_inner()).unwrap();
        let sent = image::load_from_memory(&jpeg).unwrap();
        assert_eq!((sent.width(), sent.height()), (UPLOADED_SIDE, UPLOADED_SIDE));
        assert_eq!(reduced(b"<html>"), None);
    }

    #[test]
    fn the_upload_form_holds_its_fields_and_the_image() {
        let lasting = String::from_utf8(upload_body(None, b"IMAGE")).unwrap();
        assert!(!lasting.contains("name=\"time\""));
        let body = upload_body(Some("24h"), b"IMAGE");
        let text = String::from_utf8(body).unwrap();
        assert!(text.contains("name=\"reqtype\"\r\n\r\nfileupload\r\n"));
        assert!(text.contains("name=\"time\"\r\n\r\n24h\r\n"));
        assert!(text.contains("filename=\"cover.jpg\""));
        assert!(text.ends_with(&format!("IMAGE\r\n--{BOUNDARY}--\r\n")));
    }

    #[test]
    fn only_lasting_links_are_worth_remembering() {
        assert!(!lasts("https://litter.catbox.moe/a.jpg"));
        assert!(lasts("https://files.catbox.moe/a.jpg"));
    }

    #[test]
    fn one_artist_in_common_is_enough() {
        assert!(same_artist(Some("Zola"), "Zola & Kalash, Prince Waly"));
        assert!(same_artist(Some("Kalash feat. Zola"), "Zola x Beyoncé"));
        assert!(same_artist(Some("Beyonce"), "Beyoncé"));
        assert!(!same_artist(Some("Zolaa"), "Zola & Kalash"));
        assert!(!same_artist(None, "Zola"));
    }

    #[test]
    fn the_search_keeps_the_first_artist_and_the_plain_title() {
        assert_eq!(
            search_text("Zola & Kalash, Prince Waly", "Netflix Chill (feat. Kalash)"),
            "zola Netflix Chill"
        );
    }

    #[test]
    fn deezer_refuses_a_similar_artist() {
        let reply = json!({ "data": [
            { "title": "Discovery", "artist": { "name": "Data Punk" }, "cover_big": "https://a" },
        ]});
        assert_eq!(pick_deezer(&reply, Kind::Album, "Daft Punk", "Discovery"), None);
    }

    #[test]
    fn deezer_keeps_the_cover_of_a_song_in_its_album() {
        let reply = json!({ "data": [
            { "title": "One More Time", "artist": { "name": "Daft Punk" }, "album": { "cover_big": "https://a" } },
        ]});
        assert_eq!(
            pick_deezer(&reply, Kind::Song, "Daft Punk", "One More Time").as_deref(),
            Some("https://a")
        );
    }

    #[test]
    fn an_address_discord_would_refuse_is_left_out() {
        assert_eq!(usable("http://a".into()), None);
        assert_eq!(usable(format!("https://{}", "a".repeat(300))), None);
    }

    #[test]
    fn an_unknown_artist_is_not_searched() {
        assert_eq!(find(UNKNOWN_ARTIST, "Album", "Titre"), None);
        assert_eq!(find("  ", "Album", "Titre"), None);
    }

    #[test]
    #[ignore = "interroge Apple et Deezer"]
    fn finds_a_real_cover() {
        let album = find("Daft Punk", "Discovery", "One More Time").unwrap();
        assert!(album.starts_with("https://"), "{album}");
        let single = find("Daft Punk", "", "One More Time").unwrap();
        assert!(single.starts_with("https://"), "{single}");
        assert_eq!(find("Zzqx Personne", "Rien du tout", "Non plus"), None);
        let guests = find("Zola & Kalash, Prince Waly", "", "NETFLIX CHILL").unwrap();
        assert!(guests.starts_with("https://"), "{guests}");
    }
}
