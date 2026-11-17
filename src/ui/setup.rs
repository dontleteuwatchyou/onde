//! First launch: where does the music live?

use std::path::PathBuf;

use super::*;

/// `~/Musique/Onde`, wherever the system says music goes.
fn default_music_folder(audio: Option<PathBuf>, home: Option<PathBuf>) -> Option<PathBuf> {
    audio
        .or_else(|| home.map(|home| home.join("Musique")))
        .map(|music| music.join("Onde"))
}

impl Onde {
    /// Downloads go there too. What is playing keeps playing: the folder only
    /// opens by itself when nothing is, the download screen has a button otherwise.
    pub(super) fn set_music_folder(&mut self, folder: PathBuf, cx: &mut Context<Self>) {
        let mut settings = config::load();
        settings.music_folder = Some(folder.clone());
        if let Err(error) = config::save(&settings) {
            self.message = Some(format!("Dossier non enregistré : {error:#}").into());
        }
        self.downloads.folder = folder.clone();
        self.setup_step = 1;
        if self.queue.current.is_none() {
            self.open_source(Arc::new(LocalFolder(folder)), cx);
        }
        cx.notify();
    }

    pub(super) fn pick_music_folder(&mut self, cx: &mut Context<Self>) {
        let answer = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Choisir".into()),
        });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = answer.await else {
                return;
            };
            let Some(folder) = paths.into_iter().next() else {
                return;
            };
            let _ = this.update(cx, |this, cx| this.set_music_folder(folder, cx));
        })
        .detach();
    }

    fn create_music_folder(&mut self, folder: PathBuf, cx: &mut Context<Self>) {
        match std::fs::create_dir_all(&folder) {
            Ok(()) => self.set_music_folder(folder, cx),
            Err(error) => {
                self.message = Some(format!("Création impossible : {error}").into());
                cx.notify();
            }
        }
    }

    pub(super) fn render_setup(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let palette = pal();
        let step = self.setup_step;
        let total = 3u8;
        let wrapper = div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap_4()
            .px_8();

        let dots = div().flex().gap_2().children((0..total).map(|i| {
            div()
                .w(px(8.))
                .h(px(8.))
                .rounded_full()
                .bg(if i == step { palette.tide } else { palette.deep })
        }));

        let skip = ghost("setup-skip", "Passer").on_click(cx.listener(|this, _, _, cx| {
            this.setup_step += 1;
            if this.setup_step >= 3 {
                this.ask_folder = false;
            }
            cx.notify();
        }));

        match step {
            0 => {
                let new_folder =
                    default_music_folder(dirs::audio_dir(), dirs::home_dir());
                wrapper
                    .child(dots)
                    .child(display("Où ranger ta musique ?", 40.))
                    .child(
                        div()
                            .text_color(palette.tx2)
                            .child("Mets-y tes morceaux à la main : tes téléchargements iront aussi là."),
                    )
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .child(
                                primary("setup-pick", "Choisir un dossier")
                                    .on_click(cx.listener(|this, _, _, cx| this.pick_music_folder(cx))),
                            )
                            .when_some(new_folder.clone(), |row, folder| {
                                row.child(
                                    btn("setup-create", "Créer un dossier Onde").on_click(
                                        cx.listener(move |this, _, _, cx| {
                                            this.create_music_folder(folder.clone(), cx);
                                        }),
                                    ),
                                )
                            })
                            .child(skip),
                    )
                    .when_some(new_folder, |page, folder| {
                        page.child(
                            mono(format!("Le dossier créé sera : {}", folder.display()))
                                .text_color(palette.tx3),
                        )
                    })
            }
            1 => {
                let has_api = dirs::config_dir()
                    .map(|d| d.join("onde/telegram/api.env").exists())
                    .unwrap_or(false);
                let has_session = crate::telegram::Telegram::has_session();
                wrapper
                    .child(dots)
                    .child(display("Telegram", 40.))
                    .child(
                        div()
                            .max_w(px(520.))
                            .text_color(palette.tx2)
                            .child("Onde peut télécharger de la musique en FLAC via le bot @DeezLoad2Bot sur Telegram. C'est optionnel : YouTube est aussi disponible."),
                    )
                    .when(!has_api, |page| {
                        page.child(
                            div()
                                .max_w(px(520.))
                                .flex()
                                .flex_col()
                                .gap_2()
                                .child(
                                    div()
                                        .text_size(px(13.))
                                        .text_color(palette.tx2)
                                        .child("Pour configurer : crée une application sur my.telegram.org/apps, puis dans un terminal :"),
                                )
                                .child(
                                    div()
                                        .p_3()
                                        .rounded(px(10.))
                                        .bg(palette.deep)
                                        .font_family("DM Mono")
                                        .text_size(px(12.))
                                        .child(if cfg!(windows) { "mkdir %APPDATA%\\onde\\telegram" } else { "mkdir -p ~/.config/onde/telegram" })
                                        .child(div().child(if cfg!(windows) {
                                            "echo ONDE_TELEGRAM_API_ID=ton_id > %APPDATA%\\onde\\telegram\\api.env"
                                        } else {
                                            "echo 'ONDE_TELEGRAM_API_ID=ton_id' > ~/.config/onde/telegram/api.env"
                                        }))
                                        .child(div().child(if cfg!(windows) {
                                            "echo ONDE_TELEGRAM_API_HASH=ton_hash >> %APPDATA%\\onde\\telegram\\api.env"
                                        } else {
                                            "echo 'ONDE_TELEGRAM_API_HASH=ton_hash' >> ~/.config/onde/telegram/api.env"
                                        })),
                                ),
                        )
                    })
                    .when(has_api, |page| {
                        page.child(
                            div()
                                .text_size(px(13.))
                                .text_color(GOOD)
                                .child(if has_session {
                                    "Telegram est configuré et connecté."
                                } else {
                                    "Clés API détectées. Tu pourras te connecter dans Paramètres."
                                }),
                        )
                    })
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .child(
                                primary("setup-tg-next", "Continuer").on_click(cx.listener(|this, _, _, cx| {
                                    this.setup_step = 2;
                                    cx.notify();
                                })),
                            )
                            .child(skip),
                    )
            }
            _ => {
                let discord_on = config::load().discord_enabled;
                let host_on = config::load().host_covers;
                wrapper
                    .child(dots)
                    .child(display("Derniers réglages", 40.))
                    .child(
                        div()
                            .max_w(px(520.))
                            .flex()
                            .flex_col()
                            .gap_4()
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap_1()
                                    .child(h2("Discord Rich Presence"))
                                    .child(
                                        div()
                                            .text_size(px(13.))
                                            .text_color(palette.tx2)
                                            .child("Affiche ce que tu écoutes sur Discord avec la pochette."),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(13.))
                                            .text_color(if discord_on { GOOD } else { palette.tx3 })
                                            .child(if discord_on { "Activé" } else { "Désactivé — modifiable dans Paramètres" }),
                                    ),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap_1()
                                    .child(h2("Hébergement des pochettes"))
                                    .child(
                                        div()
                                            .text_size(px(13.))
                                            .text_color(palette.tx2)
                                            .child("Envoie les pochettes d'album sur un hébergeur pour les afficher dans Discord."),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(13.))
                                            .text_color(if host_on { GOOD } else { palette.tx3 })
                                            .child(if host_on { "Activé" } else { "Désactivé — modifiable dans Paramètres" }),
                                    ),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .child(
                                primary("setup-finish", "C'est parti !").on_click(cx.listener(|this, _, _, cx| {
                                    this.ask_folder = false;
                                    cx.notify();
                                })),
                            ),
                    )
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_new_folder_goes_inside_the_music_folder() {
        assert_eq!(
            default_music_folder(Some("/m/Musique".into()), Some("/m".into())),
            Some(PathBuf::from("/m/Musique/Onde"))
        );
        assert_eq!(
            default_music_folder(None, Some("/m".into())),
            Some(PathBuf::from("/m/Musique/Onde"))
        );
        assert_eq!(default_music_folder(None, None), None);
    }
}
