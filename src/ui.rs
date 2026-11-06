//! The window: track list and playback bar.

mod downloads;
mod quality;
mod setup;

use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{
    AnyElement, App, Context, FocusHandle, HighlightStyle, Hsla, Image, ImageFormat, KeyBinding,
    MouseButton, MouseDownEvent, ObjectFit, PathPromptOptions, SharedString, StyledText, Window,
    actions, canvas, div, img, prelude::*, px, relative, rgb, uniform_list,
};

use crate::analysis::{Analyzer, BANDS};
use crate::config;
use crate::engine::Engine;
use crate::library::{Location, format_time};
use crate::lyrics::source::{self, Origin};
use crate::lyrics::{Line, Lyrics, active_line, line_fills};
use crate::queue::{Previous, Queue};
use crate::sources::MusicSource;
use crate::sources::local::LocalFolder;
use crate::sources::subsonic::Subsonic;
use crate::visuals;

const BACKGROUND: u32 = 0x12141a;
const SURFACE: u32 = 0x1b1e26;
const HOVER: u32 = 0x262a34;
const SELECTED: u32 = 0x1f3a3d;
const TEXT: u32 = 0xeceef2;
const MUTED: u32 = 0x8b90a0;
const ACCENT: u32 = 0x3fd0c0;
const ERROR: u32 = 0xff8a80;

/// Below this width the bar keeps only the track name and the buttons.
const COMPACT_WIDTH: f32 = 760.0;

const VOLUME_STEP: f32 = 0.1;
const SEEK_STEP: f64 = 5.0;

const COVER_SIZE: f32 = 44.;
const FORMAT_WIDTH: f32 = 150.;
const AUDIO_GAP: Duration = Duration::from_millis(400);
const SLOW_TICK: Duration = Duration::from_millis(200);
/// Fast enough for the visuals and the word highlight to look continuous.
const FAST_TICK: Duration = Duration::from_millis(33);
const LINES_BEFORE: usize = 2;
const LINES_AFTER: usize = 6;
const SUNG_ALPHA: f32 = 0.3;
const UPCOMING_ALPHA: f32 = 0.45;

actions!(
    onde,
    [
        TogglePlay,
        NextTrack,
        PreviousTrack,
        SeekForward,
        SeekBackward,
        ToggleLyrics,
        ToggleVisuals,
        VolumeUp,
        VolumeDown,
        OpenNavidrome,
        ToggleDownloads,
        SubmitSearch,
        EraseSearch,
        PasteSearch
    ]
);

enum LyricsState {
    Idle,
    Loading,
    Ready(Lyrics, Origin),
    Missing,
    Failed(String),
}

pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("space", TogglePlay, Some("Lecteur")),
        KeyBinding::new("ctrl-right", NextTrack, Some("Lecteur")),
        KeyBinding::new("ctrl-left", PreviousTrack, Some("Lecteur")),
        KeyBinding::new("right", SeekForward, Some("Lecteur")),
        KeyBinding::new("left", SeekBackward, Some("Lecteur")),
        KeyBinding::new("l", ToggleLyrics, Some("Lecteur")),
        KeyBinding::new("n", OpenNavidrome, Some("Lecteur")),
        KeyBinding::new("v", ToggleVisuals, Some("Lecteur")),
        KeyBinding::new("up", VolumeUp, Some("Lecteur")),
        KeyBinding::new("down", VolumeDown, Some("Lecteur")),
        KeyBinding::new("d", ToggleDownloads, Some("Lecteur")),
        // While the download screen is open, typing goes to its search field.
        KeyBinding::new("escape", ToggleDownloads, Some("Recherche")),
        KeyBinding::new("enter", SubmitSearch, Some("Recherche")),
        KeyBinding::new("backspace", EraseSearch, Some("Recherche")),
        KeyBinding::new("ctrl-v", PasteSearch, Some("Recherche")),
    ]);
}

pub enum Startup {
    Empty,
    Folder(std::path::PathBuf),
    Navidrome,
}

