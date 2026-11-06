//! The download screen: pick a source, search, download into the music folder.

use crate::telegram::Telegram;
use serde_json::json;
use std::path::PathBuf;

use gpui::KeyDownEvent;

use super::*;
use crate::download::{self, DownloadSource, Hit, Transfer};

enum JobState {
    Waiting,
    Running,
    Done,
    Cancelled,
    Failed(SharedString),
}

struct Job {
    source: Arc<dyn DownloadSource>,
    hit: Hit,
    state: JobState,
    transfer: Transfer,
}

pub(super) struct Downloads {
    pub(super) open: bool,
    telegram: Option<Telegram>,
    telegram_consent: bool,
    telegram_delete: bool,
    telegram_received: usize,
    sources: Vec<Arc<dyn DownloadSource>>,
    selected: usize,
    telegram_selected: bool,
    query: String,
    searching: bool,
    /// Bumped on each search, so a slow answer can't replace a newer one.
    request: u64,
    hits: Vec<Hit>,
    searched: bool,
    error: Option<SharedString>,
    jobs: Vec<Job>,
    pub(super) folder: PathBuf,
}

impl Downloads {
    pub(super) fn new() -> Self {
        let folder = config::load()
            .music_folder
            .or_else(dirs::audio_dir)
            .or_else(|| dirs::home_dir().map(|home| home.join("Musique")))
            .unwrap_or_else(|| PathBuf::from("."));
        Self {
            open: false,
            telegram: None,
            telegram_consent: false,
            telegram_delete: false,
            telegram_received: 0,
            sources: download::sources(),
            selected: 0,
            telegram_selected: false,
            query: String::new(),
            searching: false,
            request: 0,
            hits: Vec::new(),
            searched: false,
            error: None,
            jobs: Vec::new(),
            folder,
        }
    }

    pub(super) fn needs_updates(&self) -> bool {
        self.telegram.is_some() || self.is_running()
    }

    pub(super) fn is_running(&self) -> bool {
        self.jobs
            .iter()
            .any(|job| matches!(job.state, JobState::Running))
    }
}

impl Onde {
    pub(super) fn poll_telegram(&mut self, cx: &mut Context<Self>) {
        let count = self
            .downloads
            .telegram
            .as_ref()
            .map(|bridge| bridge.state.lock().unwrap().files.len())
            .unwrap_or(0);
        if count > self.downloads.telegram_received {
            self.downloads.telegram_received = count;
            self.refresh_library_after_download(cx);
        }
    }

    fn telegram_action(&mut self, action: &str, cx: &mut Context<Self>) {
        let result = (|| -> anyhow::Result<()> {
            if action == "send" && self.downloads.query.trim().is_empty() {
                anyhow::bail!(
                    "Saisir un titre, un artiste ou un lien dans le champ de recherche avant l’envoi."
                );
            }
            if self.downloads.telegram.is_none() {
                self.downloads.telegram = Some(Telegram::start()?);
            }
            let command = if action == "send" {
                let folder = config::load()
                    .music_folder
                    .ok_or_else(|| anyhow::anyhow!("Choisir le dossier musical avant l’envoi"))?;
                json!({"action": action, "text": self.downloads.query, "folder": folder})
            } else {
                json!({"action": action})
            };
            self.downloads.telegram.as_mut().unwrap().send(command)?;
            self.downloads.error = None;
            Ok(())
        })();
        if let Err(error) = result {
            self.downloads.error = Some(error.to_string().into());
        }
        cx.notify();
    }

