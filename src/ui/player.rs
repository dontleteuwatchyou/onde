//! The playback bar, the queue panel and the full-screen view with lyrics.

use std::ops::Range;

use gpui::{BoxShadow, FontWeight, MouseMoveEvent, linear_color_stop, linear_gradient, point};

use super::*;
use crate::theme::alpha;
use crate::theme::rad;
use crate::visuals::{paint_volume, paint_water_line};

const VOLUME_WIDTH: f32 = 112.;

impl Onde {
    pub(super) fn paused(&self) -> bool {
        self.engine.as_ref().is_none_or(Engine::is_paused) || self.queue.current.is_none()
    }

    pub(super) fn drag_moved(
        &mut self,
        event: &MouseMoveEvent,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        if self.seek_drag.is_some() {
            let width = f32::from(window.viewport_size().width).max(1.);
            self.seek_drag = Some((f32::from(event.position.x) / width).clamp(0., 1.));
            cx.notify();
        }
        if self.volume_drag {
            self.volume_at(f32::from(event.position.x), cx);
        }
    }

    pub(super) fn drag_ended(&mut self, cx: &mut Context<Self>) {
        self.volume_drag = false;
        if let Some(fraction) = self.seek_drag.take() {
            let duration = self.queue.current_track().map_or(0.0, |t| t.duration);
            self.seek_to(f64::from(fraction) * duration, cx);
        }
    }

    fn volume_at(&mut self, x: f32, cx: &mut Context<Self>) {
        let bounds = self.volume_bounds.get();
        let start = f32::from(bounds.origin.x);
        let width = f32::from(bounds.size.width).max(1.);
        if let Some(engine) = &self.engine {
            engine.set_volume(((x - start) / width).clamp(0., 1.));
        }
        self.muted_volume = None;
        cx.notify();
    }

    fn toggle_mute(&mut self, cx: &mut Context<Self>) {
        let Some(engine) = &self.engine else { return };
        match self.muted_volume.take() {
            Some(volume) => engine.set_volume(volume),
            None => {
                self.muted_volume = Some(engine.volume());
                engine.set_volume(0.0);
            }
        }
        cx.notify();
    }

