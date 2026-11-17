//! Dedicated search screen: find music via Deezer's catalog, download via the
//! user's preferred source (Telegram by default).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{FontWeight, RenderImage, SharedString};
use image::imageops::FilterType;
use image::{Frame, RgbaImage};
use serde_json::json;
use smallvec::SmallVec;

use gpui::img;

use super::*;
use crate::download::Transfer;

const DEEZER_TIMEOUT: Duration = Duration::from_secs(10);
const COVER_SIDE: u32 = 56;
const RESULTS: usize = 20;

#[derive(Clone)]
pub(super) struct MusicInfo {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration: Option<f64>,
    pub cover_url: Option<String>,
}

pub(super) enum DownloadState {
    SentToTelegram,
    Searching,
    Downloading(Transfer),
    Done,
    Failed(SharedString),
}

pub(super) struct SearchDownload {
    pub info: MusicInfo,
    pub state: DownloadState,
}

pub(super) struct SearchScreen {
    pub(super) query: String,
    pub(super) results: Vec<MusicInfo>,
    pub(super) searching: bool,
    pub(super) searched: bool,
    pub(super) error: Option<SharedString>,
    pub(super) request: u64,
    pub(super) last_typed: Option<Instant>,
    covers: HashMap<String, Option<Arc<RenderImage>>>,
    covers_loading: usize,
    pub(super) active_downloads: Vec<SearchDownload>,
}

impl SearchScreen {
    pub(super) fn new() -> Self {
        Self {
            query: String::new(),
            results: Vec::new(),
            searching: false,
            searched: false,
            error: None,
            request: 0,
            last_typed: None,
            covers: HashMap::new(),
            covers_loading: 0,
            active_downloads: Vec::new(),
        }
    }

    pub(super) fn needs_updates(&self) -> bool {
        self.active_downloads.iter().any(|dl| {
            matches!(
                dl.state,
                DownloadState::SentToTelegram
                    | DownloadState::Searching
                    | DownloadState::Downloading(_)
            )
        })
    }
}

fn deezer_search(query: &str) -> anyhow::Result<Vec<MusicInfo>> {
    let agent = ureq::Agent::new_with_config(
        ureq::Agent::config_builder()
            .timeout_global(Some(DEEZER_TIMEOUT))
            .build(),
    );
    let body: serde_json::Value = agent
        .get("https://api.deezer.com/search")
        .query("q", query)
        .query("limit", RESULTS.to_string())
        .call()
        .map_err(|e| anyhow::anyhow!("{e}"))?
        .body_mut()
        .read_json()
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let Some(data) = body["data"].as_array() else {
        return Ok(Vec::new());
    };
    Ok(data
        .iter()
        .filter_map(|item| {
            Some(MusicInfo {
                title: item["title"].as_str()?.to_owned(),
                artist: item["artist"]["name"].as_str().unwrap_or("").to_owned(),
                album: item["album"]["title"].as_str().unwrap_or("").to_owned(),
                duration: item["duration"].as_f64(),
                cover_url: item["album"]["cover_medium"]
                    .as_str()
                    .map(String::from),
            })
        })
        .collect())
}

fn fetch_cover_image(url: &str) -> Option<Arc<RenderImage>> {
    let agent = ureq::Agent::new_with_config(
        ureq::Agent::config_builder()
            .timeout_global(Some(DEEZER_TIMEOUT))
            .build(),
    );
    let mut response = agent.get(url).call().ok()?;
    let mut body = response
        .body_mut()
        .with_config()
        .limit(2_000_000)
        .reader();
    let mut bytes = Vec::new();
    std::io::Read::read_to_end(&mut body, &mut bytes).ok()?;
    let decoded = image::load_from_memory(&bytes).ok()?;
    let mut thumb: RgbaImage = decoded
        .resize_to_fill(COVER_SIDE, COVER_SIDE, FilterType::Triangle)
        .into_rgba8();
    for pixel in thumb.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }
    let frames: SmallVec<[Frame; 1]> = SmallVec::from_elem(Frame::new(thumb), 1);
    Some(Arc::new(RenderImage::new(frames)))
}

