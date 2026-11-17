//! Dedicated search screen: find music, see results with details, download.

use std::sync::Arc;
use std::time::Instant;

use gpui::{FontWeight, SharedString};
use serde_json::json;

use super::*;
use crate::download::{self, DownloadSource, Hit, Transfer};
use crate::telegram::MenuButton;

pub(super) enum SearchResult {
    Track(Hit, Arc<dyn DownloadSource>),
    Telegram(MenuButton),
}

pub(super) struct SearchScreen {
    pub(super) query: String,
    pub(super) results: Vec<SearchResult>,
    pub(super) searching: bool,
    pub(super) searched: bool,
    pub(super) error: Option<SharedString>,
    pub(super) request: u64,
    pub(super) last_typed: Option<Instant>,
    pub(super) use_telegram: bool,
    sources: Vec<Arc<dyn DownloadSource>>,
    selected: usize,
}

impl SearchScreen {
    pub(super) fn new() -> Self {
        let sources = download::sources();
        Self {
            query: String::new(),
            results: Vec::new(),
            searching: false,
            searched: false,
            error: None,
            request: 0,
            last_typed: None,
            use_telegram: true,
            sources,
            selected: 0,
        }
    }
}

impl Onde {
    pub(super) fn run_search(&mut self, cx: &mut Context<Self>) {
        let query = self.search.query.trim().to_owned();
        if query.is_empty() {
            self.search.searching = false;
            self.search.results.clear();
            self.search.searched = false;
            cx.notify();
            return;
        }

        if self.search.use_telegram {
            self.search_telegram(cx);
        } else {
            self.search_source(cx);
        }
    }

    fn search_telegram(&mut self, cx: &mut Context<Self>) {
        let query = self.search.query.trim().to_owned();
        if query.is_empty() {
            return;
        }
        let alive = self
            .downloads
            .telegram
            .as_mut()
            .is_some_and(|bridge| bridge.is_alive());
        if !alive && self.downloads.telegram_consent {
            match crate::telegram::Telegram::start().and_then(|mut bridge| {
                bridge.send(json!({"action": "connect"}))?;
                Ok(bridge)
            }) {
                Ok(bridge) => self.downloads.telegram = Some(bridge),
                Err(error) => {
                    self.search.error = Some(format!("{error:#}").into());
                    cx.notify();
                    return;
                }
            }
            self.search.error =
                Some("Service Telegram relancé : relancer la recherche une fois connecté.".into());
            cx.notify();
            return;
        }
        let connected = self
            .downloads
            .telegram
            .as_ref()
            .is_some_and(|bridge| bridge.state.lock().unwrap().connected);
        if !self.downloads.telegram_consent || !connected {
            self.search.error =
                Some("Accepter et connecter Telegram dans Téléchargements d'abord.".into());
            cx.notify();
            return;
        }
        let folder = config::load().music_folder;
        let Some(folder) = folder else {
            self.search.error = Some("Choisir le dossier musical d'abord.".into());
            cx.notify();
            return;
        };
        let bridge = self.downloads.telegram.as_mut().unwrap();
        match bridge.send(json!({"action": "send", "text": query, "folder": folder})) {
            Ok(()) => {
                self.search.searching = true;
                self.search.error = None;
            }
            Err(error) => {
                self.search.error = Some(error.to_string().into());
            }
        }
        cx.notify();
    }