    pub(super) fn render_bar(
        &mut self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let palette = pal();
        let track = self.queue.current_track().cloned();
        let art = self.current_art(cx);
        let duration = track.as_ref().map_or(0.0, |t| t.duration);
        let played = if duration > 0.0 {
            (self.position / duration).clamp(0.0, 1.0) as f32
        } else {
            0.0
        };
        let progress = self.seek_drag.unwrap_or(played);
        let shown_time = if self.seek_drag.is_some() {
            f64::from(progress) * duration
        } else {
            self.position
        };
        let paused = self.paused();
        let volume = self.engine.as_ref().map_or(0.0, Engine::volume);
        let compact = window.viewport_size().width < px(COMPACT_WIDTH);
        let bass = self.analyzer.borrow().punch();

        let sound = self.analyzer.clone();
        let water = self.water_line.clone();
        let line = canvas(
            |_, _, _| {},
            move |bounds, _, window, _| {
                paint_water_line(
                    bounds,
                    &sound.borrow(),
                    &palette,
                    progress,
                    &mut water.borrow_mut(),
                    window,
                );
            },
        )
        .absolute()
        .top(px(-34.))
        .left_0()
        .w_full()
        .h(px(46.));

        let sound = self.analyzer.clone();
        let spot = self.volume_bounds.clone();
        let volume_bars = canvas(
            move |bounds, _, _| spot.set(bounds),
            move |bounds, _, window, _| {
                paint_volume(bounds, &sound.borrow(), &palette, volume, window);
            },
        )
        .size_full();

        let artist_name = track.as_ref().map(|t| t.artist.clone());
        let subtitle = track.as_ref().map_or(String::new(), |t| {
            if t.format.is_empty() {
                t.artist.clone()
            } else {
                format!("{} · {}", t.artist, t.format)
            }
        });
        let now = div()
            .flex_1()
            .min_w_0()
            .flex()
            .items_center()
            .gap_3()
            .when(track.is_some(), |now| {
                now.child(
                    div()
                        .id("bar-cover")
                        .cursor_pointer()
                        .on_click(cx.listener(|this, _, _, cx| this.toggle_fullscreen(cx)))
                        .child(widgets::cover(art.as_ref(), 48., 6.)),
                )
            })
            .child(
                div()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        div().truncate().font_weight(FontWeight::SEMIBOLD).child(
                            track
                                .as_ref()
                                .map_or("Aucune lecture".to_owned(), |t| t.title.clone()),
                        ),
                    )
                    .child(
                        div()
                            .id("bar-artist")
                            .truncate()
                            .text_size(px(12.))
                            .line_height(px(16.))
                            .text_color(palette.tx2)
                            .cursor_pointer()
                            .child(subtitle)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if let Some(name) = artist_name.clone() {
                                    this.go(Screen::Artist(name), cx);
                                }
                            })),
                    ),
            )
            .when_some(self.queue.current, |now, current| {
                let liked = self.is_liked(current);
                now.child(
                    icon_button(
                        "bar-like",
                        if liked { "heart-fill" } else { "heart" },
                        liked,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.toggle_like(current, cx);
                    })),
                )
                .when(!compact, |now| {
                    now.child(icon_button("bar-add", "listplus", false).on_click(
                        cx.listener(move |this, _, _, cx| this.open_add_menu(current, cx)),
                    ))
                })
            });

        let play = div()
            .id("play")
            .size(px(44.))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded_full()
            .bg(palette.spark)
            .cursor_pointer()
            .shadow(vec![BoxShadow {
                color: alpha(palette.spark, 0.35 + 0.6 * bass).into(),
                offset: point(px(0.), px(0.)),
                blur_radius: px(10. + 26. * bass),
                spread_radius: px(0.),
            }])
            .hover(|button| button.opacity(0.9))
            .on_click(cx.listener(|this, _, _, cx| this.toggle_play(cx)))
            .child(icon(
                if paused { "play" } else { "pause" },
                22.,
                palette.ink,
            ));

        let transport = div()
            .flex_none()
            .flex()
            .items_center()
            .gap_2()
            .child(
                icon_button("shuffle", "shuffle", self.queue.shuffle)
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_shuffle(cx))),
            )
            .child(
                icon_button("previous", "prev", false)
                    .on_click(cx.listener(|this, _, _, cx| this.previous(cx))),
            )
            .child(play)
            .child(
                icon_button("next", "next", false)
                    .on_click(cx.listener(|this, _, _, cx| this.next(cx))),
            )
            .when(!compact, |row| {
                row.child(
                    mono(format!(
                        "{} / {}",
                        format_time(shown_time),
                        format_time(duration)
                    ))
                    .pl_2()
                    .text_color(palette.tx2),
                )
            });

        let tools = div()
            .flex_1()
            .min_w_0()
            .flex()
            .items_center()
            .justify_end()
            .gap_1()
            .when(!compact, |row| {
                row.child(
                    icon_button(
                        "mute",
                        if self.muted_volume.is_some() {
                            "volx"
                        } else {
                            "vol"
                        },
                        false,
                    )
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_mute(cx))),
                )
                .child(
                    div()
                        .id("volume")
                        .w(px(VOLUME_WIDTH))
                        .h(px(28.))
                        .flex_none()
                        .cursor_pointer()
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, event: &MouseDownEvent, _, cx| {
                                this.volume_drag = true;
                                this.volume_at(f32::from(event.position.x), cx);
                            }),
                        )
                        .child(volume_bars),
                )
                .child(
                    mono(format!("{:.0}", volume * 100.))
                        .w(px(28.))
                        .text_color(palette.tx3),
                )
            })
            .child(
                icon_button("lyrics", "lyr", self.fullscreen && self.show_lyrics)
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_lyrics(cx))),
            )
            .child(
                icon_button("queue", "queue", self.show_queue)
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_queue(cx))),
            )
            .child(
                icon_button("visuals", "wave", self.fullscreen && !self.show_lyrics)
                    .on_click(cx.listener(|this, _, _, cx| this.visual_key(cx))),
            );

        let seek = div()
            .id("seek")
            .absolute()
            .top(px(-14.))
            .left_0()
            .w_full()
            .h(px(24.))
            .cursor_pointer()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    if this.queue.current.is_some() && duration > 0.0 {
                        let width = f32::from(window.viewport_size().width).max(1.);
                        this.seek_drag = Some((f32::from(event.position.x) / width).clamp(0., 1.));
                        cx.notify();
                    }
                }),
            );

        div()
            .relative()
            .flex_none()
            .h(px(76.))
            .bg(palette.deep)
            .child(line)
            .child(
                div()
                    .size_full()
                    .flex()
                    .items_center()
                    .gap_4()
                    .px_6()
                    .pt_2()
                    .child(now)
                    .child(transport)
                    .child(tools),
            )
            .child(seek)
    }

    pub(super) fn render_queue(&mut self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let palette = pal();
        let count = self.queue.upcoming_count();
        let current = self.queue.current_track().cloned();
        let art = self.current_art(cx);
        div()
            .w(px(388.))
            .flex_none()
            .h_full()
            .flex()
            .flex_col()
            .bg(palette.deep)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .p_4()
                    .child(h2("File d’attente").flex_1())
                    .child(mono(format!("{count} à suivre")).text_color(palette.tx2))
                    .child(
                        icon_small("queue-close", "close")
                            .on_click(cx.listener(|this, _, _, cx| this.toggle_queue(cx))),
                    ),
            )
            .when_some(current, |panel, track| {
                panel.child(
                    div()
                        .flex()
                        .items_center()
                        .gap_3()
                        .mx_4()
                        .mb_2()
                        .p_2()
                        .rounded(rad(10.))
                        .bg(palette.rise)
                        .child(widgets::cover(art.as_ref(), 40., 6.))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .child(
                                    div()
                                        .truncate()
                                        .text_color(palette.spark)
                                        .font_weight(FontWeight::MEDIUM)
                                        .child(track.title),
                                )
                                .child(
                                    div()
                                        .truncate()
                                        .text_size(px(12.))
                                        .text_color(palette.tx2)
                                        .child(track.artist),
                                ),
                        ),
                )
            })
            .child(if count == 0 {
                div()
                    .px_4()
                    .py_6()
                    .text_color(palette.tx2)
                    .child("Rien à suivre.")
                    .into_any_element()
            } else {
                uniform_list(
                    "queue-list",
                    count,
                    cx.processor(|this, range: Range<usize>, _, cx| {
                        let palette = pal();
                        let rows: Vec<usize> = range
                            .filter_map(|row| this.queue.upcoming_at(row))
                            .collect();
                        rows.into_iter()
                            .map(|index| {
                                let track = this.queue.tracks[index].clone();
                                let art = this.art(&album_key(&track), cx);
                                div()
                                    .id(("queue-row", index))
                                    .w_full()
                                    .h(px(52.))
                                    .flex()
                                    .items_center()
                                    .gap_3()
                                    .px_4()
                                    .cursor_pointer()
                                    .hover(|row| row.bg(pal().rise))
                                    .on_click(
                                        cx.listener(move |this, _, _, cx| this.play(index, cx)),
                                    )
                                    .child(widgets::cover(art.as_ref(), 36., 6.))
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .flex()
                                            .flex_col()
                                            .child(
                                                div()
                                                    .truncate()
                                                    .font_weight(FontWeight::MEDIUM)
                                                    .child(track.title),
                                            )
                                            .child(
                                                div()
                                                    .truncate()
                                                    .text_size(px(12.))
                                                    .text_color(palette.tx2)
                                                    .child(track.artist),
                                            ),
                                    )
                                    .child(
                                        mono(format_time(track.duration)).text_color(palette.tx3),
                                    )
                            })
                            .collect()
                    }),
                )
                .flex_1()
                .w_full()
                .into_any_element()
            })
    }

    pub(super) fn render_fullscreen(
        &mut self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let palette = pal();
        let width = f32::from(window.viewport_size().width);
        let height = f32::from(window.viewport_size().height);
        let art = self.current_art(cx);
        let lyrics_mode = self.show_lyrics;
        let (visual, sound, state) = (
            self.visual,
            self.analyzer.clone(),
            self.visual_state.clone(),
        );
        let lit = self.show_visuals;
        let grid_art = art.clone();
        let stage = canvas(
            |_, _, _| {},
            move |bounds, _, window, _| {
                if lit {
                    visuals::paint(
                        visual,
                        bounds,
                        &sound.borrow(),
                        &palette,
                        &mut state.borrow_mut(),
                        grid_art.as_ref().map(|art| &art.grid),
                        lyrics_mode,
                        window,
                    );
                }
            },
        )
        .absolute()
        .size_full();

        let mut pills = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_2()
            .p_4()
            .child(
                btn("fs-close", "Réduire · Échap")
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_fullscreen(cx))),
            )
            .child(div().flex_1());
        for (index, choice) in VISUALS.into_iter().enumerate() {
            pills = pills.child(
                chip(("fs-visual", index), choice.name(), choice == self.visual)
                    .on_click(cx.listener(move |this, _, _, cx| this.set_visual(choice, cx))),
            );
        }

        let track = self.queue.current_track().cloned();
        let content = if lyrics_mode {
            self.render_lyrics(width, cx)
        } else {
            let (title, detail, format) = track.map_or(
                ("Aucune lecture".to_owned(), String::new(), String::new()),
                |t| {
                    let detail = if t.album.trim().is_empty() {
                        t.artist.clone()
                    } else {
                        format!("{} — {}", t.artist, t.album)
                    };
                    (t.title, detail, t.format)
                },
            );
            div()
                .absolute()
                .bottom_0()
                .left_0()
                .w_full()
                .h(relative(0.62))
                .flex()
                .items_end()
                .gap_6()
                .px_8()
                .pb(px(56.))
                .bg(linear_gradient(
                    180.,
                    linear_color_stop(alpha(palette.ink, 0.), 0.),
                    linear_color_stop(palette.ink, 1.),
                ))
                .child(widgets::cover(
                    art.as_ref(),
                    (height * 0.26).clamp(160., 300.),
                    16.,
                ))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .when(!format.is_empty(), |text| {
                            text.child(div().flex().child(tag(format)))
                        })
                        .child(
                            display(title, (width * 0.08).clamp(44., 128.))
                                .line_clamp(2)
                                .overflow_hidden(),
                        )
                        .child(
                            div()
                                .truncate()
                                .text_size(px(18.))
                                .line_height(px(24.))
                                .child(detail),
                        ),
                )
                .into_any_element()
        };
        div()
            .relative()
            .flex_1()
            .min_h_0()
            .bg(palette.deep)
            .child(stage)
            .child(content)
            .child(div().absolute().top_0().left_0().w_full().child(pills))
    }

    fn render_lyrics(&mut self, width: f32, cx: &mut Context<Self>) -> AnyElement {
        let palette = pal();
        let size = (width * 0.034).clamp(26., 46.);
        let frame = || {
            div()
                .absolute()
                .top(px(72.))
                .bottom(px(44.))
                .left(px(48.))
                .w(relative(0.62))
                .flex()
                .flex_col()
                .gap_4()
                .font_family("Bricolage Grotesque")
                .font_weight(FontWeight::BOLD)
                .text_size(px(size))
                .line_height(px(size * 1.15))
        };
        let notice = |text: String| frame().justify_center().child(text).into_any_element();
        match &self.lyrics {
            LyricsState::Idle => notice("Lance un morceau pour voir ses paroles.".into()),
            LyricsState::Loading => notice("Recherche des paroles…".into()),
            LyricsState::Missing => notice("Pas de paroles pour ce morceau.".into()),
            LyricsState::Failed(error) => frame()
                .justify_center()
                .child("Paroles indisponibles.")
                .child(
                    div()
                        .font_family("Instrument Sans")
                        .text_size(px(14.))
                        .line_height(px(20.))
                        .text_color(palette.tx2)
                        .child(error.clone()),
                )
                .into_any_element(),
            LyricsState::Ready(Lyrics::Instrumental, _) => notice("Morceau instrumental.".into()),
            LyricsState::Ready(Lyrics::Plain(lines), origin) => frame()
                .id("plain-lyrics")
                .overflow_y_scroll()
                .children(lines.iter().map(|line| div().child(line.clone())))
                .child(origin_label(*origin))
                .into_any_element(),
            LyricsState::Ready(Lyrics::Synced(lines), origin) => {
                let ms = (self.position * 1000.0) as u64;
                let active = active_line(lines, ms);
                let center = active.unwrap_or(0);
                let first = center.saturating_sub(LINES_BEFORE);
                let last = (center + LINES_AFTER).min(lines.len().saturating_sub(1));
                frame()
                    .overflow_hidden()
                    .children((first..=last).map(|index| {
                        let line = &lines[index];
                        let start = line.start_ms as f64 / 1000.0;
                        div()
                            .id(index)
                            .cursor_pointer()
                            .text_color(alpha(palette.tx, line_alpha(line, index, active)))
                            .on_click(cx.listener(move |this, _, _, cx| this.seek_to(start, cx)))
                            .child(line_text(line, ms, active == Some(index)))
                    }))
                    .child(origin_label(*origin))
                    .into_any_element()
            }
        }
    }
}

fn line_alpha(line: &Line, index: usize, active: Option<usize>) -> f32 {
    match active {
        Some(current) if index < current => SUNG_ALPHA,
        // With word timing the words light up on top of this base.
        Some(current) if index == current && line.words.is_empty() => 1.0,
        _ => UPCOMING_ALPHA,
    }
}

/// The active line lights up word by word when the lyrics have word timing.
fn line_text(line: &Line, ms: u64, is_active: bool) -> StyledText {
    if line.text.trim().is_empty() {
        return StyledText::new("…");
    }
    let text = StyledText::new(line.text.clone());
    if !is_active || line.words.is_empty() {
        return text;
    }
    // GPUI blends a highlight over the text color, so the alpha is just the fill.
    let color = pal().tx;
    text.with_highlights(line_fills(line, ms).into_iter().map(move |(range, fill)| {
        let style = HighlightStyle {
            color: Some(alpha(color, fill).into()),
            ..Default::default()
        };
        (range, style)
    }))
}

fn origin_label(origin: Origin) -> impl IntoElement + use<> {
    mono(format!("Source : {}", origin.label())).text_color(pal().tx3)
}
