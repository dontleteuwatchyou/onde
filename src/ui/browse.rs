//! Home, the library, album and artist pages.

use std::ops::Range;
use std::rc::Rc;

use gpui::{Div, FontWeight, Stateful, linear_color_stop, linear_gradient};

use super::*;
use crate::catalog::matching_tracks;
use crate::collections::track_id;
use crate::theme::{alpha, mix};
use crate::visuals::paint_corners;

const CARD_MIN: f32 = 160.;
const CARD_GAP: f32 = 16.;
const PAGE_PADDING: f32 = 32.;
const ROW_HEIGHT: f32 = 44.;
const CARD_TEXT: f32 = 76.;
const QUEUE_WIDTH: f32 = 388.;
pub(super) const WIDE_LIST: f32 = 780.;

#[derive(Clone, Copy, PartialEq)]
pub(super) enum LibraryView {
    Tracks,
    Albums,
    Artists,
}

impl Onde {
    pub(super) fn refilter(&mut self) {
        let query = self.library_query.clone();
        self.visible_tracks = matching_tracks(&self.queue.tracks, &query, self.track_sort);
        self.visible_albums = self.catalog.matching_albums(&query);
        self.visible_artists = self.catalog.matching_artists(&query);
    }

    pub(super) fn catalog_changed(&mut self) {
        self.catalog = Catalog::build(&self.queue.tracks);
        self.track_index = self
            .queue
            .tracks
            .iter()
            .enumerate()
            .map(|(index, track)| (track_id(track), index))
            .collect();
        self.refilter();
        self.restart_hosting();
    }

    pub(super) fn content_width(&self, window: &Window) -> f32 {
        let queue = if self.show_queue { QUEUE_WIDTH } else { 0. };
        f32::from(window.viewport_size().width) - queue - PAGE_PADDING * 2.
    }

    /// How many cards fit in a row, and how wide each is.
    fn card_layout(&self, window: &Window) -> (usize, f32) {
        let width = self.content_width(window).max(CARD_MIN);
        let columns = (((width + CARD_GAP) / (CARD_MIN + CARD_GAP)) as usize).max(1);
        let card = (width - CARD_GAP * (columns - 1) as f32) / columns as f32;
        (columns, card)
    }

    fn album_card(&mut self, album: usize, width: f32, cx: &mut Context<Self>) -> Stateful<Div> {
        let palette = pal();
        let (key, title, artist) = {
            let album = &self.catalog.albums[album];
            (album.key.clone(), album.title.clone(), album.artist.clone())
        };
        let art = self.art(&key, cx);
        let playing = self
            .queue
            .current_track()
            .is_some_and(|track| album_key(track) == key);
        div()
            .id(("album", album))
            .w(px(width))
            .flex_none()
            .flex()
            .flex_col()
            .gap_1()
            .p_2()
            .rounded(px(16.))
            .cursor_pointer()
            .when(playing, |card| card.border_1().border_color(palette.spark))
            .hover(|card| card.bg(pal().rise))
            .on_click(cx.listener(move |this, _, _, cx| this.go(Screen::Album(key.clone()), cx)))
            .child(widgets::cover(art.as_ref(), width - 16., 10.))
            .child(
                div()
                    .pt_1()
                    .truncate()
                    .font_weight(FontWeight::MEDIUM)
                    .child(title),
            )
            .child(
                div()
                    .truncate()
                    .text_size(px(12.))
                    .line_height(px(16.))
                    .text_color(palette.tx2)
                    .child(artist),
            )
    }

    fn album_row(&mut self, albums: &[usize], width: f32, cx: &mut Context<Self>) -> Div {
        let mut row = div().w_full().flex().gap(px(CARD_GAP));
        for &album in albums {
            row = row.child(self.album_card(album, width, cx));
        }
        row
    }

