//! Spotify-style suggestions at the bottom of a playlist: similar tracks pulled
//! from Deezer's catalogue, each marked as already in the library (playable) or
//! not yet downloaded (one click fetches it and drops it back in the playlist).

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use gpui::{FontWeight, RenderImage, SharedString, img};
use serde_json::Value;

use super::search::{DownloadState, MusicInfo, fetch_cover_image};
use super::*;
use crate::collections::track_id;

const DEEZER_TIMEOUT: Duration = Duration::from_secs(10);
/// How many of a playlist's artists seed the recommendations.
const SEED_ARTISTS: usize = 3;
const MAX_SUGGESTIONS: usize = 30;

#[derive(Default)]
pub(super) struct Suggestions {
    /// The playlist the current results belong to, if any.
    pub(super) for_playlist: Option<usize>,
    pub(super) results: Vec<MusicInfo>,
    pub(super) loading: bool,
    pub(super) error: Option<SharedString>,
    /// Bumped on each request, so a slow reply can't fill the wrong playlist.
    request: u64,
    covers: HashMap<String, Option<Arc<RenderImage>>>,
}

/// Normalised key so a Deezer track and a tagged file line up despite case.
fn norm(artist: &str, title: &str) -> String {
    format!("{}\u{0}{}", artist.trim().to_lowercase(), title.trim().to_lowercase())
}

fn parse_track(item: &Value) -> Option<MusicInfo> {
    Some(MusicInfo {
        title: item["title"].as_str()?.to_owned(),
        artist: item["artist"]["name"].as_str().unwrap_or("").to_owned(),
        album: item["album"]["title"].as_str().unwrap_or("").to_owned(),
        duration: item["duration"].as_f64(),
        cover_url: item["album"]["cover_medium"].as_str().map(String::from),
        deezer_link: item["link"].as_str().map(String::from),
    })
}

fn artist_id(agent: &ureq::Agent, name: &str) -> Option<u64> {
    let body: Value = agent
        .get("https://api.deezer.com/search/artist")
        .query("q", name)
        .query("limit", "1")
        .call()
        .ok()?
        .body_mut()
        .read_json()
        .ok()?;
    body["data"][0]["id"].as_u64()
}

/// For each seed artist, pull Deezer's "radio" (a flow of similar tracks), then
/// interleave the seeds so no single artist dominates the list.
fn fetch_similar(seeds: Vec<String>) -> anyhow::Result<Vec<MusicInfo>> {
    let agent = ureq::Agent::new_with_config(
        ureq::Agent::config_builder()
            .timeout_global(Some(DEEZER_TIMEOUT))
            .build(),
    );
    let mut lists: Vec<Vec<MusicInfo>> = Vec::new();
    for seed in seeds.iter().take(SEED_ARTISTS) {
        let Some(id) = artist_id(&agent, seed) else {
            continue;
        };
        let body: Value = match agent
            .get(format!("https://api.deezer.com/artist/{id}/radio"))
            .call()
        {
            Ok(mut reply) => match reply.body_mut().read_json() {
                Ok(value) => value,
                Err(_) => continue,
            },
            Err(_) => continue,
        };
        let Some(data) = body["data"].as_array() else {
            continue;
        };
        lists.push(data.iter().filter_map(parse_track).collect());
    }

    Ok(interleave(lists))
}

/// Round-robin across the per-artist lists, dropping duplicates, capped.
fn interleave(lists: Vec<Vec<MusicInfo>>) -> Vec<MusicInfo> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    let mut row = 0;
    loop {
        let mut progressed = false;
        for list in &lists {
            if let Some(info) = list.get(row) {
                progressed = true;
                if seen.insert(norm(&info.artist, &info.title)) {
                    out.push(info.clone());
                    if out.len() >= MAX_SUGGESTIONS {
                        return out;
                    }
                }
            }
        }
        if !progressed {
            break;
        }
        row += 1;
    }
    out
}

impl Onde {
    fn resolve_ids(&self, ids: &[String]) -> Vec<usize> {
        ids.iter()
            .filter_map(|id| self.track_index.get(id).copied())
            .collect()
    }

    /// The artists of a playlist, most frequent first.
    fn seed_artists(&self, resolved: &[usize]) -> Vec<String> {
        let mut freq: HashMap<String, usize> = HashMap::new();
        let mut order: Vec<String> = Vec::new();
        for &index in resolved {
            if let Some(track) = self.queue.tracks.get(index) {
                let name = track.artist.trim();
                if name.is_empty() {
                    continue;
                }
                let key = name.to_lowercase();
                let count = freq.entry(key.clone()).or_insert(0);
                if *count == 0 {
                    order.push(name.to_owned());
                }
                *count += 1;
            }
        }
        order.sort_by_key(|name| std::cmp::Reverse(freq[&name.to_lowercase()]));
        order
    }

