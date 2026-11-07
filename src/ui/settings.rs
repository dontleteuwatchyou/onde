//! Sources and settings.

use gpui::{Div, FontWeight, linear_color_stop, linear_gradient};

use super::*;
use crate::discord::{Activity, Presence, Status, valid_client_id};
use crate::cover_host::Hosting;
use crate::online_cover::{Picture, upload_lasting};
use crate::theme::THEMES;

/// Seconds a track's start may move before Discord is told: more means a seek.
const PRESENCE_DRIFT: i64 = 3;

fn unix_seconds() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |age| age.as_secs() as i64)
}

const KEYS: &[(&str, &str)] = &[
    ("Espace", "Lecture ou pause"),
    ("Ctrl + ← / →", "Morceau précédent ou suivant"),
    ("← / →", "Reculer ou avancer de 5 s"),
    ("↑ / ↓", "Volume"),
    ("F", "Plein écran"),
    ("L", "Paroles"),
    ("V", "Visuels, puis visuel suivant"),
    ("Q", "File d’attente"),
    ("H", "Liker le morceau en cours"),
    ("S", "Lecture aléatoire"),
    ("D", "Téléchargements"),
    ("/", "Chercher dans la bibliothèque"),
    ("N", "Ouvrir Navidrome"),
    ("Échap", "Fermer, quitter le plein écran"),
];

fn card() -> Div {
    div()
        .flex()
        .flex_col()
        .gap_2()
        .p_4()
        .rounded(px(16.))
        .bg(pal().rise)
}

fn source_title(title: &'static str, on: bool, state: &'static str) -> Div {
    div()
        .flex()
        .items_center()
        .gap_2()
        .child(
            div()
                .size(px(8.))
                .flex_none()
                .rounded_full()
                .bg(if on { GOOD } else { ERROR }),
        )
        .child(div().font_weight(FontWeight::SEMIBOLD).child(title))
        .child(div().flex_1())
        .child(mono(state).text_color(pal().tx3))
}

impl Onde {
    pub(super) fn set_theme(&mut self, name: &'static str, cx: &mut Context<Self>) {
        set_palette(Palette::named(name));
        self.save_settings(|settings| settings.theme = Some(name.to_owned()));
        cx.notify();
    }

    pub(super) fn set_visual(&mut self, visual: Visual, cx: &mut Context<Self>) {
        self.visual = visual;
        self.save_settings(|settings| settings.visual = Some(visual.name().to_owned()));
        cx.notify();
    }