    fn render_telegram(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self
            .downloads
            .telegram
            .as_ref()
            .map(|bridge| bridge.state.lock().unwrap().clone())
            .unwrap_or_default();
        let mut panel = div().id("telegram-panel").max_h(px(300.)).overflow_y_scroll().flex_shrink_0().flex().flex_col().gap_2().p_3().bg(rgb(SURFACE)).text_sm()
            .child("Telegram · @DeezLoad2Bot · Global search par défaut")
            .child("Session locale donnant accès au compte Telegram. Le bot tiers reçoit vos recherches, liens et informations de profil exposées par Telegram. Conditions du bot et droits du catalogue non vérifiés : n’envoyez que des demandes que vous êtes autorisé à télécharger. Aucun code ni mot de passe 2FA demandé.")
            .child(format!("État : {}", if state.status.is_empty() { "Désactivé" } else { &state.status }));
        if !self.downloads.telegram_consent {
            return panel.child(
                div()
                    .flex()
                    .gap_2()
                    .child(
                        button("tg-accept", "Accepter et connecter par QR").on_click(cx.listener(
                            |this, _, _, cx| {
                                this.downloads.telegram_consent = true;
                                this.telegram_action("connect", cx);
                            },
                        )),
                    )
                    .child(button("tg-refuse", "Refuser").on_click(cx.listener(
                        |this, _, _, cx| {
                            this.downloads.telegram_consent = false;
                            cx.notify();
                        },
                    ))),
            );
        }
        if !state.qr.is_empty() {
            let mut code = div().flex().flex_col().flex_shrink_0();
            for row in &state.qr {
                let mut line = div().flex().h(px(4.)).flex_shrink_0();
                for pixel in row {
                    line =
                        line.child(div().w(px(4.)).h(px(4.)).flex_shrink_0().bg(rgb(if *pixel {
                            0x000000
                        } else {
                            0xffffff
                        })));
                }
                code = code.child(line);
            }
            panel = panel.child(code);
        }
        let mut actions = div().flex().flex_wrap().gap_2();
        if state.connected && !state.busy {
            actions = actions.child(
                button("tg-send", "Autoriser le bot : envoyer et récupérer").on_click(cx.listener(
                    |this, _, _, cx| {
                        this.telegram_action("send", cx);
                    },
                )),
            );
        } else if !state.busy {
            actions = actions.child(
                button("tg-connect", "Connecter")
                    .on_click(cx.listener(|this, _, _, cx| this.telegram_action("connect", cx))),
            );
        }
        for (id, label, action) in [
            ("tg-cancel", "Arrêter la réception", "cancel"),
            ("tg-disconnect", "Déconnecter", "disconnect"),
            ("tg-logout", "Révoquer la session", "logout"),
        ] {
            actions = actions.child(
                button(id, label)
                    .on_click(cx.listener(move |this, _, _, cx| this.telegram_action(action, cx))),
            );
        }
        actions = actions.child(
            button(
                "tg-delete",
                if self.downloads.telegram_delete {
                    "Confirmer la suppression locale"
                } else {
                    "Supprimer la session…"
                },
            )
            .on_click(cx.listener(|this, _, _, cx| {
                if this.downloads.telegram_delete {
                    this.telegram_action("delete", cx);
                    this.downloads.telegram_delete = false;
                } else {
                    this.downloads.telegram_delete = true;
                }
                cx.notify();
            })),
        );
        panel = panel.child(actions);
        for (index, choice) in state.buttons.iter().enumerate() {
            let key = choice.key.clone();
            panel = panel.child(
                div()
                    .id(("tg-menu", index))
                    .px_3()
                    .py_2()
                    .rounded_md()
                    .bg(rgb(HOVER))
                    .cursor_pointer()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(choice.label.clone())
                            .when(!choice.details.is_empty(), |row| {
                                row.child(
                                    div()
                                        .text_sm()
                                        .text_color(rgb(MUTED))
                                        .child(choice.details.clone()),
                                )
                            }),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(bridge) = &mut this.downloads.telegram {
                            if let Err(error) = bridge.send(json!({"action": "click", "key": key}))
                            {
                                this.downloads.error = Some(error.to_string().into());
                            }
                        }
                        cx.notify();
                    })),
            );
        }
        for id in state.candidates.iter().take(10) {
            let id = *id;
            panel = panel.child(
                div()
                    .id(("tg-file", id as usize))
                    .cursor_pointer()
                    .child(format!(
                        "Recevoir le fichier {id} non rattaché à la requête"
                    ))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(bridge) = &mut this.downloads.telegram {
                            if bridge.send(json!({"action": "receive", "id": id})).is_ok() {
                                bridge
                                    .state
                                    .lock()
                                    .unwrap()
                                    .candidates
                                    .retain(|value| *value != id);
                            }
                        }
                        cx.notify();
                    })),
            );
        }
        for file in state.files.iter().rev().take(5) {
            panel = panel.child(format!("Reçu : {file}"));
        }
        panel
    }

    pub(super) fn toggle_downloads(&mut self, cx: &mut Context<Self>) {
        self.downloads.open = !self.downloads.open;
        cx.notify();
    }

    pub(super) fn type_in_search(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        let modifiers = event.keystroke.modifiers;
        if !self.downloads.open || modifiers.control || modifiers.alt || modifiers.platform {
            return;
        }
        if let Some(text) = &event.keystroke.key_char {
            self.downloads
                .query
                .extend(text.chars().filter(|c| !c.is_control()));
            cx.notify();
        }
    }

    pub(super) fn erase_in_search(&mut self, cx: &mut Context<Self>) {
        self.downloads.query.pop();
        cx.notify();
    }

    pub(super) fn paste_in_search(&mut self, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            self.downloads
                .query
                .extend(text.chars().filter(|c| !c.is_control()));
            cx.notify();
        }
    }

    fn select_source(&mut self, index: usize, cx: &mut Context<Self>) {
        self.downloads.telegram_selected = false;
        self.downloads.selected = index;
        self.downloads.hits.clear();
        self.downloads.searched = false;
        self.downloads.error = None;
        self.search(cx);
        cx.notify();
    }

    pub(super) fn search(&mut self, cx: &mut Context<Self>) {
        if self.downloads.telegram_selected {
            let connected = self
                .downloads
                .telegram
                .as_ref()
                .is_some_and(|bridge| bridge.state.lock().unwrap().connected);
            if self.downloads.telegram_consent && connected {
                self.telegram_action("send", cx);
            } else {
                self.downloads.error =
                    Some("Accepter et connecter Telegram avant d’envoyer une recherche.".into());
                cx.notify();
            }
            return;
        }
        let query = self.downloads.query.trim().to_owned();
        self.downloads.request += 1;
        if query.is_empty() {
            self.downloads.searching = false;
            return;
        }
        let request = self.downloads.request;
        let source = self.downloads.sources[self.downloads.selected].clone();
        self.downloads.searching = true;
        self.downloads.error = None;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let found = cx
                .background_spawn(async move { source.search(&query) })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.downloads.request != request {
                    return;
                }
                this.downloads.searching = false;
                this.downloads.searched = true;
                match found {
                    Ok(hits) => this.downloads.hits = hits,
                    Err(error) => {
                        this.downloads.hits.clear();
                        this.downloads.error = Some(format!("{error:#}").into());
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn enqueue(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(hit) = self.downloads.hits.get(index).cloned() else {
            return;
        };
        let source = self.downloads.sources[self.downloads.selected].clone();
        let already_there = self.downloads.jobs.iter().any(|job| {
            job.hit.reference == hit.reference
                && !matches!(job.state, JobState::Failed(_) | JobState::Cancelled)
        });
        if already_there {
            return;
        }
        self.downloads.jobs.push(Job {
            source,
            hit,
            state: JobState::Waiting,
            transfer: Transfer::default(),
        });
        self.start_next_download(cx);
        cx.notify();
    }

    /// One at a time: sites dislike parallel downloads, and so does a home connection.
    fn start_next_download(&mut self, cx: &mut Context<Self>) {
        if self.downloads.is_running() {
            return;
        }
        let folder = self.downloads.folder.clone();
        let Some((index, job)) = self
            .downloads
            .jobs
            .iter_mut()
            .enumerate()
            .find(|(_, job)| matches!(job.state, JobState::Waiting))
        else {
            return;
        };
        job.state = JobState::Running;
        let (source, hit, transfer) = (job.source.clone(), job.hit.clone(), job.transfer.clone());
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    std::fs::create_dir_all(&folder)?;
                    source.download(&hit, &folder, &transfer)
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                let job = &mut this.downloads.jobs[index];
                let finished = result.is_ok();
                job.state = match result {
                    Ok(()) => JobState::Done,
                    Err(_) if job.transfer.is_cancelled() => JobState::Cancelled,
                    Err(error) => JobState::Failed(format!("{error:#}").into()),
                };
                if finished {
                    this.refresh_library_after_download(cx);
                }
                this.start_next_download(cx);
                cx.notify();
            });
        })
        .detach();
    }

    fn cancel_download(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(job) = self.downloads.jobs.get_mut(index) else {
            return;
        };
        match job.state {
            JobState::Waiting => job.state = JobState::Cancelled,
            // The state changes once the download has really stopped.
            JobState::Running => job.transfer.cancel(),
            _ => {}
        }
        cx.notify();
    }

    /// With nothing open, the download folder opens. Otherwise what is open is
    /// listed again, which leaves playback alone: a server may well store its
    /// music in the download folder, and it can't be recognised from here.
    fn refresh_library_after_download(&mut self, cx: &mut Context<Self>) {
        if self.source.is_some() {
            self.refresh_source(cx);
        } else {
            self.open_downloads_folder(cx);
        }
    }

    fn open_downloads_folder(&mut self, cx: &mut Context<Self>) {
        self.open_source(Arc::new(LocalFolder(self.downloads.folder.clone())), cx);
    }

    fn shows_downloads_folder(&self) -> bool {
        self.source
            .as_ref()
            .is_some_and(|source| source.folder() == Some(self.downloads.folder.as_path()))
    }

    pub(super) fn render_downloads(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let state = &self.downloads;
        let source = &state.sources[state.selected];
        let mut tabs = div().flex().gap_2();
        for (index, choice) in state.sources.iter().enumerate() {
            let selected = !state.telegram_selected && index == state.selected;
            tabs = tabs.child(
                div()
                    .id(("source", index))
                    .px_3()
                    .py_1()
                    .rounded_md()
                    .text_sm()
                    .cursor_pointer()
                    .bg(rgb(if selected { SELECTED } else { HOVER }))
                    .text_color(rgb(if selected { ACCENT } else { TEXT }))
                    .child(choice.name())
                    .on_click(cx.listener(move |this, _, _, cx| this.select_source(index, cx))),
            );
        }
        tabs = tabs.child(
            div()
                .id("source-telegram")
                .px_3()
                .py_1()
                .rounded_md()
                .text_sm()
                .cursor_pointer()
                .bg(rgb(if state.telegram_selected {
                    SELECTED
                } else {
                    HOVER
                }))
                .text_color(rgb(if state.telegram_selected {
                    ACCENT
                } else {
                    TEXT
                }))
                .child("Telegram")
                .on_click(cx.listener(|this, _, _, cx| {
                    this.downloads.telegram_selected = true;
                    this.downloads.request += 1;
                    this.downloads.searching = false;
                    this.downloads.hits.clear();
                    this.downloads.searched = false;
                    this.downloads.error = None;
                    cx.notify();
                })),
        );
        let field = div()
            .px_3()
            .py_2()
            .rounded_md()
            .bg(rgb(SURFACE))
            .border_1()
            .border_color(rgb(ACCENT))
            .when(state.query.is_empty(), |field| {
                field
                    .text_color(rgb(MUTED))
                    .child("Tape un titre ou un artiste, puis Entrée")
            })
            .when(!state.query.is_empty(), |field| {
                field.child(format!("{}▏", state.query))
            });

        let mut rows = div().flex().flex_col();
        for (index, hit) in state.hits.iter().enumerate() {
            let time = hit.seconds.map(format_time).unwrap_or_default();
            rows = rows.child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .w_full()
                    .px_2()
                    .py_1()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .child(hit.title.clone()),
                    )
                    .child(
                        div()
                            .w(relative(0.25))
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_sm()
                            .text_color(rgb(MUTED))
                            .child(hit.artist.clone()),
                    )
                    .child(
                        div()
                            .w(px(56.))
                            .flex()
                            .justify_end()
                            .text_sm()
                            .text_color(rgb(MUTED))
                            .child(time),
                    )
                    .child(
                        div()
                            .id(("get", index))
                            .px_3()
                            .py_1()
                            .rounded_md()
                            .bg(rgb(HOVER))
                            .text_sm()
                            .cursor_pointer()
                            .hover(|button| button.bg(rgb(SELECTED)))
                            .child("Télécharger")
                            .on_click(cx.listener(move |this, _, _, cx| this.enqueue(index, cx))),
                    ),
            );
        }

        let status = if state.searching {
            Some("Recherche…".to_owned())
        } else if state.searched && state.hits.is_empty() && state.error.is_none() {
            Some("Aucun résultat.".to_owned())
        } else {
            None
        };

        let mut jobs = div().flex().flex_col().gap_1();
        for (index, job) in state.jobs.iter().enumerate().rev().take(5) {
            let label = match &job.state {
                JobState::Waiting => "En attente".to_owned(),
                JobState::Running if job.transfer.is_cancelled() => "Annulation…".to_owned(),
                JobState::Running => format!("{:.0} %", job.transfer.progress() * 100.0),
                JobState::Done => "Terminé".to_owned(),
                JobState::Cancelled => "Annulé".to_owned(),
                JobState::Failed(reason) => format!("Échec : {reason}"),
            };
            let failed = matches!(job.state, JobState::Failed(_));
            let cancellable = matches!(job.state, JobState::Waiting | JobState::Running)
                && !job.transfer.is_cancelled();
            jobs = jobs.child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_3()
                    .text_sm()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .child(job.hit.title.clone()),
                    )
                    .child(
                        div()
                            .text_color(rgb(if failed { ERROR } else { MUTED }))
                            .child(label),
                    )
                    .when(cancellable, |row| {
                        row.child(
                            div()
                                .id(("cancel", index))
                                .px_2()
                                .rounded_md()
                                .bg(rgb(HOVER))
                                .cursor_pointer()
                                .hover(|button| button.bg(rgb(SELECTED)))
                                .child("Annuler")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.cancel_download(index, cx)
                                })),
                        )
                    }),
            );
        }

        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .gap_3()
            .px_5()
            .pb_3()
            .child(tabs)
            .when(state.telegram_selected, |panel| panel.child(self.render_telegram(cx)))
            .child(
                div()
                    .text_sm()
                    .text_color(rgb(MUTED))
                    .child(if state.telegram_selected { "Telegram : Entrée lance Global search. Choisis un résultat : réception automatique dans le dossier musical.".to_owned() } else { format!("Qualité : {}", source.quality()) }),
            )
            .child(field)
            .when_some(state.error.clone(), |panel, error| {
                panel.child(div().text_sm().text_color(rgb(ERROR)).child(error))
            })
            .when_some(status, |panel, status| {
                panel.child(div().text_sm().text_color(rgb(MUTED)).child(status))
            })
            .child(
                div()
                    .id("hits")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(rows),
            )
            .when(!state.jobs.is_empty(), |panel| panel.child(jobs))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_3()
                    .text_sm()
                    .text_color(rgb(MUTED))
                    .child(format!("Dossier : {}", state.folder.display()))
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .when(!self.shows_downloads_folder(), |actions| {
                                actions.child(button("open-downloads", "Ouvrir").on_click(
                                    cx.listener(|this, _, _, cx| this.open_downloads_folder(cx)),
                                ))
                            })
                            .child(button("download-folder", "Changer").on_click(
                                cx.listener(|this, _, _, cx| this.pick_music_folder(cx)),
                            )),
                    ),
            )
    }
}
