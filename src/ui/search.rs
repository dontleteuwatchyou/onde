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
    pub deezer_link: Option<String>,
}

pub(super) enum DownloadState {
    WaitingForConnection { query: String, folder: String },
    SentToTelegram,
    WaitingForChoice(Vec<crate::telegram::MenuButton>),
    WaitingForFile,
    Searching,
    Downloading(Transfer),
    Done,
    Failed(SharedString),
}

pub(super) struct SearchDownload {
    pub info: MusicInfo,
    pub state: DownloadState,
    /// File count at the moment this download was sent — used to detect new arrivals.
    files_at_send: usize,
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
                DownloadState::WaitingForConnection { .. }
                    | DownloadState::SentToTelegram
                    | DownloadState::WaitingForFile
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
                deezer_link: item["link"].as_str().map(String::from),
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
        let plain_query = if info.artist.is_empty() {
            info.title.clone()
        } else {
            format!("{} - {}", info.artist, info.title)
        };
        let telegram_query = if let Some(link) = &info.deezer_link {
            link.clone()
        } else {
            let clean = |s: &str| {
                let s = if let Some(i) = s.find('(') { s[..i].trim() } else { s.trim() };
                let s = if let Some(i) = s.find('[') { s[..i].trim() } else { s };
                s.to_owned()
            };
            let clean_title = clean(&info.title);
            let clean_artist = clean(&info.artist);
            if clean_artist.is_empty() {
                format!(".trk {clean_title}")
            } else {
                format!(".trk {clean_artist} {clean_title}")
            }
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
                            files_at_send: 0,
                        });
                        cx.notify();
                        return;
                    }
                }
            }
            let connected = self
                .downloads
                .telegram
                .as_ref()
                .is_some_and(|bridge| bridge.state.lock().unwrap().connected);
            if !connected {
                let folder = config::load().music_folder.unwrap_or_default().to_string_lossy().to_string();
                self.search.active_downloads.push(SearchDownload {
                    info,
                    state: DownloadState::WaitingForConnection {
                        query: telegram_query,
                        folder,
                    },
                    files_at_send: 0,
                });
                cx.notify();
                return;
            }
            let folder = config::load().music_folder;
            let Some(folder) = folder else {
                self.search.active_downloads.push(SearchDownload {
                    info,
                    state: DownloadState::Failed("Choisir le dossier musical d'abord.".into()),
                    files_at_send: 0,
                });
                cx.notify();
                return;
            };
            let bridge = self.downloads.telegram.as_mut().unwrap();
            let current_files = {
                let mut st = bridge.state.lock().unwrap();
                st.buttons.clear();
                st.files.len()
            };
            match bridge.send(json!({
                "action": "send",
                "text": telegram_query,
                "folder": folder,
            })) {
                Ok(()) => {
                    self.search.active_downloads.push(SearchDownload {
                        info,
                        state: DownloadState::SentToTelegram,
                        files_at_send: current_files,
                        });
                }
                Err(error) => {
                    self.search.active_downloads.push(SearchDownload {
                        info,
                        state: DownloadState::Failed(error.to_string().into()),
                        files_at_send: 0,
                        });
                }
            }
        } else {
            // Fallback: search via yt-dlp then download the best match
            self.search.active_downloads.push(SearchDownload {
                info: info.clone(),
                state: DownloadState::Searching,
                files_at_send: 0,
            });
            let dl_index = self.search.active_downloads.len() - 1;
            let sources = crate::download::sources();
            let source = sources.into_iter().next().unwrap();
            let query = plain_query.clone();
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

    /// Drive Telegram inline downloads: auto-click the best-matching result and mark Done on file arrival.
    /// Called from tick() so it runs regardless of which screen is visible.
    pub(super) fn tick_search_downloads(&mut self) {
        let (buttons, file_count, busy, status, connected) = self
            .downloads
            .telegram
            .as_ref()
            .map(|b| {
                let st = b.state.lock().unwrap();
                (st.buttons.clone(), st.files.len(), st.busy, st.status.clone(), st.connected)
            })
            .unwrap_or_default();

        // Auto-send queued downloads once Telegram connects.
        if connected {
            for dl in &mut self.search.active_downloads {
                if let DownloadState::WaitingForConnection { query, folder } = &dl.state {
                    let query = query.clone();
                    let folder = folder.clone();
                    if let Some(bridge) = &mut self.downloads.telegram {
                        let current_files = {
                            let mut st = bridge.state.lock().unwrap();
                            st.buttons.clear();
                            st.files.len()
                        };
                        if bridge.send(json!({
                            "action": "send",
                            "text": query,
                            "folder": folder,
                        })).is_ok() {
                            dl.files_at_send = current_files;
                            dl.state = DownloadState::SentToTelegram;
                        } else {
                            dl.state = DownloadState::Failed("Envoi échoué.".into());
                        }
                    }
                    break; // one at a time
                }
            }
        }

        // When bot returns inline results, show them as choices instead of auto-clicking.
        if let Some(dl) = self
            .search
            .active_downloads
            .iter_mut()
            .find(|dl| matches!(dl.state, DownloadState::SentToTelegram))
        {
            if !buttons.is_empty() {
                dl.state = DownloadState::WaitingForChoice(buttons);
            } else if !busy && status.contains("Échec") {
                dl.state = DownloadState::Failed(status.into());
            }
        }

        for dl in &mut self.search.active_downloads {
            if matches!(dl.state, DownloadState::WaitingForFile) && file_count > dl.files_at_send {
                dl.state = DownloadState::Done;
            }
        }
    }

    pub(super) fn pick_bot_result(&mut self, dl_index: usize, key: String) {
        if let Some(bridge) = &mut self.downloads.telegram {
            let _ = bridge.send(json!({"action": "click", "key": key}));
        }
        if let Some(dl) = self.search.active_downloads.get_mut(dl_index) {
            dl.state = DownloadState::WaitingForFile;
        }
    }

    /// Renders active search downloads for the Téléchargements panel.
    pub(super) fn render_search_downloads_panel(
        &self,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let palette = pal();
        let mut panel = div().flex().flex_col().gap_2();
        for (dl_index, dl) in self.search.active_downloads.iter().enumerate().rev().take(20) {
            let (label, tint, fraction) = match &dl.state {
                DownloadState::WaitingForConnection { .. } => {
                    ("Connexion à Telegram…".to_owned(), palette.tx3, 0.1)
                }
                DownloadState::SentToTelegram => {
                    ("En attente du bot…".to_owned(), palette.tx3, 0.3)
                }
                DownloadState::WaitingForChoice(_) => {
                    ("Choisis un résultat".to_owned(), palette.spark, 0.3)
                }
                DownloadState::WaitingForFile => {
                    ("En attente du fichier…".to_owned(), GOOD, 0.5)
                }
                DownloadState::Searching => ("Recherche…".to_owned(), palette.spark, 0.1),
                DownloadState::Downloading(transfer) => {
                    let p = transfer.progress();
                    (format!("{:.0} %", p * 100.0), palette.spark, p)
                }
                DownloadState::Done => ("Terminé".to_owned(), GOOD, 1.0),
                DownloadState::Failed(reason) => {
                    (format!("Échec : {reason}"), ERROR, 1.0)
                }
            };
            let removable = matches!(
                dl.state,
                DownloadState::Done | DownloadState::Failed(_)
            );
            panel = panel.child(
                div()
                    .id(("sdl", dl_index))
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .text_size(px(13.))
                                    .child(format!("{} — {}", dl.info.artist, dl.info.title)),
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
                                            .w(relative(fraction as f32))
                                            .bg(tint),
                                    ),
                            )
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(tint)
                                    .child(label),
                            ),
                    )
                    .when(removable, |row| {
                        row.child(
                            widgets::icon_small(("rm-sdl", dl_index), "close").on_click(
                                cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    if dl_index < this.search.active_downloads.len() {
                                        this.search.active_downloads.remove(dl_index);
                                    }
                                    cx.notify();
                                }),
                            ),
                        )
                    }),
            );
        }
        panel
    }

    pub(super) fn render_search(
        &mut self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let palette = pal();
        let narrow = window.viewport_size().width < px(COMPACT_WIDTH);

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

        // Telegram bridge status
        let tg_state = self
            .downloads
            .telegram
            .as_ref()
            .map(|b| {
                let st = b.state.lock().unwrap();
                (st.status.clone(), st.buttons.len(), st.connected)
            });

        // Active downloads panel
        let has_downloads = !self.search.active_downloads.is_empty();
        let mut dl_panel = div()
            .flex()
            .flex_col()
            .gap_3()
            .p_3()
            .rounded(px(12.))
            .bg(palette.rise);
        for (index, dl) in self.search.active_downloads.iter().enumerate().rev().take(10) {
            let mut entry = div()
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
                        )
                        .child(
                            widgets::icon_small(("rm-dl", index), "close").on_click(
                                cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    if index < this.search.active_downloads.len() {
                                        this.search.active_downloads.remove(index);
                                    }
                                    cx.notify();
                                }),
                            ),
                        ),
                );
            match &dl.state {
                DownloadState::WaitingForConnection { .. } => {
                    entry = entry.child(
                        div()
                            .text_size(px(12.))
                            .text_color(palette.tx3)
                            .child("Connexion à Telegram en cours…"),
                    );
                }
                DownloadState::SentToTelegram => {
                    let status_text = tg_state
                        .as_ref()
                        .map(|(s, _, _)| s.as_str())
                        .unwrap_or("Recherche sur Telegram…");
                    entry = entry.child(
                        div()
                            .text_size(px(12.))
                            .text_color(palette.tx3)
                            .child(status_text.to_owned()),
                    );
                }
                DownloadState::WaitingForChoice(choices) => {
                    entry = entry.child(
                        div()
                            .text_size(px(12.))
                            .text_color(palette.spark)
                            .child("Choisis le bon résultat :"),
                    );
                    let mut buttons = div().flex().flex_col().gap_1();
                    for (ci, choice) in choices.iter().enumerate() {
                        let key = choice.key.clone();
                        buttons = buttons.child(
                            div()
                                .id(("bot-choice", index * 100 + ci))
                                .px_3()
                                .py_2()
                                .rounded(px(8.))
                                .bg(palette.rise2)
                                .cursor_pointer()
                                .hover(|row| row.bg(pal().tide))
                                .child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .gap(px(1.))
                                        .child(
                                            div()
                                                .text_size(px(13.))
                                                .font_weight(FontWeight::MEDIUM)
                                                .child(choice.label.clone()),
                                        )
                                        .when(!choice.details.is_empty(), |col| {
                                            col.child(
                                                div()
                                                    .text_size(px(12.))
                                                    .text_color(pal().tx2)
                                                    .child(choice.details.clone()),
                                            )
                                        }),
                                )
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.pick_bot_result(index, key.clone());
                                    cx.notify();
                                })),
                        );
                    }
                    entry = entry.child(buttons);
                }
                DownloadState::WaitingForFile => {
                    entry = entry
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
                                        .w(relative(0.5))
                                        .bg(GOOD),
                                ),
                        )
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(GOOD)
                                .child("En attente du fichier…"),
                        );
                }
                DownloadState::Searching => {
                    entry = entry.child(
                        div()
                            .text_size(px(12.))
                            .text_color(palette.spark)
                            .child("Recherche du morceau…"),
                    );
                }
                DownloadState::Downloading(transfer) => {
                    let p = transfer.progress();
                    entry = entry
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
                                        .w(relative(p as f32))
                                        .bg(palette.spark),
                                ),
                        )
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(palette.spark)
                                .child(format!("{:.0} %", p * 100.0)),
                        );
                }
                DownloadState::Done => {
                    entry = entry.child(
                        div()
                            .text_size(px(12.))
                            .text_color(GOOD)
                            .child("Terminé"),
                    );
                }
                DownloadState::Failed(reason) => {
                    entry = entry.child(
                        div()
                            .text_size(px(12.))
                            .text_color(ERROR)
                            .child(format!("Échec : {reason}")),
                    );
                }
            }
            dl_panel = dl_panel.child(entry);
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
                        mono("Catalogue Deezer")
                            .pb_2()
                            .text_color(palette.tx3),
                    ),
            )
            .child({
                let use_telegram = self.downloads.telegram_consent;
                let chip = |id: &'static str, label: &'static str, active: bool| {
                    let bg = if active { palette.tide } else { palette.deep };
                    let tx = if active { palette.ink } else { palette.tx2 };
                    div()
                        .id(id)
                        .cursor_pointer()
                        .px_3()
                        .py(px(6.))
                        .rounded(px(20.))
                        .bg(bg)
                        .text_size(px(13.))
                        .text_color(tx)
                        .child(label)
                };
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(palette.tx3)
                            .child("Source"),
                    )
                    .child(
                        chip("src-telegram", "Telegram", use_telegram)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.downloads.telegram_consent = true;
                                cx.notify();
                            })),
                    )
                    .child(
                        chip("src-youtube", "YouTube", !use_telegram)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.downloads.telegram_consent = false;
                                cx.notify();
                            })),
                    )
            })
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
