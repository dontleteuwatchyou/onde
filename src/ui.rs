//! The window: navigation between the screens, and what they all share.

mod browse;
mod covers;
mod downloads;
mod layouts;
mod player;
mod playlists;
mod quality;
mod settings;
mod setup;
mod updates;
mod widgets;

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{
    AnyElement, App, Bounds, BoxShadow, Context, FocusHandle, FontWeight, HighlightStyle,
    KeyBinding, MouseButton, MouseDownEvent, PathPromptOptions, Pixels, SharedString, StyledText,
    Window, actions, canvas, div, point, prelude::*, px, relative, uniform_list,
};

use crate::analysis::Analyzer;
use crate::catalog::{Catalog, TrackSort, album_key};
use crate::collections::Collections;
use crate::config;
use crate::cover_host::{Hosting, Links};
use crate::discord::Presence;
use crate::engine::Engine;
use crate::library::{Location, format_time};
use crate::lyrics::source::{self, Origin};
use crate::lyrics::{Line, Lyrics, active_line, line_fills};
use crate::queue::{Previous, Queue};
use crate::sources::MusicSource;
use crate::sources::local::LocalFolder;
use crate::sources::subsonic::Subsonic;
use crate::theme::rad;
use crate::theme::{Layout, Look, Palette, alpha, look, pal, set_look, set_palette};
use crate::visuals::{self, VISUALS, Visual, VisualState, WaterLine};
use browse::LibraryView;
use covers::CoverArt;
use widgets::{
    ERROR, GOOD, btn, chip, display, ghost, h2, icon, icon_button, icon_small, mono, primary, tag,
};

/// Below this width the navigation moves to the bottom and the bar slims down.
const COMPACT_WIDTH: f32 = 760.0;

const VOLUME_STEP: f32 = 0.1;
const SEEK_STEP: f64 = 5.0;
const RECENT_ALBUMS: usize = 12;

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
        ToggleQueue,
        ToggleFullscreen,
        LikeCurrent,
        ToggleShuffle,
        Leave,
        VolumeUp,
        VolumeDown,
        OpenNavidrome,
        ToggleDownloads,
        StartSearch,
        SubmitSearch,
        LeaveSearch,
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

#[derive(Clone, PartialEq)]
enum Screen {
    Home,
    Library,
    Album(String),
    Artist(String),
    Playlists,
    Liked,
    Playlist(usize),
    Downloads,
    Sources,
    Settings,
}

fn random_seed() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(1, |age| age.as_nanos() as u64)
}