pub struct Onde {
    quality_open: bool,
    quality_busy: bool,
    quality_title: String,
    quality_report: Option<crate::quality::Report>,
    quality_error: Option<String>,
    queue: Queue,
    source: Option<Arc<dyn MusicSource>>,
    engine: Option<Engine>,
    message: Option<SharedString>,
    position: f64,
    scanning: bool,
    lyrics: LyricsState,
    /// Bumped on each track change, so a slow lookup can't show the wrong lyrics.
    lyrics_request: u64,
    show_lyrics: bool,
    analyzer: Option<Analyzer>,
    show_visuals: bool,
    last_tick: Instant,
    last_audio: Instant,
    downloads: downloads::Downloads,
    /// First launch, before a music folder was chosen.
    ask_folder: bool,
    cover: Option<Arc<Image>>,
    /// Which track `cover` belongs to, or is being fetched for.
    cover_for: Option<Location>,
    focus: FocusHandle,
}

impl Onde {
    pub fn new(startup: Startup, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        window.focus(&focus);
        let (engine, message) = match Engine::new() {
            Ok(engine) => (Some(engine), None),
            Err(error) => (None, Some(format!("Audio indisponible : {error:#}").into())),
        };
        cx.spawn(async move |this, cx| {
            let mut interval = SLOW_TICK;
            loop {
                cx.background_executor().timer(interval).await;
                match this.update(cx, |this, cx| this.tick(cx)) {
                    Ok(next) => interval = next,
                    Err(_) => return,
                }
            }
        })
        .detach();
        let analyzer = engine.as_ref().map(|e| Analyzer::new(e.sample_rate()));
        let mut this = Self {
            quality_open: false,
            quality_busy: false,
            quality_title: String::new(),
            quality_report: None,
            quality_error: None,
            queue: Queue::default(),
            source: None,
            engine,
            message,
            position: 0.0,
            scanning: false,
            lyrics: LyricsState::Idle,
            lyrics_request: 0,
            show_lyrics: false,
            analyzer,
            show_visuals: true,
            last_tick: Instant::now(),
            last_audio: Instant::now(),
            downloads: downloads::Downloads::new(),
            ask_folder: false,
            cover: None,
            cover_for: None,
            focus,
        };
        match startup {
            Startup::Empty => match config::load().music_folder {
                Some(folder) if folder.is_dir() => {
                    this.open_source(Arc::new(LocalFolder(folder)), cx);
                }
                _ => this.ask_folder = true,
            },
            Startup::Folder(folder) => this.open_source(Arc::new(LocalFolder(folder)), cx),
            Startup::Navidrome => this.open_navidrome(cx),
        }
        this
    }

    fn tick(&mut self, cx: &mut Context<Self>) -> Duration {
        self.poll_telegram(cx);
        if self.downloads.open && self.downloads.needs_updates() {
            cx.notify();
        }
        let Some(engine) = &self.engine else {
            return SLOW_TICK;
        };
        if let Some(error) = engine.take_error() {
            self.message = Some(format!("Lecture impossible : {error}").into());
            cx.notify();
        }
        let finished = engine.take_finished();
        let position = engine.position();
        if finished {
            match self.queue.next_index() {
                Some(next) => self.play(next, cx),
                None => self.stop(cx),
            }
        } else if self.queue.current.is_some() && position != self.position {
            self.position = position;
            cx.notify();
        }
        let visuals_moving = self.update_visuals(cx);
        let lyrics_moving = self.show_lyrics && matches!(self.lyrics, LyricsState::Ready(..));
        let playing =
            self.queue.current.is_some() && self.engine.as_ref().is_some_and(|e| !e.is_paused());
        if visuals_moving || (lyrics_moving && playing) {
            FAST_TICK
        } else {
            SLOW_TICK
        }
    }

    /// Returns whether the visuals need another frame soon.
    fn update_visuals(&mut self, cx: &mut Context<Self>) -> bool {
        let dt = self.last_tick.elapsed().as_secs_f32();
        self.last_tick = Instant::now();
        let (Some(engine), Some(analyzer)) = (&self.engine, &mut self.analyzer) else {
            return false;
        };
        let was_quiet = analyzer.is_quiet();
        let mut received = false;
        engine.drain_tap(|sample| {
            received = true;
            analyzer.push(sample);
        });
        if received {
            self.last_audio = Instant::now();
        }
        // The sound card can take a while between two buffers: only a longer
        // gap means the audio really stopped.
        let hearing = self.queue.current.is_some()
            && !engine.is_paused()
            && self.last_audio.elapsed() < AUDIO_GAP;
        if self.show_visuals && hearing {
            analyzer.update(dt);
        } else {
            analyzer.release(dt);
        }
        let moving = !was_quiet || !analyzer.is_quiet();
        if self.show_visuals && moving {
            cx.notify();
        }
        self.show_visuals && (moving || hearing)
    }

