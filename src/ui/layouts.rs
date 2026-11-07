//! The two other ways to lay out the window: Verre (a side list) and Courant
//! (a rail of icons, with what plays filling the home).

use gpui::{Context, Div, FontWeight, Window, canvas, div, prelude::*, px};

use super::{NAVIGATION, Onde, Screen, widgets};
use crate::catalog::album_key;
use crate::theme::{Layout, alpha, look, pal, rad};
use crate::visuals::paint_wave;
use widgets::{display, h2, icon, mono, primary};

const SIDEBAR_WIDTH: f32 = 232.;
const RAIL_WIDTH: f32 = 72.;
const NEXT_CARDS: usize = 6;
const NEXT_COVER: f32 = 132.;

/// The sections of the side list, with the liked tracks set apart.
const SECTIONS: [(&str, &[usize]); 2] = [("Écouter", &[0, 1, 2]), ("Onde", &[3, 4, 5])];

fn logo(size: f32) -> Div {
    let palette = pal();
    div()
        .relative()
        .size(px(size))
        .flex_none()
        .child(icon("logo-top", size, palette.spark).absolute())
        .child(icon("logo-bottom", size, palette.tx).absolute())
}

impl Onde {
    /// Beside the pages, when the layout is not the classic one.
    pub(super) fn render_side(&mut self, cx: &mut Context<Self>) -> Option<Div> {
        match look().layout {
            Layout::Classique => None,
            Layout::Verre => Some(self.render_sidebar(cx)),
            Layout::Courant => Some(self.render_rail(cx)),
        }
    }

    /// How much of the width the side list or the rail takes.
    pub(super) fn side_width(&self, window: &Window) -> f32 {
        if self.fullscreen || window.viewport_size().width < px(super::COMPACT_WIDTH) {
            return 0.;
        }
        match look().layout {
            Layout::Classique => 0.,
            Layout::Verre => SIDEBAR_WIDTH,
            Layout::Courant => RAIL_WIDTH,
        }
    }