const NAVIGATION: [(&str, &str, Screen); 6] = [
    ("Accueil", "home", Screen::Home),
    ("Bibliothèque", "lib", Screen::Library),
    ("Playlists", "listplus", Screen::Playlists),
    ("Téléchargements", "dl", Screen::Downloads),
    ("Sources", "server", Screen::Sources),
    ("Réglages", "sliders", Screen::Settings),
];

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
        KeyBinding::new("q", ToggleQueue, Some("Lecteur")),
        KeyBinding::new("f", ToggleFullscreen, Some("Lecteur")),
        KeyBinding::new("h", LikeCurrent, Some("Lecteur")),
        KeyBinding::new("s", ToggleShuffle, Some("Lecteur")),
        KeyBinding::new("escape", Leave, Some("Lecteur")),
        KeyBinding::new("up", VolumeUp, Some("Lecteur")),
        KeyBinding::new("down", VolumeDown, Some("Lecteur")),
        KeyBinding::new("d", ToggleDownloads, Some("Lecteur")),
        KeyBinding::new("/", StartSearch, Some("Lecteur")),
        // While a search field is active, typing goes to it.
        KeyBinding::new("escape", LeaveSearch, Some("Recherche")),
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
    catalog: Catalog,
    source: Option<Arc<dyn MusicSource>>,
    engine: Option<Engine>,
    message: Option<SharedString>,
    position: f64,
    scanning: bool,
    lyrics: LyricsState,
    /// Bumped on each track change, so a slow lookup can't show the wrong lyrics.
    lyrics_request: u64,
    /// In full screen: the lyrics rather than the title.
    show_lyrics: bool,
    analyzer: Rc<RefCell<Analyzer>>,
    visual: Visual,
    visual_state: Rc<RefCell<VisualState>>,
    water_line: Rc<RefCell<WaterLine>>,
    volume_bounds: Rc<Cell<Bounds<Pixels>>>,
    show_visuals: bool,
    fullscreen: bool,
    show_queue: bool,
    screen: Screen,
    library_view: LibraryView,
    library_query: String,
    library_typing: bool,
    track_sort: TrackSort,
    visible_tracks: Vec<usize>,
    visible_albums: Vec<usize>,
    visible_artists: Vec<usize>,
    update: updates::UpdateState,
    update_dismissed: bool,
    /// Being dragged to, as a fraction of the track.
    seek_drag: Option<f32>,
    volume_drag: bool,
    muted_volume: Option<f32>,
    recent: Vec<String>,
    collections: Collections,
    track_index: HashMap<String, usize>,
    /// The track being added to a playlist, while that menu is open.
    adding_to: Option<usize>,
    /// The name being typed for a new playlist.
    naming: Option<String>,
    /// The playlist being filled from the whole library, and the filter typed there.
    filling: Option<usize>,
    fill_query: String,
    /// The delete button has been pressed once.
    deleting: bool,
    presence: Option<Presence>,
    cover_links: Arc<Links>,
    host_covers: bool,
    hosting: Option<Hosting>,
    discord_id_saved: bool,
    /// What Discord was last told: the track and when it started.
    presence_sent: Option<(String, i64)>,
    last_tick: Instant,
    last_audio: Instant,
    downloads: downloads::Downloads,
    /// First launch, before a music folder was chosen.
    ask_folder: bool,
    /// By album. `None`: being fetched, or there is no picture.
    covers: HashMap<String, Option<Arc<CoverArt>>>,
    covers_wanted: Vec<String>,
    covers_running: usize,
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
        let settings = config::load();
        if let Some(theme) = &settings.theme {
            set_palette(Palette::named(theme));
        }
        set_look(Look::from_names(
            settings.layout.as_deref(),
            settings.rounding.as_deref(),
            settings.density.as_deref(),
            settings.round_covers,
        ));
        let rate = engine.as_ref().map_or(44_100, Engine::sample_rate);
        let mut this = Self {
            quality_open: false,
            quality_busy: false,
            quality_title: String::new(),
            quality_report: None,
            quality_error: None,
            queue: Queue::default(),
            catalog: Catalog::default(),
            source: None,
            engine,
            message,
            position: 0.0,
            scanning: false,
            lyrics: LyricsState::Idle,
            lyrics_request: 0,
            show_lyrics: false,
            analyzer: Rc::new(RefCell::new(Analyzer::new(rate))),
            visual: settings
                .visual
                .as_deref()
                .and_then(Visual::from_name)
                .unwrap_or(Visual::Maree),
            visual_state: Rc::default(),
            water_line: Rc::default(),
            volume_bounds: Rc::default(),
            show_visuals: true,
            fullscreen: false,
            show_queue: false,
            screen: Screen::Home,
            library_view: LibraryView::Tracks,
            library_query: String::new(),
            library_typing: false,
            track_sort: TrackSort::Artist,
            visible_tracks: Vec::new(),
            visible_albums: Vec::new(),
            visible_artists: Vec::new(),
            update: updates::UpdateState::default(),
            update_dismissed: false,
            seek_drag: None,
            volume_drag: false,
            muted_volume: None,
            recent: settings.recent_albums,
            collections: settings.collections,
            track_index: HashMap::new(),
            adding_to: None,
            naming: None,
            filling: None,
            fill_query: String::new(),
            deleting: false,
            presence: None,
            cover_links: Arc::new(Links::load()),
            host_covers: settings.host_covers,
            hosting: None,
            discord_id_saved: false,
            presence_sent: None,
            last_tick: Instant::now(),
            last_audio: Instant::now(),
            downloads: downloads::Downloads::new(),
            ask_folder: false,
            covers: HashMap::new(),
            covers_wanted: Vec::new(),
            covers_running: 0,
            focus,
        };
        match startup {
            Startup::Empty => match settings.music_folder {
                Some(folder) if folder.is_dir() => {
                    this.open_source(Arc::new(LocalFolder(folder)), cx);
                }
                _ => this.ask_folder = true,
            },
            Startup::Folder(folder) => this.open_source(Arc::new(LocalFolder(folder)), cx),
            Startup::Navidrome => this.open_navidrome(cx),
        }
        this.restart_presence();
        this.queue.shuffle = config::load().shuffle;
        this.start_update_checks(cx);
        this
    }

    fn go(&mut self, screen: Screen, cx: &mut Context<Self>) {
        self.screen = screen;
        self.library_typing = false;
        self.quality_open = false;
        self.fullscreen = false;
        self.adding_to = None;
        self.naming = None;
        self.filling = None;
        self.deleting = false;
        cx.notify();
    }

    /// Plays from the whole library, whatever was playing before.
    fn start(&mut self, index: usize, cx: &mut Context<Self>) {
        self.queue.follow(index, Vec::new(), random_seed());
        self.play(index, cx);
    }

    /// Plays from a list: what follows is the rest of that list.
    fn start_in(&mut self, index: usize, order: Vec<usize>, cx: &mut Context<Self>) {
        self.queue.follow(index, order, random_seed());
        self.play(index, cx);
    }

    fn tick(&mut self, cx: &mut Context<Self>) -> Duration {
        self.poll_telegram(cx);
        if self.screen == Screen::Downloads && self.downloads.needs_updates() {
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
        self.update_presence();
        let moving = self.update_visuals(cx);
        let playing =
            self.queue.current.is_some() && self.engine.as_ref().is_some_and(|e| !e.is_paused());
        if moving || playing {
            FAST_TICK
        } else {
            SLOW_TICK
        }
    }

    /// Returns whether what follows the sound needs another frame soon.
    fn update_visuals(&mut self, cx: &mut Context<Self>) -> bool {
        let dt = self.last_tick.elapsed().as_secs_f32();
        self.last_tick = Instant::now();
        let Some(engine) = &self.engine else {
            return false;
        };
        let mut analyzer = self.analyzer.borrow_mut();
        let was_still = analyzer.is_quiet() && analyzer.activity() < 0.01;
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
        if hearing {
            analyzer.update(dt);
        } else {
            analyzer.release(dt);
        }
        let moving = !was_still || !(analyzer.is_quiet() && analyzer.activity() < 0.01);
        if moving {
            cx.notify();
        }
        moving || hearing
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
                    this.catalog_changed();
                    // Those that had no picture may have one now.
                    this.covers.retain(|_, art| art.is_some());
                    crate::memory::give_back();
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
                        // Pictures stay on the graphics card until handed back.
                        for art in this.covers.drain().filter_map(|(_, art)| art) {
                            cx.drop_image(art.image.clone(), None);
                        }
                        this.covers_wanted.clear();
                        this.catalog_changed();
                        this.position = 0.0;
                        this.load_lyrics(cx);
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
        self.remember_album();
        self.load_lyrics(cx);
        cx.notify();
    }

    fn remember_album(&mut self) {
        let Some(key) = self.queue.current_track().map(album_key) else {
            return;
        };
        if self.recent.first() == Some(&key) {
            return;
        }
        self.recent.retain(|known| *known != key);
        self.recent.insert(0, key);
        self.recent.truncate(RECENT_ALBUMS);
        let recent = self.recent.clone();
        self.save_settings(|settings| settings.recent_albums = recent);
    }

    fn stop(&mut self, cx: &mut Context<Self>) {
        if let Some(engine) = &self.engine {
            engine.stop();
        }
        self.queue.current = None;
        self.position = 0.0;
        self.load_lyrics(cx);
        cx.notify();
    }

    fn toggle_play(&mut self, cx: &mut Context<Self>) {
        let Some(engine) = &self.engine else { return };
        if self.queue.current.is_none() {
            if let Some(first) = self.queue.first_index(random_seed()) {
                self.start(first, cx);
            }
            return;
        }
        engine.set_paused(!engine.is_paused());
        cx.notify();
    }

    fn toggle_shuffle(&mut self, cx: &mut Context<Self>) {
        self.queue.toggle_shuffle(random_seed());
        let on = self.queue.shuffle;
        self.save_settings(|settings| settings.shuffle = on);
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

    /// Opens the visuals; when they're already showing, moves on to the next one.
    fn visual_key(&mut self, cx: &mut Context<Self>) {
        if self.fullscreen && !self.show_lyrics {
            let now = VISUALS.iter().position(|v| *v == self.visual).unwrap_or(0);
            self.set_visual(VISUALS[(now + 1) % VISUALS.len()], cx);
        } else {
            self.fullscreen = true;
            self.show_lyrics = false;
            cx.notify();
        }
    }

    fn toggle_lyrics(&mut self, cx: &mut Context<Self>) {
        if self.fullscreen && self.show_lyrics {
            self.show_lyrics = false;
        } else {
            self.fullscreen = true;
            self.show_lyrics = true;
        }
        cx.notify();
    }

    fn toggle_fullscreen(&mut self, cx: &mut Context<Self>) {
        self.fullscreen = !self.fullscreen;
        cx.notify();
    }

    fn toggle_queue(&mut self, cx: &mut Context<Self>) {
        self.show_queue = !self.show_queue;
        cx.notify();
    }

    fn leave(&mut self, cx: &mut Context<Self>) {
        if self.adding_to.is_some() {
            self.adding_to = None;
        } else if self.filling.is_some() {
            self.filling = None;
        } else if self.fullscreen {
            self.fullscreen = false;
        } else if self.show_queue {
            self.show_queue = false;
        } else if self.quality_open {
            self.quality_open = false;
        }
        cx.notify();
    }

    fn start_search(&mut self, cx: &mut Context<Self>) {
        self.screen = Screen::Library;
        self.library_typing = true;
        self.quality_open = false;
        self.fullscreen = false;
        cx.notify();
    }

    fn change_volume(&mut self, delta: f32, cx: &mut Context<Self>) {
        if let Some(engine) = &self.engine {
            engine.set_volume(engine.volume() + delta);
        }
        cx.notify();
    }

    fn nav_active(&self, target: &Screen) -> bool {
        match (&self.screen, target) {
            (Screen::Album(_) | Screen::Artist(_), Screen::Library) => true,
            (Screen::Liked | Screen::Playlist(_), Screen::Playlists) => true,
            (current, target) => current == target,
        }
    }

    fn render_top(&mut self, compact: bool, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let palette = pal();
        let back = match self.screen {
            Screen::Album(_) | Screen::Artist(_) => Some(Screen::Library),
            Screen::Liked | Screen::Playlist(_) => Some(Screen::Playlists),
            _ => None,
        };
        let logo = div()
            .relative()
            .size(px(28.))
            .flex_none()
            .child(icon("logo-top", 28., palette.spark).absolute())
            .child(icon("logo-bottom", 28., palette.tx).absolute());
        let mut nav = div()
            .flex()
            .items_center()
            .gap_1()
            .flex_1()
            .min_w_0()
            .overflow_hidden();
        let classic = compact || look().layout == Layout::Classique;
        if !compact && classic {
            for (index, (label, _, target)) in NAVIGATION.into_iter().enumerate() {
                let on = self.nav_active(&target);
                nav = nav.child(
                    div()
                        .id(("nav", index))
                        .h(px(36.))
                        .flex()
                        .items_center()
                        .px_3()
                        .rounded(rad(10.))
                        .cursor_pointer()
                        .font_weight(FontWeight::MEDIUM)
                        .when(on, |item| {
                            item.bg(palette.rise2).shadow(vec![BoxShadow {
                                color: alpha(palette.tide, 0.6).into(),
                                offset: point(px(0.), px(0.)),
                                blur_radius: px(14.),
                                spread_radius: px(0.),
                            }])
                        })
                        .when(!on, |item| item.hover(|item| item.bg(pal().rise)))
                        .on_click(cx.listener(move |this, _, _, cx| this.go(target.clone(), cx)))
                        .child(label),
                );
            }
        }
        let empty = !classic && back.is_none();
        div()
            .h(px(if empty { 16. } else { 56. }))
            .flex_none()
            .flex()
            .items_center()
            .gap_6()
            .px_6()
            .when_some(back, |top, back| {
                top.child(
                    icon_button("back", "back", false)
                        .on_click(cx.listener(move |this, _, _, cx| this.go(back.clone(), cx))),
                )
            })
            .when(classic, |top| {
                top.child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(logo)
                        .child(display("onde", 22.)),
                )
            })
            .child(nav)
    }

    fn render_tabs(&mut self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let palette = pal();
        let mut tabs = div().h(px(56.)).flex_none().flex().bg(palette.deep);
        for (index, (label, name, target)) in NAVIGATION.into_iter().enumerate() {
            let on = self.nav_active(&target);
            let tint = if on { palette.spark } else { palette.tx2 };
            tabs = tabs.child(
                div()
                    .id(("tab", index))
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .text_size(px(10.))
                    .text_color(tint)
                    .on_click(cx.listener(move |this, _, _, cx| this.go(target.clone(), cx)))
                    .child(icon(name, 20., tint))
                    .child(div().truncate().child(label)),
            );
        }
        tabs
    }
}