impl Onde {
    pub(super) fn run_search(&mut self, cx: &mut Context<Self>) {
        let query = self.search.query.trim().to_owned();
        self.search.request += 1;
        if query.is_empty() {
            self.search.searching = false;
            self.search.results.clear();
            self.search.searched = false;
            cx.notify();
            return;
        }
        let request = self.search.request;
        self.search.searching = true;
        self.search.error = None;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let found = cx
                .background_spawn(async move { deezer_search(&query) })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.search.request != request {
                    return;
                }
                this.search.searching = false;
                this.search.searched = true;
                match found {
                    Ok(results) => {
                        this.search.results = results;
                        this.load_search_covers(cx);
                    }
                    Err(error) => {
                        this.search.results.clear();
                        this.search.error = Some(format!("{error:#}").into());
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn load_search_covers(&mut self, cx: &mut Context<Self>) {
        for info in &self.search.results {
            let Some(url) = &info.cover_url else {
                continue;
            };
            if self.search.covers.contains_key(url) {
                continue;
            }
            self.search.covers.insert(url.clone(), None);
            let url = url.clone();
            self.search.covers_loading += 1;
            cx.spawn(async move |this, cx| {
                let image = cx
                    .background_spawn({
                        let url = url.clone();
                        async move { fetch_cover_image(&url) }
                    })
                    .await;
                let _ = this.update(cx, |this, cx| {
                    this.search.covers_loading -= 1;
                    if let Some(img) = image {
                        this.search.covers.insert(url, Some(img));
                    }
                    cx.notify();
                });
            })
            .detach();
        }
    }

    fn search_download(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(info) = self.search.results.get(index).cloned() else {
            return;
        };
        let download_query = if info.artist.is_empty() {
            info.title.clone()
        } else {
            format!("{} - {}", info.artist, info.title)
        };

        // Check if already downloading this track
        let already = self.search.active_downloads.iter().any(|dl| {
            dl.info.title == info.title
                && dl.info.artist == info.artist
                && !matches!(dl.state, DownloadState::Done | DownloadState::Failed(_))
        });
        if already {
            return;
        }

        // Try Telegram first
        if self.downloads.telegram_consent {
            let alive = self
                .downloads
                .telegram
                .as_mut()
                .is_some_and(|bridge| bridge.is_alive());
            if !alive {
                match crate::telegram::Telegram::start().and_then(|mut bridge| {
                    bridge.send(json!({"action": "connect"}))?;
                    Ok(bridge)
                }) {
                    Ok(bridge) => self.downloads.telegram = Some(bridge),
                    Err(error) => {
                        self.search.active_downloads.push(SearchDownload {
                            info,
                            state: DownloadState::Failed(format!("{error:#}").into()),
                        });
                        cx.notify();
                        return;
                    }
                }
                self.search.active_downloads.push(SearchDownload {
                    info,
                    state: DownloadState::Failed(
                        "Telegram relancé, réessayer dans quelques secondes.".into(),
                    ),
                });
                cx.notify();
                return;
            }
            let connected = self
                .downloads
                .telegram
                .as_ref()
                .is_some_and(|bridge| bridge.state.lock().unwrap().connected);
            if !connected {
                self.search.active_downloads.push(SearchDownload {
                    info,
                    state: DownloadState::Failed(
                        "Telegram non connecté. Aller dans Téléchargements pour connecter.".into(),
                    ),
                });
                cx.notify();
                return;
            }
            let folder = config::load().music_folder;
            let Some(folder) = folder else {
                self.search.active_downloads.push(SearchDownload {
                    info,
                    state: DownloadState::Failed("Choisir le dossier musical d'abord.".into()),
                });
                cx.notify();
                return;
            };
            let bridge = self.downloads.telegram.as_mut().unwrap();
            match bridge.send(json!({
                "action": "send",
                "text": download_query,
                "folder": folder,
            })) {
                Ok(()) => {
                    self.search.active_downloads.push(SearchDownload {
                        info,
                        state: DownloadState::SentToTelegram,
                    });
                }
                Err(error) => {
                    self.search.active_downloads.push(SearchDownload {
                        info,
                        state: DownloadState::Failed(error.to_string().into()),
                    });
                }
            }
        } else {
            // Fallback: search via yt-dlp then download the best match
            self.search.active_downloads.push(SearchDownload {
                info: info.clone(),
                state: DownloadState::Searching,
            });
            let dl_index = self.search.active_downloads.len() - 1;
            let sources = crate::download::sources();
            let source = sources.into_iter().next().unwrap();
            let query = download_query.clone();
            let folder = self.downloads.folder.clone();
            cx.spawn(async move |this, cx| {
                // First search to find the real URL
                let source_for_search = source.clone();
                let search_result = cx
                    .background_spawn(async move { source_for_search.search(&query) })
                    .await;
                let _ = this.update(cx, |this, cx| {
                    match search_result {
                        Ok(hits) if !hits.is_empty() => {
                            let hit = hits[0].clone();
                            let transfer = Transfer::default();
                            this.search.active_downloads[dl_index].state =
                                DownloadState::Downloading(transfer.clone());
                            cx.notify();
                            // Now download
                            let source = source.clone();
                            cx.spawn(async move |this, cx| {
                                let result = cx
                                    .background_spawn(async move {
                                        std::fs::create_dir_all(&folder)?;
                                        source.download(&hit, &folder, &transfer)
                                    })
                                    .await;
                                let _ = this.update(cx, |this, cx| {
                                    this.search.active_downloads[dl_index].state = match result {
                                        Ok(()) => DownloadState::Done,
                                        Err(error) => {
                                            DownloadState::Failed(format!("{error:#}").into())
                                        }
                                    };
                                    if matches!(
                                        this.search.active_downloads[dl_index].state,
                                        DownloadState::Done
                                    ) {
                                        this.refresh_library_after_download(cx);
                                    }
                                    cx.notify();
                                });
                            })
                            .detach();
                        }
                        Ok(_) => {
                            this.search.active_downloads[dl_index].state =
                                DownloadState::Failed("Aucun résultat trouvé.".into());
                            cx.notify();
                        }
                        Err(error) => {
                            this.search.active_downloads[dl_index].state =
                                DownloadState::Failed(format!("{error:#}").into());
                            cx.notify();
                        }
                    }
                });
            })
            .detach();
        }
        cx.notify();
    }

    pub(super) fn render_search(
        &mut self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let palette = pal();
        let narrow = window.viewport_size().width < px(COMPACT_WIDTH);

        // Update telegram-sent downloads: check if files arrived
        if let Some(bridge) = &self.downloads.telegram {
            let file_count = bridge.state.lock().unwrap().files.len();
            for dl in &mut self.search.active_downloads {
                if matches!(dl.state, DownloadState::SentToTelegram) {
                    // Mark as done when telegram has received new files
                    // (simplified: we can't perfectly match file to request)
                    if file_count > 0 {
                        // Keep as SentToTelegram - user sees "Envoyé"
                    }
                }
            }
        }

        let field = div()
            .flex()
            .flex_none()
            .items_center()
            .gap_3()
            .h(px(52.))
            .px_5()
            .rounded(px(14.))
            .bg(palette.rise)
            .border_1()
            .border_color(palette.tide)
            .child(icon("search", 20., palette.tx2))
            .when(self.search.query.is_empty(), |field| {
                field
                    .text_color(palette.tx3)
                    .text_size(px(16.))
                    .child("Rechercher un artiste, un titre, un album…")
            })
            .when(!self.search.query.is_empty(), |field| {
                field
                    .text_size(px(16.))
                    .child(format!("{}▏", self.search.query))
            });

        // Active downloads panel
        let has_downloads = !self.search.active_downloads.is_empty();
        let mut dl_panel = div()
            .flex()
            .flex_col()
            .gap_2()
            .p_3()
            .rounded(px(12.))
            .bg(palette.rise);
        for (index, dl) in self.search.active_downloads.iter().enumerate().rev().take(10) {
            let (label, tint, fraction) = match &dl.state {
                DownloadState::SentToTelegram => {
                    ("Envoyé à Telegram — en attente du fichier".to_owned(), GOOD, 0.5)
                }
                DownloadState::Searching => {
                    ("Recherche du morceau…".to_owned(), palette.spark, 0.1)
                }
                DownloadState::Downloading(transfer) => {
                    let p = transfer.progress();
                    (format!("{:.0} %", p * 100.0), palette.spark, p)
                }
                DownloadState::Done => ("Terminé".to_owned(), GOOD, 1.0),
                DownloadState::Failed(reason) => {
                    (format!("Échec : {reason}"), ERROR, 1.0)
                }
            };
            dl_panel = dl_panel.child(
                div()
                    .id(("search-dl-status", index))
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_size(px(13.))
                                    .font_weight(FontWeight::MEDIUM)
                                    .child(format!("{} — {}", dl.info.artist, dl.info.title)),
                            ),
                    )
                    .child(
                        div()
                            .h(px(4.))
                            .w_full()
                            .rounded(px(2.))
                            .bg(palette.rise2)
                            .child(
                                div()
                                    .h_full()
                                    .rounded(px(2.))
                                    .w(relative(fraction))
                                    .bg(tint),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(tint)
                            .child(label),
                    ),
            );
        }

        // Results
        let mut results_list = div().flex().flex_col().gap_1();
        for (index, info) in self.search.results.iter().enumerate() {
            let cover_image = info
                .cover_url
                .as_ref()
                .and_then(|url| self.search.covers.get(url)?.clone());

            let initial = info
                .artist
                .chars()
                .next()
                .or_else(|| info.title.chars().next())
                .unwrap_or('♪')
                .to_uppercase()
                .next()
                .unwrap_or('♪');

            let cover = if let Some(image) = cover_image {
                div()
                    .flex_none()
                    .size(px(56.))
                    .rounded(px(8.))
                    .overflow_hidden()
                    .child(img(gpui::ImageSource::Render(image)).size(px(56.)))
                    .into_any_element()
            } else {
                div()
                    .flex_none()
                    .size(px(56.))
                    .rounded(px(8.))
                    .bg(palette.rise2)
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(22.))
                    .font_weight(FontWeight::BOLD)
                    .text_color(palette.spark)
                    .child(initial.to_string())
                    .into_any_element()
            };

            // Check if this track is already being downloaded
            let downloading = self.search.active_downloads.iter().any(|dl| {
                dl.info.title == info.title
                    && dl.info.artist == info.artist
                    && !matches!(dl.state, DownloadState::Done | DownloadState::Failed(_))
            });

            results_list = results_list.child(
                div()
                    .id(("search-result", index))
                    .flex()
                    .items_center()
                    .gap_4()
                    .w_full()
                    .py(px(6.))
                    .px_3()
                    .rounded(px(12.))
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
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_size(px(15.))
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
                            )
                            .when(!info.album.is_empty(), |col| {
                                col.child(
                                    div()
                                        .truncate()
                                        .text_size(px(12.))
                                        .text_color(palette.tx3)
                                        .child(info.album.clone()),
                                )
                            }),
                    )
                    .when_some(info.duration, |row, seconds| {
                        row.child(
                            div()
                                .flex_none()
                                .text_size(px(12.))
                                .text_color(palette.tx3)
                                .pr_2()
                                .child(crate::library::format_time(seconds)),
                        )
                    })
                    .child(if downloading {
                        tag("En cours…").into_any_element()
                    } else {
                        btn(("search-dl", index), "Télécharger")
                            .on_click(
                                cx.listener(move |this, _, _, cx| this.search_download(index, cx)),
                            )
                            .into_any_element()
                    }),
            );
        }

        let status = if self.search.searching {
            Some("Recherche…")
        } else if self.search.searched
            && self.search.results.is_empty()
            && self.search.error.is_none()
        {
            Some("Aucun résultat.")
        } else {
            None
        };

        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .gap_4()
            .px(px(if narrow { 20. } else { 40. }))
            .pt_2()
            .pb(px(40.))
            .child(
                div()
                    .flex()
                    .items_end()
                    .gap_4()
                    .child(display("Recherche", 40.))
                    .child(
                        mono("Catalogue Deezer · Téléchargement via Telegram")
                            .pb_2()
                            .text_color(palette.tx3),
                    ),
            )
            .child(field)
            .when_some(self.search.error.clone(), |page, error| {
                page.child(div().text_size(px(13.)).text_color(ERROR).child(error))
            })
            .when_some(status, |page, status| {
                page.child(
                    div()
                        .text_size(px(13.))
                        .text_color(palette.tx2)
                        .child(status),
                )
            })
            .when(has_downloads, |page| page.child(dl_panel))
            .child(
                div()
                    .id("search-results")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(results_list),
            )
    }
}