    fn section(
        &mut self,
        title: &'static str,
        albums: &[usize],
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<Div> {
        if albums.is_empty() {
            return None;
        }
        let (columns, width) = self.card_layout(window);
        let shown = &albums[..albums.len().min(columns)];
        Some(
            div()
                .flex()
                .flex_col()
                .gap_3()
                .pt_8()
                .child(h2(title))
                .child(self.album_row(shown, width, cx)),
        )
    }

    /// A visual on a canvas behind what's added to it.
    fn stage(&mut self, art: Option<Arc<CoverArt>>, height: f32) -> Div {
        let palette = pal();
        let lit = self.show_visuals;
        let (visual, sound, state) = (
            self.visual,
            self.analyzer.clone(),
            self.visual_state.clone(),
        );
        div()
            .relative()
            .w_full()
            .h(px(height))
            .flex_none()
            .bg(palette.deep)
            .when(lit, |stage| {
                stage.child(
                    canvas(
                        |_, _, _| {},
                        move |bounds, _, window, _| {
                            visuals::paint(
                                visual,
                                bounds,
                                &sound.borrow(),
                                &palette,
                                &mut state.borrow_mut(),
                                art.as_ref().map(|art| &art.grid),
                                false,
                                window,
                            );
                        },
                    )
                    .absolute()
                    .size_full(),
                )
            })
    }

    fn hero(&mut self, window: &Window, cx: &mut Context<Self>) -> Div {
        const HEIGHT: f32 = 340.;
        let palette = pal();
        let width = f32::from(window.viewport_size().width);
        let featured = self
            .queue
            .current_track()
            .map(album_key)
            .or_else(|| {
                self.resume_albums()
                    .first()
                    .map(|&album| self.catalog.albums[album].key.clone())
            })
            .or_else(|| {
                self.catalog
                    .recent_albums(1)
                    .first()
                    .map(|&album| self.catalog.albums[album].key.clone())
            });
        let art = featured.as_ref().and_then(|key| self.art(key, cx));
        let track = self.queue.current_track().cloned();
        let playing = track.is_some();
        let album = featured.as_ref().and_then(|key| self.catalog.album(key));
        let first = album.and_then(|album| album.tracks.first().copied());
        let (title, detail) = match (&track, album) {
            (Some(track), _) if track.album.trim().is_empty() => {
                (track.title.clone(), track.artist.clone())
            }
            (Some(track), _) => (
                track.title.clone(),
                format!("{} — {}", track.artist, track.album),
            ),
            (None, Some(album)) => (
                album.title.clone(),
                format!("{} — {} morceaux", album.artist, album.tracks.len()),
            ),
            (None, None) => ("L’eau est encore calme.".to_owned(), String::new()),
        };
        let format = track.map(|track| track.format).unwrap_or_default();
        let paused = self.paused();
        let empty = album.is_none() && !playing;
        let size = (width * 0.06).clamp(40., 88.);

        let mut buttons = div().flex().flex_wrap().gap_2().pt_3();
        if playing {
            buttons = buttons
                .child(
                    primary("hero-play", if paused { "Lecture" } else { "Pause" })
                        .on_click(cx.listener(|this, _, _, cx| this.toggle_play(cx))),
                )
                .child(
                    btn("hero-full", "Plein écran")
                        .on_click(cx.listener(|this, _, _, cx| this.toggle_fullscreen(cx))),
                );
        } else if let Some(first) = first {
            buttons = buttons.child(
                primary("hero-start", "Écouter")
                    .on_click(cx.listener(move |this, _, _, cx| this.start(first, cx))),
            );
        } else {
            buttons = buttons
                .child(
                    primary("hero-folder", "Choisir un dossier")
                        .on_click(cx.listener(|this, _, _, cx| this.pick_music_folder(cx))),
                )
                .child(
                    btn("hero-sources", "Voir les sources")
                        .on_click(cx.listener(|this, _, _, cx| this.go(Screen::Sources, cx))),
                );
        }
        if let Some(key) = featured.filter(|_| !empty) {
            buttons = buttons.child(ghost("hero-album", "Voir l’album").on_click(cx.listener(
                move |this, _, _, cx| this.go(Screen::Album(key.clone()), cx),
            )));
        }

        self.stage(art.clone(), HEIGHT)
            .child(
                div()
                    .absolute()
                    .bottom_0()
                    .left_0()
                    .w_full()
                    .h(relative(1.))
                    .bg(linear_gradient(
                        180.,
                        linear_color_stop(alpha(palette.deep, 0.15), 0.),
                        linear_color_stop(palette.deep, 1.),
                    )),
            )
            .child(
                canvas(
                    |_, _, _| {},
                    move |bounds, _, window, _| {
                        paint_corners(bounds, 24., palette.ink, window);
                    },
                )
                .absolute()
                .size_full(),
            )
            .child(
                div()
                    .absolute()
                    .bottom_0()
                    .left_0()
                    .w_full()
                    .p_6()
                    .flex()
                    .items_end()
                    .gap_6()
                    .when(!empty && width > COMPACT_WIDTH, |row| {
                        row.child(widgets::cover(art.as_ref(), 216., 16.))
                    })
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .when(!format.is_empty(), |text| text.child(div().flex().child(tag(format))))
                            .child(display(title, size).line_clamp(2).overflow_hidden())
                            .when(empty, |text| {
                                text.child(div().text_color(palette.tx2).child(
                                    "Aucun morceau pour l’instant. Indique un dossier de musique ou connecte ton serveur : Onde réunit tout au même endroit.",
                                ))
                            })
                            .when(!detail.is_empty(), |text| {
                                text.child(div().truncate().text_size(px(16.)).child(detail))
                            })
                            .child(buttons),
                    ),
            )
    }