    fn load_cover(&mut self, cx: &mut Context<Self>) {
        let (track, music) = (self.queue.current_track().cloned(), self.source.clone());
        let wanted = track.as_ref().map(|track| track.location.clone());
        if wanted == self.cover_for {
            return;
        }
        self.cover = None;
        self.cover_for = wanted.clone();
        let (Some(track), Some(music)) = (track, music) else {
            return;
        };
        cx.spawn(async move |this, cx| {
            let bytes = cx
                .background_spawn(async move { music.cover(&track) })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.cover_for != wanted {
                    return;
                }
                this.cover = bytes.ok().flatten().and_then(|bytes| {
                    let format = image_format(&bytes)?;
                    Some(Arc::new(Image::from_bytes(format, bytes)))
                });
                cx.notify();
            });
        })
        .detach();
    }

    /// Lists the source again without touching what is playing.
    fn refresh_source(&mut self, cx: &mut Context<Self>) {
        let Some(music) = self.source.clone() else {
            return;
        };
        cx.spawn(async move |this, cx| {
            let listed = cx
                .background_spawn(async move {
                    // Not fatal: the server still finds new files on its own schedule.
                    let _ = music.rescan();
                    music.list_tracks()
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                if let Ok(tracks) = listed {
                    this.queue.refresh(tracks);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn load_lyrics(&mut self, cx: &mut Context<Self>) {
        self.lyrics_request += 1;
        let request = self.lyrics_request;
        let (Some(track), Some(music)) = (self.queue.current_track().cloned(), self.source.clone())
        else {
            self.lyrics = LyricsState::Idle;
            return;
        };
        self.lyrics = LyricsState::Loading;
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move { source::load(&track, music.as_ref()) })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.lyrics_request != request {
                    return;
                }
                this.lyrics = match result {
                    Ok(Some((lyrics, origin))) => LyricsState::Ready(lyrics, origin),
                    Ok(None) => LyricsState::Missing,
                    Err(error) => LyricsState::Failed(format!("{error:#}")),
                };
                cx.notify();
            });
        })
        .detach();
    }

    fn open_source(&mut self, music: Arc<dyn MusicSource>, cx: &mut Context<Self>) {
        self.scanning = true;
        self.message = None;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let lister = music.clone();
            let listed = cx
                .background_spawn(async move { lister.list_tracks() })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.scanning = false;
                match listed {
                    Ok(tracks) => {
                        if let Some(engine) = &this.engine {
                            engine.stop();
                        }
                        this.source = Some(music);
                        this.queue.replace(tracks);
                        this.position = 0.0;
                        this.load_lyrics(cx);
                        this.load_cover(cx);
                    }
                    // What was open stays open and playing.
                    Err(error) => {
                        this.message = Some(format!("Ouverture impossible : {error:#}").into());
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn open_navidrome(&mut self, cx: &mut Context<Self>) {
        match config::load().navidrome {
            Some(credentials) => self.open_source(Arc::new(Subsonic::new(credentials)), cx),
            None => {
                self.message = Some(
                    "Navidrome n'est pas configuré. Dans un terminal : onde navidrome-login ADRESSE UTILISATEUR"
                        .into(),
                );
                cx.notify();
            }
        }
    }

    fn pick_folder(&mut self, cx: &mut Context<Self>) {
        let answer = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Ouvrir".into()),
        });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = answer.await else {
                return;
            };
            let Some(folder) = paths.into_iter().next() else {
                return;
            };
            let _ = this.update(cx, |this, cx| {
                this.open_source(Arc::new(LocalFolder(folder)), cx)
            });
        })
        .detach();
    }

    fn play(&mut self, index: usize, cx: &mut Context<Self>) {
        let (Some(engine), Some(music), Some(track)) =
            (&self.engine, &self.source, self.queue.tracks.get(index))
        else {
            return;
        };
        match music.audio(track) {
            Ok(audio) => engine.load(audio),
            Err(error) => {
                self.message = Some(format!("Lecture impossible : {error:#}").into());
                cx.notify();
                return;
            }
        }
        self.queue.current = Some(index);
        self.position = 0.0;
        self.message = None;
        self.load_lyrics(cx);
        self.load_cover(cx);
        cx.notify();
    }

    fn stop(&mut self, cx: &mut Context<Self>) {
        if let Some(engine) = &self.engine {
            engine.stop();
        }
        self.queue.current = None;
        self.position = 0.0;
        self.load_lyrics(cx);
        self.load_cover(cx);
        cx.notify();
    }

    fn toggle_play(&mut self, cx: &mut Context<Self>) {
        let Some(engine) = &self.engine else { return };
        if self.queue.current.is_none() {
            if let Some(first) = self.queue.next_index() {
                self.play(first, cx);
            }
            return;
        }
        engine.set_paused(!engine.is_paused());
        cx.notify();
    }

    fn next(&mut self, cx: &mut Context<Self>) {
        if let Some(next) = self.queue.next_index() {
            self.play(next, cx);
        }
    }

    fn previous(&mut self, cx: &mut Context<Self>) {
        if self.queue.current.is_none() {
            return;
        }
        match self.queue.previous(self.position) {
            Previous::Track(index) => self.play(index, cx),
            Previous::Restart => self.seek_to(0.0, cx),
        }
    }

    fn seek_to(&mut self, seconds: f64, cx: &mut Context<Self>) {
        let Some(track) = self.queue.current_track() else {
            return;
        };
        let limit = if track.duration > 0.0 {
            track.duration
        } else {
            f64::MAX
        };
        let target = seconds.clamp(0.0, limit);
        if let Some(engine) = &self.engine {
            engine.seek(target);
        }
        self.position = target;
        cx.notify();
    }

    fn toggle_visuals(&mut self, cx: &mut Context<Self>) {
        self.show_visuals = !self.show_visuals;
        cx.notify();
    }

    fn toggle_lyrics(&mut self, cx: &mut Context<Self>) {
        self.show_lyrics = !self.show_lyrics;
        cx.notify();
    }

    fn change_volume(&mut self, delta: f32, cx: &mut Context<Self>) {
        if let Some(engine) = &self.engine {
            engine.set_volume(engine.volume() + delta);
        }
        cx.notify();
    }

    fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .items_center()
            .justify_between()
            .px_5()
            .py_3()
            .child(div().text_xl().text_color(rgb(ACCENT)).child("Onde"))
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(
                        button("visuals", "Visuels")
                            .on_click(cx.listener(|this, _, _, cx| this.toggle_visuals(cx))),
                    )
                    .child(
                        button("downloads", "Télécharger")
                            .on_click(cx.listener(|this, _, _, cx| this.toggle_downloads(cx))),
                    )
                    .child(
                        button("navidrome", "Navidrome")
                            .on_click(cx.listener(|this, _, _, cx| this.open_navidrome(cx))),
                    )
                    .child(
                        button("open", "Ouvrir un dossier")
                            .on_click(cx.listener(|this, _, _, cx| this.pick_folder(cx))),
                    ),
            )
    }

    fn render_list(&self, cx: &mut Context<Self>) -> impl IntoElement {
        if self.queue.tracks.is_empty() {
            let text = if self.scanning {
                "Lecture du dossier…"
            } else {
                "Ouvre un dossier de musique ou ton serveur Navidrome pour commencer."
            };
            return div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .text_color(rgb(MUTED))
                .child(text)
                .into_any_element();
        }
        uniform_list(
            "tracks",
            self.queue.tracks.len(),
            cx.processor(|this, range: std::ops::Range<usize>, window, cx| {
                let compact = window.viewport_size().width < px(COMPACT_WIDTH);
                range
                    .map(|index| {
                        let track = &this.queue.tracks[index];
                        let is_current = this.queue.current == Some(index);
                        div()
                            .id(index)
                            .w_full()
                            .flex()
                            .items_center()
                            .gap_3()
                            .h(px(44.))
                            .px_5()
                            .cursor_pointer()
                            .when(is_current, |row| row.bg(rgb(SELECTED)))
                            .when(!is_current, |row| row.hover(|row| row.bg(rgb(HOVER))))
                            .on_click(cx.listener(move |this, _, _, cx| this.play(index, cx)))
                            .child(
                                div()
                                    .id(("verify", index))
                                    .px_2()
                                    .text_sm()
                                    .child("Vérifier")
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.inspect_track(index, cx);
                                    })),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_color(rgb(if is_current { ACCENT } else { TEXT }))
                                    .child(track.title.clone()),
                            )
                            .child(
                                div()
                                    .w(relative(0.32))
                                    .truncate()
                                    .text_sm()
                                    .text_color(rgb(MUTED))
                                    .child(track.artist.clone()),
                            )
                            .when(!compact, |row| {
                                row.child(
                                    div()
                                        .w(px(FORMAT_WIDTH))
                                        .truncate()
                                        .text_sm()
                                        .text_color(rgb(MUTED))
                                        .child(track.format.clone()),
                                )
                            })
                            .child(
                                div()
                                    .w(px(56.))
                                    .flex()
                                    .justify_end()
                                    .text_sm()
                                    .text_color(rgb(MUTED))
                                    .child(format_time(track.duration)),
                            )
                    })
                    .collect()
            }),
        )
        .flex_1()
        .w_full()
        .into_any_element()
    }

    fn render_visuals(&self) -> impl IntoElement {
        let (levels, bass) = self
            .analyzer
            .as_ref()
            .map_or(([0.0; BANDS], 0.0), |a| (*a.levels(), a.bass()));
        canvas(
            |_, _, _| {},
            move |bounds, _, window, _| visuals::paint(bounds, &levels, bass, window),
        )
        .absolute()
        .size_full()
    }

    fn render_lyrics(&self, cx: &mut Context<Self>) -> AnyElement {
        let notice = |text: String| {
            div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .px_5()
                .text_color(rgb(MUTED))
                .child(text)
                .into_any_element()
        };
        match &self.lyrics {
            LyricsState::Idle => notice("Lance un morceau pour voir ses paroles.".into()),
            LyricsState::Loading => notice("Recherche des paroles…".into()),
            LyricsState::Missing => notice("Aucune parole trouvée pour ce morceau.".into()),
            LyricsState::Failed(error) => notice(format!("Paroles indisponibles : {error}")),
            LyricsState::Ready(Lyrics::Instrumental, _) => notice("Morceau instrumental.".into()),
            LyricsState::Ready(Lyrics::Plain(lines), origin) => div()
                .id("plain-lyrics")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .px_8()
                .py_4()
                .flex()
                .flex_col()
                .gap_1()
                .text_lg()
                .children(lines.iter().map(|line| div().child(line.clone())))
                .child(origin_label(*origin))
                .into_any_element(),
            LyricsState::Ready(Lyrics::Synced(lines), origin) => {
                self.render_synced(lines, *origin, cx)
            }
        }
    }

    fn render_synced(&self, lines: &[Line], origin: Origin, cx: &mut Context<Self>) -> AnyElement {
        let ms = (self.position * 1000.0) as u64;
        let active = active_line(lines, ms);
        let center = active.unwrap_or(0);
        let first = center.saturating_sub(LINES_BEFORE);
        let last = (center + LINES_AFTER).min(lines.len() - 1);
        div()
            .flex_1()
            .min_h_0()
            .overflow_hidden()
            .flex()
            .flex_col()
            .gap_4()
            .px_8()
            .py_6()
            .text_3xl()
            .children((first..=last).map(|index| {
                let line = &lines[index];
                let start = line.start_ms as f64 / 1000.0;
                div()
                    .id(index)
                    .cursor_pointer()
                    .text_color(text_alpha(line_alpha(line, index, active)))
                    .on_click(cx.listener(move |this, _, _, cx| this.seek_to(start, cx)))
                    .child(line_text(line, ms, active == Some(index)))
            }))
            .child(origin_label(origin))
            .into_any_element()
    }

    fn render_bar(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let track = self.queue.current_track();
        let duration = track.map_or(0.0, |t| t.duration);
        let progress = if duration > 0.0 {
            (self.position / duration).clamp(0.0, 1.0) as f32
        } else {
            0.0
        };
        let paused =
            self.engine.as_ref().is_none_or(Engine::is_paused) || self.queue.current.is_none();
        let volume = self.engine.as_ref().map_or(0.0, Engine::volume);
        let width = window.viewport_size().width;
        let compact = width < px(COMPACT_WIDTH);

        div()
            .flex()
            .flex_col()
            .bg(rgb(SURFACE))
            .child(
                // The bar spans the window, so the click's x gives the fraction directly.
                div()
                    .id("progress")
                    .w_full()
                    .h(px(6.))
                    .bg(rgb(HOVER))
                    .cursor_pointer()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                            let fraction = f64::from(event.position.x / width);
                            this.seek_to(fraction * duration, cx);
                        }),
                    )
                    .child(div().h_full().w(relative(progress)).bg(rgb(ACCENT))),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_4()
                    .px_5()
                    .py_3()
                    .when(track.is_some(), |bar| {
                        bar.child(
                            div()
                                .size(px(COVER_SIZE))
                                .flex_none()
                                .rounded_md()
                                .overflow_hidden()
                                .bg(rgb(HOVER))
                                .when_some(self.cover.clone(), |frame, cover| {
                                    frame.child(img(cover).size_full().object_fit(ObjectFit::Cover))
                                }),
                        )
                    })
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(div().truncate().text_color(rgb(TEXT)).child(
                                track.map_or(SharedString::from("Aucune lecture"), |t| {
                                    t.title.clone().into()
                                }),
                            ))
                            .child(div().truncate().text_sm().text_color(rgb(MUTED)).child(
                                track.map_or(SharedString::default(), |t| {
                                    if t.format.is_empty() {
                                        t.artist.clone().into()
                                    } else {
                                        format!("{} · {}", t.artist, t.format).into()
                                    }
                                }),
                            )),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                button("previous", "Préc.")
                                    .on_click(cx.listener(|this, _, _, cx| this.previous(cx))),
                            )
                            .child(
                                button("play", if paused { "Lecture" } else { "Pause" })
                                    .on_click(cx.listener(|this, _, _, cx| this.toggle_play(cx))),
                            )
                            .child(
                                button("next", "Suiv.")
                                    .on_click(cx.listener(|this, _, _, cx| this.next(cx))),
                            )
                            .child(
                                button(
                                    "lyrics",
                                    if self.show_lyrics { "Liste" } else { "Paroles" },
                                )
                                .on_click(cx.listener(|this, _, _, cx| this.toggle_lyrics(cx))),
                            ),
                    )
                    .when(!compact, |bar| {
                        bar.child(
                            div()
                                .flex_1()
                                .flex()
                                .items_center()
                                .justify_end()
                                .gap_2()
                                .text_sm()
                                .text_color(rgb(MUTED))
                                .child(format!(
                                    "{} / {}",
                                    format_time(self.position),
                                    format_time(duration)
                                ))
                                .child(button("volume-down", "−").on_click(cx.listener(
                                    |this, _, _, cx| this.change_volume(-VOLUME_STEP, cx),
                                )))
                                .child(format!("{:.0} %", volume * 100.0))
                                .child(button("volume-up", "+").on_click(cx.listener(
                                    |this, _, _, cx| this.change_volume(VOLUME_STEP, cx),
                                ))),
                        )
                    }),
            )
    }
}