    fn discord_text(&self) -> &'static str {
        match self.presence.as_ref().map(Presence::status) {
            None => "Coupé",
            Some(Status::Connected) => "Connecté",
            Some(Status::Refused) => "Identifiant refusé par Discord",
            Some(Status::Searching) => "Discord n’est pas ouvert",
        }
    }

    pub(super) fn restart_presence(&mut self) {
        let settings = config::load();
        self.discord_id_saved = settings.discord_client_id.is_some();
        self.presence = settings
            .discord_client_id
            .filter(|_| settings.discord_enabled)
            .map(|id| Presence::start(id, self.cover_links.clone()));
        self.presence_sent = None;
    }

    fn paste_discord_id(&mut self, cx: &mut Context<Self>) {
        let pasted = cx.read_from_clipboard().and_then(|item| item.text());
        let Some(id) = pasted.as_deref().and_then(valid_client_id) else {
            self.message = Some("Le presse-papiers ne contient pas d’identifiant Discord.".into());
            cx.notify();
            return;
        };
        self.message = None;
        self.save_settings(|settings| {
            settings.discord_client_id = Some(id);
            settings.discord_enabled = true;
        });
        self.restart_presence();
        cx.notify();
    }

    /// Goes through what isn't hosted yet. Without the setting, stops what runs.
    pub(super) fn restart_hosting(&mut self) {
        self.hosting = None;
        let (true, Some(music)) = (self.host_covers, self.source.clone()) else {
            return;
        };
        let jobs: Vec<(String, Picture)> = self
            .catalog
            .albums
            .iter()
            .filter(|album| self.cover_links.get(&album.key).is_none())
            .filter_map(|album| {
                let track = self.queue.tracks.get(*album.tracks.first()?)?.clone();
                let music = music.clone();
                let picture = Box::new(move || music.cover(&track).ok().flatten()) as Picture;
                Some((album.key.clone(), picture))
            })
            .collect();
        self.hosting = Some(Hosting::start(
            jobs,
            self.cover_links.clone(),
            upload_lasting,
        ));
    }

    fn toggle_hosting(&mut self, cx: &mut Context<Self>) {
        self.host_covers = !self.host_covers;
        let on = self.host_covers;
        self.save_settings(|settings| settings.host_covers = on);
        self.restart_hosting();
        cx.notify();
    }

    fn toggle_discord(&mut self, cx: &mut Context<Self>) {
        let on = self.presence.is_none();
        self.save_settings(|settings| settings.discord_enabled = on);
        self.restart_presence();
        cx.notify();
    }

    /// Tells Discord what is playing, when that changed.
    pub(super) fn update_presence(&mut self) {
        let Some(presence) = &self.presence else {
            return;
        };
        let paused = self.engine.as_ref().is_none_or(Engine::is_paused);
        let track = self.queue.current_track().filter(|_| !paused);
        let sent = track.map(|track| {
            // When the track would have started: it only moves when seeking.
            let start = unix_seconds() - self.position as i64;
            (crate::collections::track_id(track), start)
        });
        let same = match (&sent, &self.presence_sent) {
            (None, None) => true,
            (Some((id, start)), Some((old_id, old_start))) => {
                id == old_id && (start - old_start).abs() <= PRESENCE_DRIFT
            }
            _ => false,
        };
        if same {
            return;
        }
        let picture = track.cloned().zip(self.source.clone()).map(|(track, music)| {
            Box::new(move || music.cover(&track).ok().flatten()) as Picture
        });
        presence.show(
            track.map(|track| Activity {
                title: track.title.clone(),
                artist: track.artist.clone(),
                album: track.album.clone(),
                cover_key: album_key(track),
                position: self.position,
                duration: track.duration,
            }),
            picture,
        );
        self.presence_sent = sent;
    }

    fn render_discord(&mut self, cx: &mut Context<Self>) -> Div {
        let palette = pal();
        let has_id = self.discord_id_saved;
        let on = self.presence.is_some();
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(h2("Discord"))
                    .when(has_id, |row| {
                        row.child(
                            chip("discord-power", if on { "Activé" } else { "Coupé" }, on)
                                .on_click(cx.listener(|this, _, _, cx| this.toggle_discord(cx))),
                        )
                    })
                    .child(mono(self.discord_text()).text_color(palette.tx3)),
            )
            .child(
                div()
                    .text_size(px(13.))
                    .text_color(palette.tx2)
                    .child("Affiche le morceau en cours sur ton profil, avec son temps. Discord reçoit le titre, l’artiste et l’album tant que c’est activé. Pour la pochette, Onde cherche l’artiste, l’album et le titre sur Apple Music, puis Deezer. S’ils ne trouvent rien, la pochette seule part 24 h sur litterbox.catbox.moe, visible par qui a le lien."),
            )
            .child(
                div()
                    .text_size(px(13.))
                    .text_color(palette.tx2)
                    .child("Crée une application sur discord.com/developers, copie son « Application ID », puis colle-le ici."),
            )
            .child(
                div().flex().child(
                    btn(
                        "discord-paste",
                        if has_id {
                            "Remplacer l’identifiant"
                        } else {
                            "Coller l’identifiant"
                        },
                    )
                    .on_click(cx.listener(|this, _, _, cx| this.paste_discord_id(cx))),
                ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(
                        chip(
                            "host-covers",
                            "Héberger toutes les pochettes à l’avance",
                            self.host_covers,
                        )
                        .on_click(cx.listener(|this, _, _, cx| this.toggle_hosting(cx))),
                    )
                    .when_some(self.hosting.as_ref(), |row, hosting| {
                        row.child(mono(hosting.describe()).text_color(palette.tx3))
                    }),
            )
            .child(
                div()
                    .text_size(px(13.))
                    .text_color(palette.tx2)
                    .child("Envoie les pochettes de la bibliothèque sur catbox.moe, pour que Discord les affiche tout de suite. Elles y restent, visibles par qui a le lien, et ne peuvent pas être supprimées ensuite. Couper le réglage arrête les envois, pas ce qui est déjà en ligne."),
            )
    }

    pub(super) fn save_settings(&mut self, change: impl FnOnce(&mut config::Config)) {
        let mut settings = config::load();
        change(&mut settings);
        if let Err(error) = config::save(&settings) {
            self.message = Some(format!("Réglage non enregistré : {error:#}").into());
        }
    }

    pub(super) fn render_sources(&mut self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let palette = pal();
        let settings = config::load();
        let folder_open = self
            .source
            .as_ref()
            .and_then(|source| source.folder().map(|path| path.to_path_buf()));
        let server_open = self.source.is_some() && folder_open.is_none();
        let count = self.queue.tracks.len();
        let detail = |text: String| {
            div()
                .text_size(px(13.))
                .text_color(palette.tx2)
                .child(text)
        };
        let folder_text = match (&folder_open, &settings.music_folder) {
            (Some(folder), _) => format!("{} · {count} morceaux", folder.display()),
            (None, Some(folder)) => folder.display().to_string(),
            (None, None) => "Aucun dossier choisi.".to_owned(),
        };
        div()
            .id("sources")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .px_8()
            .pt_2()
            .pb(px(56.))
            .flex()
            .flex_col()
            .gap_4()
            .child(display("Sources", 40.))
            .child(
                card()
                    .child(source_title(
                        "Dossier local",
                        folder_open.is_some(),
                        if folder_open.is_some() { "Ouvert" } else { "Fermé" },
                    ))
                    .child(detail(folder_text))
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .child(
                                btn("pick-folder", "Ouvrir un dossier")
                                    .on_click(cx.listener(|this, _, _, cx| this.pick_folder(cx))),
                            )
                            .child(ghost("music-folder", "Dossier de musique").on_click(
                                cx.listener(|this, _, _, cx| this.pick_music_folder(cx)),
                            )),
                    ),
            )
            .child(
                card()
                    .child(source_title(
                        "Navidrome",
                        server_open,
                        if server_open { "Connecté" } else { "Fermé" },
                    ))
                    .child(match &settings.navidrome {
                        Some(credentials) => detail(credentials.url.clone()),
                        None => mono("Dans un terminal : onde navidrome-login ADRESSE UTILISATEUR")
                            .text_color(palette.tx2),
                    })
                    .when(settings.navidrome.is_some(), |panel| {
                        panel.child(
                            div().flex().child(
                                btn("open-navidrome", "Ouvrir le serveur").on_click(
                                    cx.listener(|this, _, _, cx| this.open_navidrome(cx)),
                                ),
                            ),
                        )
                    }),
            )
            .child(
                card()
                    .child(source_title("Spotify", false, "Pas encore branché"))
                    .child(detail(
                        "Prévu : importer tes playlists, puis retrouver les morceaux sur tes autres sources."
                            .to_owned(),
                    )),
            )
    }

    pub(super) fn render_settings(&mut self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let palette = pal();
        let mut visuals_grid = div().flex().flex_wrap().gap_3();
        for (index, choice) in VISUALS.into_iter().enumerate() {
            let on = choice == self.visual;
            visuals_grid = visuals_grid.child(
                div()
                    .id(("visual", index))
                    .w(px(200.))
                    .flex()
                    .flex_col()
                    .gap_1()
                    .p_3()
                    .rounded(px(16.))
                    .bg(if on { palette.rise2 } else { palette.rise })
                    .when(on, |card| card.border_1().border_color(palette.spark))
                    .cursor_pointer()
                    .hover(|card| card.bg(pal().rise2))
                    .on_click(cx.listener(move |this, _, _, cx| this.set_visual(choice, cx)))
                    .child(div().font_weight(FontWeight::SEMIBOLD).child(choice.name()))
                    .child(
                        div()
                            .text_size(px(12.))
                            .line_height(px(16.))
                            .text_color(palette.tx2)
                            .child(choice.describe()),
                    ),
            );
        }
        let mut themes = div().flex().flex_wrap().gap_3();
        for (index, theme) in THEMES.iter().enumerate() {
            let swatch = Palette::new(theme);
            let on = palette.name == theme.0;
            let name = theme.0;
            themes = themes.child(
                div()
                    .id(("theme", index))
                    .w(px(120.))
                    .flex()
                    .flex_col()
                    .gap_1()
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| this.set_theme(name, cx)))
                    .child(
                        div()
                            .h(px(64.))
                            .rounded(px(10.))
                            .border_2()
                            .border_color(if on { palette.spark } else { palette.line })
                            .bg(linear_gradient(
                                135.,
                                linear_color_stop(swatch.ink, 0.3),
                                linear_color_stop(swatch.tide, 1.),
                            ))
                            .flex()
                            .items_end()
                            .justify_end()
                            .p_2()
                            .child(div().size(px(16.)).rounded_full().bg(swatch.spark)),
                    )
                    .child(div().text_size(px(13.)).child(name)),
            );
        }
        let mut keys = div().flex().flex_col().gap_1();
        for (key, what) in KEYS {
            keys = keys.child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(div().w(px(120.)).flex().child(tag(*key)))
                    .child(
                        div()
                            .text_size(px(13.))
                            .text_color(palette.tx2)
                            .child(*what),
                    ),
            );
        }
        let power = if self.show_visuals { "Activés" } else { "Coupés" };
        div()
            .id("settings")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .px_8()
            .pt_2()
            .pb(px(56.))
            .flex()
            .flex_col()
            .gap_6()
            .child(display("Réglages", 40.))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_3()
                            .child(h2("Visuels"))
                            .child(
                                chip("visuals-power", power, self.show_visuals).on_click(
                                    cx.listener(|this, _, _, cx| this.toggle_visuals(cx)),
                                ),
                            ),
                    )
                    .child(visuals_grid),
            )
            .child(div().flex().flex_col().gap_3().child(h2("Thème")).child(themes))
            .child(self.render_discord(cx))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(h2("Raccourcis"))
                    .child(keys),
            )
    }
}