    /// Fetch suggestions for a playlist, unless they are already loaded or in
    /// flight for it. Called when the playlist page opens.
    pub(super) fn load_suggestions(&mut self, playlist: usize, cx: &mut Context<Self>) {
        let already = self.suggestions.for_playlist == Some(playlist)
            && (self.suggestions.loading || !self.suggestions.results.is_empty());
        if already {
            return;
        }
        let Some(found) = self.collections.playlists.get(playlist) else {
            return;
        };
        let resolved = self.resolve_ids(&found.tracks.clone());
        let seeds = self.seed_artists(&resolved);

        self.suggestions.for_playlist = Some(playlist);
        self.suggestions.results.clear();
        self.suggestions.error = None;
        self.suggestions.request += 1;
        if seeds.is_empty() {
            self.suggestions.loading = false;
            cx.notify();
            return;
        }
        self.suggestions.loading = true;
        let request = self.suggestions.request;
        cx.notify();

        cx.spawn(async move |this, cx| {
            let found = cx
                .background_spawn(async move { fetch_similar(seeds) })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.suggestions.request != request {
                    return;
                }
                this.suggestions.loading = false;
                match found {
                    Ok(mut results) => {
                        // Drop anything already sitting in the playlist.
                        let in_playlist: HashSet<String> = this
                            .collections
                            .playlists
                            .get(playlist)
                            .map(|p| {
                                this.resolve_ids(&p.tracks)
                                    .iter()
                                    .filter_map(|&i| this.queue.tracks.get(i))
                                    .map(|t| norm(&t.artist, &t.title))
                                    .collect()
                            })
                            .unwrap_or_default();
                        results.retain(|info| {
                            !in_playlist.contains(&norm(&info.artist, &info.title))
                        });
                        this.suggestions.results = results;
                        this.load_suggestion_covers(cx);
                    }
                    Err(error) => {
                        this.suggestions.error = Some(format!("{error:#}").into());
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn load_suggestion_covers(&mut self, cx: &mut Context<Self>) {
        let urls: Vec<String> = self
            .suggestions
            .results
            .iter()
            .filter_map(|info| info.cover_url.clone())
            .filter(|url| !self.suggestions.covers.contains_key(url))
            .collect();
        for url in urls {
            self.suggestions.covers.insert(url.clone(), None);
            cx.spawn(async move |this, cx| {
                let image = cx
                    .background_spawn({
                        let url = url.clone();
                        async move { fetch_cover_image(&url) }
                    })
                    .await;
                let _ = this.update(cx, |this, cx| {
                    if let Some(img) = image {
                        this.suggestions.covers.insert(url, Some(img));
                        cx.notify();
                    }
                });
            })
            .detach();
        }
    }

    /// The library track matching a suggestion, if it is already downloaded.
    fn library_match(&self, info: &MusicInfo) -> Option<usize> {
        let key = norm(&info.artist, &info.title);
        (0..self.queue.tracks.len())
            .find(|&i| norm(&self.queue.tracks[i].artist, &self.queue.tracks[i].title) == key)
    }

    fn play_suggestion(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(info) = self.suggestions.results.get(index).cloned() else {
            return;
        };
        if let Some(track) = self.library_match(&info) {
            self.start(track, cx);
        }
    }

    fn download_suggestion(&mut self, index: usize, playlist: usize, cx: &mut Context<Self>) {
        let Some(info) = self.suggestions.results.get(index).cloned() else {
            return;
        };
        self.start_download(info, Some(playlist), cx);
        self.go(Screen::Downloads, cx);
    }

    /// Once a suggestion's download lands in the library, drop it in the
    /// playlist it was fetched for. Runs each tick while a download is pending.
    pub(super) fn apply_pending_playlist_adds(&mut self, cx: &mut Context<Self>) {
        let mut adds: Vec<(usize, String)> = Vec::new();
        for dl in &mut self.search.active_downloads {
            if !matches!(dl.state, DownloadState::Done) {
                continue;
            }
            let Some(playlist) = dl.add_to_playlist else {
                continue;
            };
            let key = norm(&dl.info.artist, &dl.info.title);
            let found = self
                .queue
                .tracks
                .iter()
                .find(|track| norm(&track.artist, &track.title) == key)
                .map(track_id);
            if let Some(id) = found {
                adds.push((playlist, id));
                dl.add_to_playlist = None;
            }
        }
        if adds.is_empty() {
            return;
        }
        for (playlist, id) in adds {
            self.collections.add(playlist, &id);
        }
        let collections = self.collections.clone();
        self.save_settings(|settings| settings.collections = collections);
        cx.notify();
    }

    /// The suggestions block shown under a playlist's tracks.
    pub(super) fn suggestions_section(
        &mut self,
        playlist: usize,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if self.suggestions.for_playlist != Some(playlist) {
            return None;
        }
        let palette = pal();
        let loading = self.suggestions.loading;
        let error = self.suggestions.error.clone();
        if !loading && self.suggestions.results.is_empty() && error.is_none() {
            return None;
        }

        let status = if loading {
            "Recherche de morceaux similaires…".to_owned()
        } else if let Some(error) = &error {
            format!("Indisponible : {error}")
        } else {
            format!("{} morceaux similaires", self.suggestions.results.len())
        };

        let header = div()
            .px_8()
            .pt_2()
            .pb_1()
            .flex()
            .items_center()
            .justify_between()
            .border_t_1()
            .border_color(palette.line)
            .child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_size(px(14.))
                    .child("Suggestions"),
            )
            .child(mono(status).text_color(palette.tx3));

        let mut list = div().flex().flex_col().gap_1();
        let results = self.suggestions.results.clone();
        for (index, info) in results.iter().enumerate() {
            let cover_image = info
                .cover_url
                .as_ref()
                .and_then(|url| self.suggestions.covers.get(url)?.clone());
            let in_library = self.library_match(info).is_some();

            let initial = info
                .artist
                .chars()
                .next()
                .or_else(|| info.title.chars().next())
                .unwrap_or('\u{266a}')
                .to_uppercase()
                .next()
                .unwrap_or('\u{266a}');
            let cover = if let Some(image) = cover_image {
                div()
                    .flex_none()
                    .size(px(44.))
                    .rounded(px(8.))
                    .overflow_hidden()
                    .child(img(gpui::ImageSource::Render(image)).size(px(44.)))
                    .into_any_element()
            } else {
                div()
                    .flex_none()
                    .size(px(44.))
                    .rounded(px(8.))
                    .bg(palette.rise2)
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(18.))
                    .font_weight(FontWeight::BOLD)
                    .text_color(palette.spark)
                    .child(initial.to_string())
                    .into_any_element()
            };

            let downloading = self.search.active_downloads.iter().any(|dl| {
                dl.info.title == info.title
                    && dl.info.artist == info.artist
                    && !matches!(dl.state, DownloadState::Done | DownloadState::Failed(_))
            });

            let action = if in_library {
                btn(("suggest-play", index), "\u{25b6}")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.play_suggestion(index, cx);
                    }))
                    .into_any_element()
            } else if downloading {
                tag("En cours…").into_any_element()
            } else {
                btn(("suggest-dl", index), "Télécharger")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.download_suggestion(index, playlist, cx);
                    }))
                    .into_any_element()
            };

            list = list.child(
                div()
                    .id(("suggest-row", index))
                    .flex()
                    .items_center()
                    .gap_3()
                    .w_full()
                    .py(px(4.))
                    .px_2()
                    .rounded(px(10.))
                    .hover(|row| row.bg(pal().rise))
                    .child(cover)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(1.))
                            .child(
                                div()
                                    .truncate()
                                    .font_weight(FontWeight::MEDIUM)
                                    .child(info.title.clone()),
                            )
                            .child(
                                div()
                                    .truncate()
                                    .text_size(px(13.))
                                    .text_color(palette.tx2)
                                    .child(if info.artist.is_empty() {
                                        "Artiste inconnu".to_owned()
                                    } else {
                                        info.artist.clone()
                                    }),
                            ),
                    )
                    .child(if in_library {
                        mono("En biblio").text_color(palette.tx3).into_any_element()
                    } else {
                        div().into_any_element()
                    })
                    .child(action),
            );
        }

        Some(
            div()
                .flex_none()
                .flex()
                .flex_col()
                .child(header)
                .child(
                    div()
                        .id("suggestions-scroll")
                        .px_8()
                        .pb_4()
                        .h(px(240.))
                        .overflow_y_scroll()
                        .child(list),
                )
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn normalises_case_and_spacing() {
        assert_eq!(norm("  Daft Punk ", "One MORE Time"), norm("daft punk", "one more time"));
        assert_ne!(norm("A", "x"), norm("B", "x"));
    }

    #[test]
    fn parses_a_deezer_track() {
        let item = json!({
            "title": "Harder, Better, Faster, Stronger",
            "artist": { "name": "Daft Punk" },
            "album": { "title": "Discovery", "cover_medium": "https://cover" },
            "duration": 224,
            "link": "https://deezer/track/1"
        });
        let info = parse_track(&item).unwrap();
        assert_eq!(info.title, "Harder, Better, Faster, Stronger");
        assert_eq!(info.artist, "Daft Punk");
        assert_eq!(info.album, "Discovery");
        assert_eq!(info.duration, Some(224.0));
        assert_eq!(info.cover_url.as_deref(), Some("https://cover"));
        assert_eq!(info.deezer_link.as_deref(), Some("https://deezer/track/1"));
    }

    #[test]
    fn a_track_without_a_title_is_skipped() {
        assert!(parse_track(&json!({ "artist": { "name": "X" } })).is_none());
    }

    fn info(artist: &str, title: &str) -> MusicInfo {
        MusicInfo {
            title: title.into(),
            artist: artist.into(),
            album: String::new(),
            duration: None,
            cover_url: None,
            deezer_link: None,
        }
    }

    #[test]
    fn interleaving_alternates_seeds_and_drops_duplicates() {
        let a = vec![info("A", "a1"), info("A", "a2")];
        let b = vec![info("B", "b1"), info("A", "a1")]; // a1 is a duplicate
        let merged = interleave(vec![a, b]);
        let keys: Vec<String> = merged.iter().map(|i| norm(&i.artist, &i.title)).collect();
        assert_eq!(
            keys,
            vec![norm("A", "a1"), norm("B", "b1"), norm("A", "a2")]
        );
    }
}