fn text_alpha(alpha: f32) -> Hsla {
    let mut color: Hsla = rgb(TEXT).into();
    color.a = alpha;
    color
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
    text.with_highlights(line_fills(line, ms).into_iter().map(|(range, fill)| {
        let style = HighlightStyle {
            color: Some(text_alpha(fill)),
            ..Default::default()
        };
        (range, style)
    }))
}

fn origin_label(origin: Origin) -> impl IntoElement {
    div()
        .pt_2()
        .text_xs()
        .text_color(rgb(MUTED))
        .child(format!("Source : {}", origin.label()))
}

fn image_format(bytes: &[u8]) -> Option<ImageFormat> {
    match bytes {
        [0xff, 0xd8, ..] => Some(ImageFormat::Jpeg),
        [0x89, b'P', b'N', b'G', ..] => Some(ImageFormat::Png),
        [
            b'R',
            b'I',
            b'F',
            b'F',
            _,
            _,
            _,
            _,
            b'W',
            b'E',
            b'B',
            b'P',
            ..,
        ] => Some(ImageFormat::Webp),
        [b'G', b'I', b'F', ..] => Some(ImageFormat::Gif),
        _ => None,
    }
}

fn button(id: &'static str, label: &'static str) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .px_3()
        .py_1()
        .rounded_md()
        .bg(rgb(HOVER))
        .text_sm()
        .text_color(rgb(TEXT))
        .cursor_pointer()
        .hover(|button| button.bg(rgb(SELECTED)))
        .child(label)
}