    fn render_sidebar(&mut self, cx: &mut Context<Self>) -> Div {
        let palette = pal();
        let mut side = div()
            .id("sidebar")
            .w(px(SIDEBAR_WIDTH))
            .flex_none()
            .h_full()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap_1()
            .px_3()
            .pt_4()
            .pb_6()
            .bg(palette.deep)
            .border_r_1()
            .border_color(palette.line)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .pb_4()
                    .child(logo(26.))
                    .child(display("onde", 22.)),
            );
        for (title, entries) in SECTIONS {
            side = side.child(
                div()
                    .px_2()
                    .pt_3()
                    .pb_1()
                    .text_size(px(11.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(palette.tx3)
                    .child(title.to_uppercase()),
            );
            for &index in entries {
                let (label, name, target) = NAVIGATION[index].clone();
                let on = self.nav_active(&target) && self.screen != Screen::Liked;
                side = side.child(self.side_item(("side", index), label, name, on, target, cx));
            }
            if title == "Écouter" {
                let on = self.screen == Screen::Liked;
                side = side.child(self.side_item(
                    ("side", 90),
                    "Titres likés",
                    "heart",
                    on,
                    Screen::Liked,
                    cx,
                ));
            }
        }
        let playlists: Vec<String> = self
            .collections
            .playlists
            .iter()
            .map(|playlist| playlist.name.clone())
            .collect();
        if !playlists.is_empty() {
            side = side.child(
                div()
                    .px_2()
                    .pt_3()
                    .pb_1()
                    .text_size(px(11.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(palette.tx3)
                    .child("PLAYLISTS"),
            );
            for (index, name) in playlists.into_iter().enumerate() {
                let on = self.screen == Screen::Playlist(index);
                side = side.child(self.side_item(
                    ("side-list", index),
                    name,
                    "listplus",
                    on,
                    Screen::Playlist(index),
                    cx,
                ));
            }
        }
        div().h_full().flex().child(side)
    }

    fn side_item(
        &mut self,
        id: (&'static str, usize),
        label: impl Into<gpui::SharedString>,
        name: &str,
        on: bool,
        target: Screen,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<Div> {
        let palette = pal();
        let tint = if on { palette.spark } else { palette.tx2 };
        div()
            .id(id)
            .h(px(36.))
            .flex_none()
            .flex()
            .items_center()
            .gap_3()
            .px_2()
            .rounded(rad(10.))
            .cursor_pointer()
            .when(on, |item| item.bg(palette.rise2))
            .when(!on, |item| item.hover(|item| item.bg(pal().rise)))
            .on_click(cx.listener(move |this, _, _, cx| this.go(target.clone(), cx)))
            .child(icon(name, 18., tint))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(if on { palette.tx } else { palette.tx2 })
                    .child(label.into()),
            )
    }

    fn render_rail(&mut self, cx: &mut Context<Self>) -> Div {
        let palette = pal();
        let mut rail = div()
            .w(px(RAIL_WIDTH))
            .flex_none()
            .h_full()
            .flex()
            .flex_col()
            .items_center()
            .gap_2()
            .pt_4()
            .bg(palette.deep)
            .child(div().pb_4().child(logo(30.)));
        for (index, (label, name, target)) in NAVIGATION.into_iter().enumerate() {
            let on = self.nav_active(&target);
            let tint = if on { palette.spark } else { palette.tx2 };
            rail = rail.child(
                div()
                    .id(("rail", index))
                    .w(px(56.))
                    .py_2()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap_1()
                    .rounded(rad(14.))
                    .cursor_pointer()
                    .when(on, |item| item.bg(alpha(palette.spark, 0.12)))
                    .when(!on, |item| item.hover(|item| item.bg(pal().rise)))
                    .on_click(cx.listener(move |this, _, _, cx| this.go(target.clone(), cx)))
                    .child(icon(name, 22., tint))
                    .child(
                        div()
                            .text_size(px(9.))
                            .line_height(px(11.))
                            .text_color(tint)
                            .child(if index == 3 { "Fichiers" } else { label }),
                    ),
            );
        }
        rail
    }

    /// The home of Courant: the track in big letters, its wave, and what follows.
    pub(super) fn render_current(&mut self, window: &Window, cx: &mut Context<Self>) -> Div {
        let palette = pal();
        let width = self.content_width(window);
        let Some(track) = self.queue.current_track().cloned() else {
            return div();
        };
        let duration = track.duration;
        let progress = if duration > 0.0 {
            (self.position / duration).clamp(0.0, 1.0) as f32
        } else {
            0.0
        };
        let size = (width * 0.09).clamp(44., 112.);
        let sound = self.analyzer.clone();
        let wave = canvas(
            |_, _, _| {},
            move |bounds, _, window, _| {
                paint_wave(bounds, &sound.borrow(), &palette, progress, window);
            },
        )
        .w_full()
        .h(px(120.));
        let paused = self.paused();
        let mut detail = track.artist.clone();
        if !track.album.trim().is_empty() {
            detail = format!("{detail} — {}", track.album);
        }

        let mut next = div().flex().gap_4().overflow_hidden();
        let count = self.queue.upcoming_count().min(NEXT_CARDS);
        for row in 0..count {
            let Some(index) = self.queue.upcoming_at(row) else {
                continue;
            };
            let upcoming = self.queue.tracks[index].clone();
            let art = self.art(&album_key(&upcoming), cx);
            next = next.child(
                div()
                    .id(("next", index))
                    .w(px(NEXT_COVER))
                    .flex_none()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| this.play(index, cx)))
                    .child(widgets::cover(art.as_ref(), NEXT_COVER, 14.))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .truncate()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(upcoming.title),
                            )
                            .child(
                                div()
                                    .truncate()
                                    .text_size(px(12.))
                                    .text_color(palette.tx2)
                                    .child(upcoming.artist),
                            ),
                    ),
            );
        }

        div()
            .flex()
            .flex_col()
            .gap_4()
            .pt_6()
            .pb_8()
            .child(mono(if paused { "EN PAUSE" } else { "EN COURS" }).text_color(palette.spark))
            .child(display(track.title.clone(), size))
            .child(
                div()
                    .text_size(px(18.))
                    .line_height(px(24.))
                    .text_color(palette.tx2)
                    .child(detail),
            )
            .when(!track.format.is_empty(), |hero| {
                hero.child(div().flex().child(widgets::tag(track.format.clone())))
            })
            .child(wave)
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(
                        primary("current-play", if paused { "Lecture" } else { "Pause" })
                            .on_click(cx.listener(|this, _, _, cx| this.toggle_play(cx))),
                    )
                    .child(
                        widgets::ghost("current-lyrics", "Paroles")
                            .on_click(cx.listener(|this, _, _, cx| this.toggle_lyrics(cx))),
                    ),
            )
            .when(count > 0, |hero| {
                hero.child(h2("Ensuite").pt_4()).child(next)
            })
    }
}