impl Render for Onde {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = pal();
        let typing = self.naming.is_some()
            || self.filling.is_some()
            || (!self.fullscreen
                && !self.ask_folder
                && !self.quality_open
                && self.adding_to.is_none()
                && (self.screen == Screen::Downloads || self.library_typing));
        let compact = window.viewport_size().width < px(COMPACT_WIDTH);
        window.set_rem_size(px(look().density));
        let root = div()
            .track_focus(&self.focus)
            .key_context(if typing { "Recherche" } else { "Lecteur" })
            .on_key_down(cx.listener(move |this, event, _, cx| {
                if typing {
                    this.type_in_search(event, cx);
                }
            }))
            .on_mouse_move(cx.listener(|this, event, window, cx| {
                this.drag_moved(event, window, cx);
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| this.drag_ended(cx)),
            )
            .on_action(cx.listener(|this, _: &ToggleDownloads, _, cx| this.toggle_downloads(cx)))
            .on_action(cx.listener(|this, _: &StartSearch, _, cx| this.start_search(cx)))
            .on_action(cx.listener(|this, _: &SubmitSearch, _, cx| this.submit_search(cx)))
            .on_action(cx.listener(|this, _: &LeaveSearch, _, cx| this.leave_search(cx)))
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
            .on_action(cx.listener(|this, _: &ToggleVisuals, _, cx| this.visual_key(cx)))
            .on_action(cx.listener(|this, _: &ToggleQueue, _, cx| this.toggle_queue(cx)))
            .on_action(cx.listener(|this, _: &LikeCurrent, _, cx| this.like_current(cx)))
            .on_action(cx.listener(|this, _: &ToggleShuffle, _, cx| this.toggle_shuffle(cx)))
            .on_action(cx.listener(|this, _: &ToggleFullscreen, _, cx| this.toggle_fullscreen(cx)))
            .on_action(cx.listener(|this, _: &Leave, _, cx| this.leave(cx)))
            .on_action(cx.listener(|this, _: &VolumeUp, _, cx| this.change_volume(VOLUME_STEP, cx)))
            .on_action(
                cx.listener(|this, _: &VolumeDown, _, cx| this.change_volume(-VOLUME_STEP, cx)),
            )
            .on_action(cx.listener(|this, _: &OpenNavidrome, _, cx| this.open_navidrome(cx)))
            .relative()
            .flex()
            .flex_col()
            .size_full()
            .font_family("Instrument Sans")
            .text_size(px(14.))
            .line_height(px(20.))
            .bg(palette.ink)
            .text_color(palette.tx);
        let bar = self.render_bar(window, cx);
        let main = if self.fullscreen {
            self.render_fullscreen(window, cx).into_any_element()
        } else {
            let screen = self.screen.clone();
            let page = if self.ask_folder {
                self.render_setup(cx).into_any_element()
            } else if self.quality_open {
                self.render_quality(cx).into_any_element()
            } else {
                match &screen {
                    Screen::Home => self.render_home(window, cx).into_any_element(),
                    Screen::Library => self.render_library(window, cx).into_any_element(),
                    Screen::Album(key) => self.render_album(key, window, cx),
                    Screen::Artist(name) => self.render_artist(name, window, cx),
                    Screen::Playlists => self.render_playlists(cx).into_any_element(),
                    Screen::Liked => self.render_collection(None, window, cx),
                    Screen::Playlist(index) => self.render_collection(Some(*index), window, cx),
                    Screen::Downloads => self.render_downloads(window, cx).into_any_element(),
                    Screen::Sources => self.render_sources(cx).into_any_element(),
                    Screen::Settings => self.render_settings(cx).into_any_element(),
                }
            };
            div()
                .flex_1()
                .min_h_0()
                .flex()
                .flex_col()
                .child(self.render_top(compact, cx))
                .when_some(self.message.clone(), |column, message| {
                    column.child(
                        div()
                            .px_8()
                            .pb_2()
                            .text_size(px(13.))
                            .text_color(ERROR)
                            .child(message),
                    )
                })
                .children(self.render_update_banner(cx))
                .children(self.render_telegram_banner(cx))
                .child(div().flex_1().min_h_0().flex().flex_col().child(page))
                .into_any_element()
        };
        let queue = self.show_queue.then(|| self.render_queue(cx));
        let side = (!compact && !self.fullscreen)
            .then(|| self.render_side(cx))
            .flatten();
        root.child(
            div()
                .flex_1()
                .min_h_0()
                .flex()
                .children(side)
                .child(div().flex_1().min_w_0().flex().flex_col().child(main))
                .when_some(queue, |row, queue| row.child(queue)),
        )
        .child(bar)
        .when(compact && !self.fullscreen, |root| {
            root.child(self.render_tabs(cx))
        })
        .when(self.adding_to.is_some(), |root| {
            root.child(self.render_add_menu(cx))
        })
        .when(self.filling.is_some(), |root| {
            root.child(self.render_fill_menu(cx))
        })
    }
}
