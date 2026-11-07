//! Liked tracks and playlists.

use std::ops::Range;
use std::rc::Rc;

use gpui::{Div, FontWeight, linear_color_stop, linear_gradient};

use super::browse::WIDE_LIST;
use super::*;
use crate::collections::track_id;
use crate::theme::mix;

impl Onde {
    fn save_collections(&mut self) {
        let collections = self.collections.clone();
        self.save_settings(|settings| settings.collections = collections);
    }

    /// Those no longer in the open source are left out, not forgotten.
    fn resolve(&self, ids: &[String]) -> Vec<usize> {
        ids.iter()
            .filter_map(|id| self.track_index.get(id).copied())
            .collect()
    }

    pub(super) fn is_liked(&self, index: usize) -> bool {
        self.queue
            .tracks
            .get(index)
            .is_some_and(|track| self.collections.is_liked(&track_id(track)))
    }

    pub(super) fn toggle_like(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(track) = self.queue.tracks.get(index) else {
            return;
        };
        self.collections.toggle_liked(&track_id(track));
        self.save_collections();
        cx.notify();
    }

    pub(super) fn like_current(&mut self, cx: &mut Context<Self>) {
        if let Some(current) = self.queue.current {
            self.toggle_like(current, cx);
        }
    }

    pub(super) fn open_add_menu(&mut self, index: usize, cx: &mut Context<Self>) {
        self.adding_to = Some(index);
        cx.notify();
    }

    fn close_add_menu(&mut self, cx: &mut Context<Self>) {
        self.adding_to = None;
        self.naming = None;
        cx.notify();
    }

    fn add_to_playlist(&mut self, playlist: usize, cx: &mut Context<Self>) {
        let id = self
            .adding_to
            .and_then(|index| self.queue.tracks.get(index))
            .map(track_id);
        if let Some(id) = id {
            self.collections.add(playlist, &id);
            self.save_collections();
        }
        self.close_add_menu(cx);
    }

    pub(super) fn start_naming(&mut self, cx: &mut Context<Self>) {
        self.naming = Some(String::new());
        cx.notify();
    }

    /// Enter in the name field. An empty name changes nothing.
    pub(super) fn confirm_naming(&mut self, cx: &mut Context<Self>) {
        let name = self.naming.take().unwrap_or_default();
        let Some(created) = self.collections.create(&name) else {
            cx.notify();
            return;
        };
        self.save_collections();
        if self.adding_to.is_some() {
            self.add_to_playlist(created, cx);
        }
        cx.notify();
    }

    fn remove_from_playlist(&mut self, playlist: usize, index: usize, cx: &mut Context<Self>) {
        let Some(track) = self.queue.tracks.get(index) else {
            return;
        };
        self.collections.remove(playlist, &track_id(track));
        self.save_collections();
        cx.notify();
    }

    fn delete_playlist(&mut self, playlist: usize, cx: &mut Context<Self>) {
        self.collections.delete(playlist);
        self.save_collections();
        self.go(Screen::Playlists, cx);
    }

    fn naming_field(&self) -> Div {
        let palette = pal();
        let name = self.naming.clone().unwrap_or_default();
        div()
            .h(px(44.))
            .flex_none()
            .flex()
            .items_center()
            .gap_2()
            .px_4()
            .rounded(px(10.))
            .bg(palette.rise)
            .border_1()
            .border_color(palette.tide)
            .child(icon("plus", 18., palette.tx2))
            .child(if name.is_empty() {
                div()
                    .text_color(palette.tx3)
                    .child("Nom de la playlist, puis Entrée")
            } else {
                div().truncate().child(format!("{name}▏"))
            })
    }