    fn search_source(&mut self, cx: &mut Context<Self>) {
        let query = self.search.query.trim().to_owned();
        self.search.request += 1;
        if query.is_empty() {
            self.search.searching = false;
            return;
        }
        let request = self.search.request;
        let source = self.search.sources[self.search.selected].clone();
        self.search.searching = true;
        self.search.error = None;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let found = cx
                .background_spawn(async move { source.search(&query) })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.search.request != request {
                    return;
                }
                this.search.searching = false;
                this.search.searched = true;
                match found {
                    Ok(hits) => {
                        let source = this.search.sources[this.search.selected].clone();
                        this.search.results = hits
                            .into_iter()
                            .map(|hit| SearchResult::Track(hit, source.clone()))
                            .collect();
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

    fn search_download(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(result) = self.search.results.get(index) else {
            return;
        };
        match result {
            SearchResult::Telegram(button) => {
                let key = button.key.clone();
                if let Some(bridge) = &mut self.downloads.telegram {
                    if let Err(error) = bridge.send(json!({"action": "click", "key": key})) {
                        self.search.error = Some(error.to_string().into());
                    }
                }
                cx.notify();
            }
            SearchResult::Track(hit, source) => {
                let already = self.downloads.jobs.iter().any(|job| {
                    job.hit.reference == hit.reference
                        && !matches!(
                            job.state,
                            downloads::JobState::Failed(_) | downloads::JobState::Cancelled
                        )
                });
                if already {
                    return;
                }
                self.downloads.jobs.push(downloads::Job {
                    source: source.clone(),
                    hit: hit.clone(),
                    state: downloads::JobState::Waiting,
                    transfer: Transfer::default(),
                });
                self.start_next_download(cx);
                cx.notify();
            }
        }
    }

    pub(super) fn render_search(
        &mut self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let palette = pal();
        let narrow = window.viewport_size().width < px(COMPACT_WIDTH);

        // Gather telegram results when using telegram
        if self.search.use_telegram {
            if let Some(bridge) = &self.downloads.telegram {
                let state = bridge.state.lock().unwrap();
                if !state.buttons.is_empty() {
                    self.search.results = state
                        .buttons
                        .iter()
                        .cloned()
                        .map(SearchResult::Telegram)
                        .collect();
                    self.search.searching = false;
                    self.search.searched = true;
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
                    .child("Rechercher un artiste, un titre…")
            })
            .when(!self.search.query.is_empty(), |field| {
                field
                    .text_size(px(16.))
                    .child(format!("{}▏", self.search.query))
            });

        // Source selector
        let mut source_chips = div().flex().items_center().gap_2();
        let tg_on = self.search.use_telegram;
        source_chips = source_chips.child(
            div()
                .id("search-src-telegram")
                .h(px(32.))
                .flex()
                .items_center()
                .px_3()
                .rounded(px(8.))
                .cursor_pointer()
                .font_weight(FontWeight::MEDIUM)
                .text_size(px(13.))
                .when(tg_on, |c| c.bg(palette.rise2).text_color(palette.spark))
                .when(!tg_on, |c| {
                    c.text_color(palette.tx2)
                        .hover(|c| c.bg(pal().rise))
                })
                .on_click(cx.listener(|this, _, _, cx| {
                    this.search.use_telegram = true;
                    this.search.results.clear();
                    this.search.searched = false;
                    this.search.error = None;
                    if !this.search.query.is_empty() {
                        this.run_search(cx);
                    }
                    cx.notify();
                }))
                .child("Telegram"),
        );
        for (index, source) in self.search.sources.iter().enumerate() {
            let on = !self.search.use_telegram && self.search.selected == index;
            let name = source.name();
            source_chips = source_chips.child(
                div()
                    .id(("search-src", index))
                    .h(px(32.))
                    .flex()
                    .items_center()
                    .px_3()
                    .rounded(px(8.))
                    .cursor_pointer()
                    .font_weight(FontWeight::MEDIUM)
                    .text_size(px(13.))
                    .when(on, |c| c.bg(palette.rise2).text_color(palette.spark))
                    .when(!on, |c| {
                        c.text_color(palette.tx2)
                            .hover(|c| c.bg(pal().rise))
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.search.use_telegram = false;
                        this.search.selected = index;
                        this.search.results.clear();
                        this.search.searched = false;
                        this.search.error = None;
                        if !this.search.query.is_empty() {
                            this.run_search(cx);
                        }
                        cx.notify();
                    }))
                    .child(name),
            );
        }

        // Results
        let mut results_list = div().flex().flex_col().gap_2();
        for (index, result) in self.search.results.iter().enumerate() {
            let (title, artist, detail, duration) = match result {
                SearchResult::Track(hit, _) => (
                    hit.title.clone(),
                    hit.artist.clone(),
                    String::new(),
                    hit.seconds,
                ),
                SearchResult::Telegram(button) => {
                    let (artist, title) = parse_telegram_label(&button.label);
                    (title, artist, button.details.clone(), None)
                }
            };
            let initial = artist
                .chars()
                .next()
                .or_else(|| title.chars().next())
                .unwrap_or('♪')
                .to_uppercase()
                .next()
                .unwrap_or('♪');

            results_list = results_list.child(
                div()
                    .id(("search-result", index))
                    .flex()
                    .items_center()
                    .gap_4()
                    .w_full()
                    .py_2()
                    .px_3()
                    .rounded(px(12.))
                    .hover(|row| row.bg(pal().rise))
                    .child(
                        // Cover placeholder
                        div()
                            .flex_none()
                            .size(px(48.))
                            .rounded(px(8.))
                            .bg(palette.rise2)
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_size(px(20.))
                            .font_weight(FontWeight::BOLD)
                            .text_color(palette.spark)
                            .child(initial.to_string()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .child(
                                div()
                                    .truncate()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_size(px(15.))
                                    .child(title),
                            )
                            .child(
                                div()
                                    .truncate()
                                    .text_size(px(13.))
                                    .text_color(palette.tx2)
                                    .child(if artist.is_empty() {
                                        "Artiste inconnu".to_owned()
                                    } else {
                                        artist
                                    }),
                            )
                            .when(!detail.is_empty(), |col| {
                                col.child(
                                    div()
                                        .truncate()
                                        .text_size(px(12.))
                                        .text_color(palette.tx3)
                                        .child(detail),
                                )
                            }),
                    )
                    .when_some(duration, |row, seconds| {
                        row.child(
                            div()
                                .flex_none()
                                .text_size(px(12.))
                                .text_color(palette.tx3)
                                .child(crate::library::format_time(seconds)),
                        )
                    })
                    .child(
                        btn(("search-dl", index), "Télécharger").on_click(
                            cx.listener(move |this, _, _, cx| this.search_download(index, cx)),
                        ),
                    ),
            );
        }

        let status = if self.search.searching {
            Some("Recherche…")
        } else if self.search.searched && self.search.results.is_empty() && self.search.error.is_none()
        {
            Some("Aucun résultat.")
        } else {
            None
        };

        let source_label = if self.search.use_telegram {
            "Telegram · @DeezLoad2Bot"
        } else {
            self.search.sources[self.search.selected].name()
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
                        mono(source_label)
                            .pb_2()
                            .text_color(palette.tx3),
                    ),
            )
            .child(field)
            .child(source_chips)
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

fn parse_telegram_label(label: &str) -> (String, String) {
    if let Some((artist, title)) = label.split_once(" - ") {
        (artist.trim().to_owned(), title.trim().to_owned())
    } else if let Some((artist, title)) = label.split_once(" – ") {
        (artist.trim().to_owned(), title.trim().to_owned())
    } else {
        (String::new(), label.to_owned())
    }
}
