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
        self.ask_folder = false;
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
        let new_folder = default_music_folder(dirs::audio_dir(), dirs::home_dir());
        let palette = pal();
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap_4()
            .px_8()
            .child(display("Où ranger ta musique ?", 48.))
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
                        row.child(btn("setup-create", "Créer un dossier Onde").on_click(
                            cx.listener(move |this, _, _, cx| {
                                this.create_music_folder(folder.clone(), cx);
                            }),
                        ))
                    })
                    .child(ghost("setup-later", "Plus tard").on_click(cx.listener(
                        |this, _, _, cx| {
                            this.ask_folder = false;
                            cx.notify();
                        },
                    ))),
            )
            .when_some(new_folder, |page, folder| {
                page.child(
                    mono(format!("Le dossier créé sera : {}", folder.display()))
                        .text_color(palette.tx3),
                )
            })
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