    fn collection_row(
        &mut self,
        id: impl Into<gpui::ElementId>,
        name: String,
        detail: String,
        first: Option<usize>,
        heart: bool,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<Div> {
        let palette = pal();
        let key = first
            .and_then(|index| self.queue.tracks.get(index))
            .map(album_key);
        let art = key.and_then(|key| self.art(&key, cx));
        div()
            .id(id)
            .w_full()
            .h(px(68.))
            .flex_none()
            .flex()
            .items_center()
            .gap_3()
            .px_2()
            .rounded(px(16.))
            .cursor_pointer()
            .hover(|row| row.bg(pal().rise))
            .child(if heart {
                div()
                    .size(px(52.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(10.))
                    .bg(linear_gradient(
                        160.,
                        linear_color_stop(palette.spark, 0.),
                        linear_color_stop(palette.tide, 1.),
                    ))
                    .child(icon("heart-fill", 26., palette.tx))
            } else {
                widgets::cover(art.as_ref(), 52., 10.)
            })
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .truncate()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(name),
                    )
                    .child(mono(detail).text_color(palette.tx3)),
            )
    }

    pub(super) fn render_playlists(&mut self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let liked = self.resolve(&self.collections.liked);
        let mut page = div()
            .id("playlists")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .px_8()
            .pt_2()
            .pb(px(56.))
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .flex()
                    .items_end()
                    .justify_between()
                    .pb_2()
                    .child(display("Playlists", 40.))
                    .child(
                        primary("new-playlist", "Nouvelle playlist")
                            .on_click(cx.listener(|this, _, _, cx| this.start_naming(cx))),
                    ),
            );
        if self.naming.is_some() && self.adding_to.is_none() {
            page = page.child(self.naming_field());
        }
        page = page.child(
            self.collection_row(
                "liked",
                "Titres likés".to_owned(),
                count_label(liked.len()),
                None,
                true,
                cx,
            )
            .on_click(cx.listener(|this, _, _, cx| this.go(Screen::Liked, cx))),
        );
        for index in 0..self.collections.playlists.len() {
            let tracks = self.resolve(&self.collections.playlists[index].tracks);
            let name = self.collections.playlists[index].name.clone();
            page = page.child(
                self.collection_row(
                    ("playlist", index),
                    name,
                    count_label(tracks.len()),
                    tracks.first().copied(),
                    false,
                    cx,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.go(Screen::Playlist(index), cx);
                })),
            );
        }
        if self.collections.playlists.is_empty() {
            page = page.child(
                div()
                    .pt_4()
                    .text_color(pal().tx2)
                    .child("Aucune playlist pour l’instant. Crée-en une, puis ajoute des morceaux avec le + d’une ligne."),
            );
        }
        page
    }

    /// The page of the liked tracks, or of a playlist when one is given.
    pub(super) fn render_collection(
        &mut self,
        playlist: Option<usize>,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let palette = pal();
        let (title, ids) = match playlist {
            None => ("Titres likés".to_owned(), self.collections.liked.clone()),
            Some(index) => match self.collections.playlists.get(index) {
                Some(found) => (found.name.clone(), found.tracks.clone()),
                None => return self.library_notice(),
            },
        };
        let tracks = Rc::new(self.resolve(&ids));
        let missing = ids.len() - tracks.len();
        let seconds: f64 = tracks
            .iter()
            .map(|&index| self.queue.tracks[index].duration)
            .sum();
        let mut meta = format!("{} · {}", count_label(tracks.len()), format_time(seconds));
        if missing > 0 {
            meta.push_str(&format!(" · {missing} sur une autre source"));
        }
        let key = tracks
            .first()
            .map(|&index| album_key(&self.queue.tracks[index]));
        let art = key.and_then(|key| self.art(&key, cx));
        let accent = art.as_ref().map_or(palette.tide, |art| art.accent);
        let width = f32::from(window.viewport_size().width);
        let side = (width * 0.14).clamp(120., 180.);
        let deleting = self.deleting;
        let first = tracks.first().copied();
        let order = tracks.clone();
        let header = div()
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
                linear_color_stop(mix(accent, palette.ink, 0.45), 0.),
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
                    .child(mono(meta).text_color(palette.tx2))
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .pt_2()
                            .when_some(first, |row, first| {
                                row.child(primary("collection-play", "Écouter").on_click(
                                    cx.listener(move |this, _, _, cx| {
                                        this.start_in(first, order.to_vec(), cx);
                                    }),
                                ))
                            })
                            .when_some(playlist, |row, index| {
                                row.child(
                                    ghost(
                                        "playlist-delete",
                                        if deleting {
                                            "Confirmer la suppression"
                                        } else {
                                            "Supprimer"
                                        },
                                    )
                                    .on_click(cx.listener(
                                        move |this, _, _, cx| {
                                            if this.deleting {
                                                this.delete_playlist(index, cx);
                                            } else {
                                                this.deleting = true;
                                                cx.notify();
                                            }
                                        },
                                    )),
                                )
                            }),
                    ),
            );
        let body = if tracks.is_empty() {
            div()
                .py_6()
                .text_color(palette.tx2)
                .child(match playlist {
                    None => "Aucun titre liké pour l’instant : clique sur le cœur d’un morceau, ou appuie sur H.",
                    Some(_) => "Cette playlist est vide : ajoute des morceaux avec le + d’une ligne.",
                })
                .into_any_element()
        } else {
            uniform_list(
                "collection-tracks",
                tracks.len(),
                cx.processor(move |this, range: Range<usize>, window, cx| {
                    let wide = this.content_width(window) > WIDE_LIST;
                    range
                        .map(|row| {
                            let index = tracks[row];
                            let line =
                                this.track_row(index, None, Some(tracks.clone()), wide, cx);
                            match playlist {
                                Some(playlist) => line.child(
                                    icon_small(("remove", index), "close").on_click(cx.listener(
                                        move |this, _, _, cx| {
                                            cx.stop_propagation();
                                            this.remove_from_playlist(playlist, index, cx);
                                        },
                                    )),
                                ),
                                None => line,
                            }
                        })
                        .collect()
                }),
            )
            .flex_1()
            .w_full()
            .into_any_element()
        };
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(header)
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .px_8()
                    .pb(px(40.))
                    .child(body),
            )
            .into_any_element()
    }

    /// "Add to a playlist", over everything else.
    pub(super) fn render_add_menu(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let palette = pal();
        let title = self
            .adding_to
            .and_then(|index| self.queue.tracks.get(index))
            .map(|track| track.title.clone())
            .unwrap_or_default();
        let id = self
            .adding_to
            .and_then(|index| self.queue.tracks.get(index))
            .map(track_id)
            .unwrap_or_default();
        let mut list = div()
            .id("add-menu-list")
            .max_h(px(320.))
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap_1();
        for (index, playlist) in self.collections.playlists.iter().enumerate() {
            let present = playlist.tracks.contains(&id);
            list = list.child(
                div()
                    .id(("add-to", index))
                    .h(px(40.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .px_3()
                    .rounded(px(10.))
                    .cursor_pointer()
                    .hover(|row| row.bg(pal().rise2))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.add_to_playlist(index, cx);
                    }))
                    .child(div().truncate().child(playlist.name.clone()))
                    .when(present, |row| {
                        row.child(mono("déjà dedans").text_color(palette.tx3))
                    }),
            );
        }
        let naming = self.naming.is_some();
        div()
            .id("add-menu")
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(gpui::Rgba {
                a: 0.6,
                ..palette.deep
            })
            .occlude()
            .on_click(cx.listener(|this, _, _, cx| this.close_add_menu(cx)))
            .child(
                div()
                    .id("add-menu-card")
                    .w(px(420.))
                    .flex()
                    .flex_col()
                    .gap_3()
                    .p_4()
                    .rounded(px(16.))
                    .bg(palette.rise)
                    .border_1()
                    .border_color(palette.line)
                    .on_click(|_, _, cx| cx.stop_propagation())
                    .child(h2("Ajouter à une playlist"))
                    .child(
                        div()
                            .truncate()
                            .text_size(px(13.))
                            .text_color(palette.tx2)
                            .child(title),
                    )
                    .child(list)
                    .when(naming, |card| card.child(self.naming_field()))
                    .when(!naming, |card| {
                        card.child(
                            div().flex().child(
                                btn("add-new", "Nouvelle playlist").on_click(cx.listener(
                                    |this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.start_naming(cx);
                                    },
                                )),
                            ),
                        )
                    }),
            )
    }
}

fn count_label(count: usize) -> String {
    if count == 1 {
        "1 morceau".to_owned()
    } else {
        format!("{count} morceaux")
    }
}