impl Render for Onde {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .track_focus(&self.focus)
            .key_context(if self.downloads.open {
                "Recherche"
            } else {
                "Lecteur"
            })
            .on_key_down(cx.listener(|this, event, _, cx| this.type_in_search(event, cx)))
            .on_action(cx.listener(|this, _: &ToggleDownloads, _, cx| this.toggle_downloads(cx)))
            .on_action(cx.listener(|this, _: &SubmitSearch, _, cx| this.search(cx)))
            .on_action(cx.listener(|this, _: &EraseSearch, _, cx| this.erase_in_search(cx)))
            .on_action(cx.listener(|this, _: &PasteSearch, _, cx| this.paste_in_search(cx)))
            .on_action(cx.listener(|this, _: &TogglePlay, _, cx| this.toggle_play(cx)))
            .on_action(cx.listener(|this, _: &NextTrack, _, cx| this.next(cx)))
            .on_action(cx.listener(|this, _: &PreviousTrack, _, cx| this.previous(cx)))
            .on_action(cx.listener(|this, _: &SeekForward, _, cx| {
                this.seek_to(this.position + SEEK_STEP, cx)
            }))
            .on_action(cx.listener(|this, _: &SeekBackward, _, cx| {
                this.seek_to(this.position - SEEK_STEP, cx)
            }))
            .on_action(cx.listener(|this, _: &ToggleLyrics, _, cx| this.toggle_lyrics(cx)))
            .on_action(cx.listener(|this, _: &ToggleVisuals, _, cx| this.toggle_visuals(cx)))
            .on_action(cx.listener(|this, _: &VolumeUp, _, cx| this.change_volume(VOLUME_STEP, cx)))
            .on_action(
                cx.listener(|this, _: &VolumeDown, _, cx| this.change_volume(-VOLUME_STEP, cx)),
            )
            .on_action(cx.listener(|this, _: &OpenNavidrome, _, cx| this.open_navidrome(cx)))
            .flex()
            .flex_col()
            .size_full()
            .bg(rgb(BACKGROUND))
            .text_color(rgb(TEXT))
            .child(self.render_header(cx))
            .when_some(self.message.clone(), |root, message| {
                root.child(
                    div()
                        .px_5()
                        .pb_2()
                        .text_sm()
                        .text_color(rgb(ERROR))
                        .child(message),
                )
            })
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .when(self.show_visuals, |area| area.child(self.render_visuals()))
                    .child(if self.ask_folder {
                        self.render_setup(cx).into_any_element()
                    } else if self.downloads.open {
                        self.render_downloads(cx).into_any_element()
                    } else if self.quality_open {
                        self.render_quality(cx).into_any_element()
                    } else if self.show_lyrics {
                        self.render_lyrics(cx)
                    } else {
                        self.render_list(cx).into_any_element()
                    }),
            )
            .child(self.render_bar(window, cx))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognises_images_by_their_first_bytes() {
        assert_eq!(image_format(&[0xff, 0xd8, 0xff]), Some(ImageFormat::Jpeg));
        assert_eq!(image_format(b"\x89PNG\r\n"), Some(ImageFormat::Png));
        assert_eq!(
            image_format(b"RIFF\0\0\0\0WEBPVP8"),
            Some(ImageFormat::Webp)
        );
        assert_eq!(image_format(b"<html>"), None);
        assert_eq!(image_format(b""), None);
    }
}