    fn resume_albums(&self) -> Vec<usize> {
        self.recent
            .iter()
            .filter_map(|key| self.catalog.album_index(key))
            .collect()
    }

    pub(super) fn render_home(
        &mut self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let hero = self.hero(window, cx);
        let resume = self.resume_albums();
        let (columns, _) = self.card_layout(window);
        let recent = self.catalog.recent_albums(columns);
        let resume = self.section("Reprendre", &resume, window, cx);
        let recent = self.section("Ajouts récents", &recent, window, cx);
        div()
            .id("home")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .px_8()
            .pt_2()
            .pb(px(56.))
            .flex()
            .flex_col()
            .child(hero)
            .when_some(resume, |page, section| page.child(section))
            .when_some(recent, |page, section| page.child(section))
            .when(self.scanning, |page| {
                page.child(
                    div()
                        .pt_6()
                        .text_color(pal().tx2)
                        .child("Lecture de la musique…"),
                )
            })
    }

    /// With a number it sits in an album's page, which already shows the rest.
    /// With a list, playing it follows that list.
    pub(super) fn track_row(
        &mut self,
        index: usize,
        number: Option<u32>,
        list: Option<Rc<Vec<usize>>>,
        wide: bool,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let palette = pal();
        let is_current = self.queue.current == Some(index);
        let track = self.queue.tracks[index].clone();
        let art = self.art(&album_key(&track), cx);
        let liked = self.is_liked(index);
        div()
            .id(("track", index))
            .w_full()
            .h(px(ROW_HEIGHT))
            .flex()
            .items_center()
            .gap_3()
            .px_2()
            .rounded(px(10.))
            .cursor_pointer()
            .when(is_current, |row| row.bg(palette.rise))
            .hover(|row| row.bg(pal().rise))
            .on_click(cx.listener(move |this, _, _, cx| match &list {
                Some(list) => this.start_in(index, list.to_vec(), cx),
                None => this.start(index, cx),
            }))
            .when_some(number, |row, number| {
                let label = if number > 0 {
                    number.to_string()
                } else {
                    "·".to_owned()
                };
                row.child(mono(label).w(px(28.)).flex_none().text_color(palette.tx3))
            })
            .when(number.is_none(), |row| {
                row.child(widgets::cover(art.as_ref(), 32., 6.))
            })
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(if is_current { palette.spark } else { palette.tx })
                    .child(track.title.clone()),
            )
            .child(
                div()
                    .w(relative(0.2))
                    .flex_none()
                    .truncate()
                    .text_size(px(13.))
                    .text_color(palette.tx2)
                    .child(track.artist.clone()),
            )
            .when(wide && number.is_none(), |row| {
                row.child(
                    div()
                        .w(relative(0.2))
                        .flex_none()
                        .truncate()
                        .text_size(px(13.))
                        .text_color(palette.tx2)
                        .child(track.album.clone()),
                )
            })
            .when(wide, |row| {
                row.child(div().w(px(150.)).flex_none().flex().when(
                    !track.format.is_empty(),
                    |cell| cell.child(tag(track.format.clone())),
                ))
            })
            .child(
                mono(format_time(track.duration))
                    .w(px(44.))
                    .flex_none()
                    .text_color(palette.tx3),
            )
            .child(
                icon_small(
                    ("like", index),
                    if liked { "heart-fill" } else { "heart" },
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.toggle_like(index, cx);
                })),
            )
            .child(
                icon_small(("add", index), "listplus").on_click(cx.listener(
                    move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.open_add_menu(index, cx);
                    },
                )),
            )
            .when(wide, |row| {
                row.child(
                    icon_small(("verify", index), "sliders").on_click(cx.listener(
                        move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.inspect_track(index, cx);
                        },
                    )),
                )
            })
    }

    fn render_track_list(&mut self, cx: &mut Context<Self>) -> AnyElement {
        if self.visible_tracks.is_empty() {
            return self.library_notice();
        }
        uniform_list(
            "tracks",
            self.visible_tracks.len(),
            cx.processor(|this, range: Range<usize>, window, cx| {
                let wide = this.content_width(window) > WIDE_LIST;
                range
                    .map(|row| {
                        let index = this.visible_tracks[row];
                        this.track_row(index, None, None, wide, cx)
                    })
                    .collect()
            }),
        )
        .flex_1()
        .w_full()
        .into_any_element()
    }

    fn album_grid(
        &mut self,
        id: &'static str,
        albums: Vec<usize>,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (columns, _) = self.card_layout(window);
        let rows = albums.len().div_ceil(columns);
        uniform_list(
            id,
            rows,
            cx.processor(move |this, range: Range<usize>, window, cx| {
                let (columns, width) = this.card_layout(window);
                range
                    .map(|row| {
                        let start = row * columns;
                        let end = (start + columns).min(albums.len());
                        let shown = albums[start..end].to_vec();
                        this.album_row(&shown, width, cx).h(px(width + CARD_TEXT))
                    })
                    .collect()
            }),
        )
        .flex_1()
        .w_full()
        .into_any_element()
    }

    fn render_artist_list(&mut self, cx: &mut Context<Self>) -> AnyElement {
        if self.visible_artists.is_empty() {
            return self.library_notice();
        }
        uniform_list(
            "artists",
            self.visible_artists.len(),
            cx.processor(|this, range: Range<usize>, _, cx| {
                let palette = pal();
                range
                    .map(|row| {
                        let artist = &this.catalog.artists[this.visible_artists[row]];
                        let name = artist.name.clone();
                        let detail =
                            format!("{} albums · {} morceaux", artist.albums.len(), artist.tracks);
                        div()
                            .id(("artist", row))
                            .w_full()
                            .h(px(ROW_HEIGHT))
                            .flex()
                            .items_center()
                            .justify_between()
                            .px_2()
                            .rounded(px(10.))
                            .cursor_pointer()
                            .hover(|row| row.bg(pal().rise))
                            .child(
                                div()
                                    .truncate()
                                    .font_weight(FontWeight::MEDIUM)
                                    .child(name.clone()),
                            )
                            .child(mono(detail).text_color(palette.tx3))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.go(Screen::Artist(name.clone()), cx);
                            }))
                    })
                    .collect()
            }),
        )
        .flex_1()
        .w_full()
        .into_any_element()
    }

    pub(super) fn library_notice(&self) -> AnyElement {
        let text = if self.scanning {
            "Lecture de la musique…"
        } else if self.library_query.is_empty() {
            "Rien ici pour l’instant."
        } else {
            "Rien ne correspond à cette recherche."
        };
        div()
            .flex_1()
            .flex()
            .items_center()
            .justify_center()
            .text_color(pal().tx2)
            .child(text)
            .into_any_element()
    }

    pub(super) fn render_library(
        &mut self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let palette = pal();
        let view = self.library_view;
        let count = match view {
            LibraryView::Tracks => format!("{} morceaux", self.visible_tracks.len()),
            LibraryView::Albums => format!("{} albums", self.visible_albums.len()),
            LibraryView::Artists => format!("{} artistes", self.visible_artists.len()),
        };
        let typing = self.library_typing;
        let query = self.library_query.clone();
        let field = div()
            .id("library-search")
            .flex_1()
            .min_w(px(160.))
            .max_w(px(420.))
            .h(px(36.))
            .flex()
            .items_center()
            .gap_2()
            .px_3()
            .rounded(px(10.))
            .bg(palette.rise)
            .border_1()
            .border_color(if typing { palette.tide } else { palette.line })
            .cursor_text()
            .on_click(cx.listener(|this, _, _, cx| {
                this.library_typing = true;
                cx.notify();
            }))
            .child(icon("search", 16., palette.tx2))
            .child(if query.is_empty() && !typing {
                div()
                    .text_size(px(13.))
                    .text_color(palette.tx3)
                    .child("Rechercher ( / )")
            } else {
                div().text_size(px(13.)).truncate().child(if typing {
                    format!("{query}▏")
                } else {
                    query
                })
            });
        let view_chip =
            |id: &'static str, label: &'static str, target: LibraryView, cx: &mut Context<Self>| {
                chip(id, label, view == target).on_click(cx.listener(move |this, _, _, cx| {
                    this.library_view = target;
                    cx.notify();
                }))
            };
        let sort = self.track_sort;
        let sort_chip =
            |id: &'static str, label: &'static str, target: TrackSort, cx: &mut Context<Self>| {
                chip(id, label, sort == target).on_click(cx.listener(move |this, _, _, cx| {
                    this.track_sort = target;
                    this.refilter();
                    cx.notify();
                }))
            };
        let body = match view {
            LibraryView::Tracks => self.render_track_list(cx),
            LibraryView::Albums if self.visible_albums.is_empty() => self.library_notice(),
            LibraryView::Albums => {
                let albums = self.visible_albums.clone();
                self.album_grid("albums", albums, window, cx)
            }
            LibraryView::Artists => self.render_artist_list(cx),
        };
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .gap_3()
            .px_8()
            .pt_2()
            .child(
                div()
                    .flex()
                    .items_end()
                    .gap_4()
                    .child(display("Bibliothèque", 40.))
                    .child(mono(count).pb_2().text_color(palette.tx3)),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_2()
                    .child(view_chip("view-tracks", "Morceaux", LibraryView::Tracks, cx))
                    .child(view_chip("view-albums", "Albums", LibraryView::Albums, cx))
                    .child(view_chip(
                        "view-artists",
                        "Artistes",
                        LibraryView::Artists,
                        cx,
                    ))
                    .child(field)
                    .when(view == LibraryView::Tracks, |row| {
                        row.child(sort_chip("sort-artist", "Artiste", TrackSort::Artist, cx))
                            .child(sort_chip("sort-title", "Titre", TrackSort::Title, cx))
                            .child(sort_chip("sort-added", "Récents", TrackSort::Added, cx))
                            .child(sort_chip("sort-time", "Durée", TrackSort::Duration, cx))
                    }),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .pb(px(40.))
                    .child(body),
            )
    }

    pub(super) fn render_album(
        &mut self,
        key: &str,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let palette = pal();
        let Some(album) = self.catalog.album(key) else {
            return self.library_notice();
        };
        let (title, artist, tracks) = (
            album.title.clone(),
            album.artist.clone(),
            album.tracks.clone(),
        );
        let meta = format!("{} morceaux · {}", tracks.len(), format_time(album.seconds));
        let art = self.art(key, cx);
        let width = f32::from(window.viewport_size().width);
        let side = (width * 0.2).clamp(140., 240.);
        let accent = art.as_ref().map_or(palette.tide, |art| art.accent);
        let first = tracks.first().copied();
        let artist_name = artist.clone();
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(
                div()
                    .w_full()
                    .flex_none()
                    .flex()
                    .items_end()
                    .gap_6()
                    .px_8()
                    .pt_4()
                    .pb_6()
                    .bg(linear_gradient(
                        180.,
                        linear_color_stop(mix(accent, palette.ink, 0.25), 0.),
                        linear_color_stop(palette.ink, 1.),
                    ))
                    .child(widgets::cover(art.as_ref(), side, 16.))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(
                                display(title, (width * 0.06).clamp(36., 84.))
                                    .line_clamp(2)
                                    .overflow_hidden(),
                            )
                            .child(
                                div()
                                    .id("album-artist")
                                    .font_weight(FontWeight::MEDIUM)
                                    .cursor_pointer()
                                    .child(artist)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.go(Screen::Artist(artist_name.clone()), cx);
                                    })),
                            )
                            .child(mono(meta).text_color(palette.tx2))
                            .when_some(first, |text, first| {
                                text.child(
                                    div().flex().pt_2().child(
                                        primary("album-play", "Écouter").on_click(cx.listener(
                                            move |this, _, _, cx| this.start(first, cx),
                                        )),
                                    ),
                                )
                            }),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .px_8()
                    .pb(px(40.))
                    .child(
                        uniform_list(
                            "album-tracks",
                            tracks.len(),
                            cx.processor(move |this, range: Range<usize>, window, cx| {
                                let wide = this.content_width(window) > WIDE_LIST;
                                range
                                    .map(|row| {
                                        let index = tracks[row];
                                        let number = this.queue.tracks[index].number;
                                        this.track_row(index, Some(number), None, wide, cx)
                                    })
                                    .collect()
                            }),
                        )
                        .flex_1()
                        .w_full(),
                    ),
            )
            .into_any_element()
    }

    pub(super) fn render_artist(
        &mut self,
        name: &str,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let palette = pal();
        let Some(artist) = self.catalog.artist(name) else {
            return self.library_notice();
        };
        let title = artist.name.clone();
        let albums = artist.albums.clone();
        let meta = format!("{} albums · {} morceaux", albums.len(), artist.tracks);
        let width = f32::from(window.viewport_size().width);
        let grid = self.album_grid("artist-albums", albums, window, cx);
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .gap_3()
            .px_8()
            .pt_2()
            .child(display(title, (width * 0.06).clamp(40., 84.)))
            .child(mono(meta).text_color(palette.tx2))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .pb(px(40.))
                    .child(grid),
            )
            .into_any_element()
    }
}
