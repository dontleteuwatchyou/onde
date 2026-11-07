//! The download screen: pick a source, search, download into the music folder.

use crate::telegram::Telegram;
use serde_json::json;
use std::path::PathBuf;

use gpui::{ElementId, FontWeight, KeyDownEvent, rgb};

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
    telegram: Option<Telegram>,
    telegram_consent: bool,
    telegram_delete: bool,
    telegram_received: usize,
    /// Results ticked for a group download, by bridge key.
    telegram_picked: Vec<String>,
    /// Last queue progress seen, to repaint every page when it moves.
    telegram_batch: crate::telegram::Batch,
    pub(super) telegram_batch_hidden: bool,
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
        // Once the user accepted Telegram (a session exists), the bridge lives as long as
        // the app. Without a session nothing starts, so Python stays optional.
        let session = Telegram::has_session();
        let mut startup_error = None;
        let mut telegram = None;
        if session {
            match Telegram::start().and_then(|mut bridge| {
                bridge.send(json!({"action": "connect"}))?;
                Ok(bridge)
            }) {
                Ok(bridge) => telegram = Some(bridge),
                Err(error) => startup_error = Some(format!("{error:#}").into()),
            }
        }
        Self {
            telegram,
            telegram_consent: session,
            telegram_delete: false,
            telegram_received: 0,
            telegram_picked: Vec::new(),
            telegram_batch: Default::default(),
            telegram_batch_hidden: false,
            sources: download::sources(),
            selected: 0,
            telegram_selected: false,
            query: String::new(),
            searching: false,
            request: 0,
            hits: Vec::new(),
            searched: false,
            error: startup_error,
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
    pub(super) fn stop_telegram(&mut self) {
        if let Some(bridge) = &mut self.downloads.telegram {
            bridge.stop();
        }
        self.downloads.telegram = None;
    }

    pub(super) fn poll_telegram(&mut self, cx: &mut Context<Self>) {
        let (count, batch) = self
            .downloads
            .telegram
            .as_ref()
            .map(|bridge| {
                let state = bridge.state.lock().unwrap();
                (state.files.len(), state.batch.clone())
            })
            .unwrap_or_default();
        if batch != self.downloads.telegram_batch {
            if batch.active && !self.downloads.telegram_batch.active {
                self.downloads.telegram_batch_hidden = false;
            }
            self.downloads.telegram_batch = batch;
            cx.notify();
        }
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
            let alive = self
                .downloads
                .telegram
                .as_mut()
                .is_some_and(|bridge| bridge.is_alive());
            if !alive {
                self.downloads.telegram = Some(Telegram::start()?);
                self.downloads.telegram_received = 0;
                if action == "send" {
                    // A new bridge has no connection yet and would refuse the request.
                    let bridge = self.downloads.telegram.as_mut().unwrap();
                    bridge.send(json!({"action": "connect"}))?;
                    anyhow::bail!(
                        "Service Telegram relancé : relancer la recherche une fois connecté."
                    );
                }
            }
            let command = if action == "send" {
                let folder = config::load()
                    .music_folder
                    .ok_or_else(|| anyhow::anyhow!("Choisir le dossier musical avant l’envoi"))?;
                json!({"action": action, "text": self.downloads.query, "folder": folder})
            } else {
                json!({"action": action})
            };
            if action == "send" {
                self.downloads.telegram_picked.clear();
            }
            self.downloads.telegram.as_mut().unwrap().send(command)?;
            self.downloads.error = None;
            Ok(())
        })();
        if let Err(error) = result {
            self.downloads.error = Some(error.to_string().into());
        }
        cx.notify();
    }

    fn telegram_queue(&mut self, keys: Vec<String>, cx: &mut Context<Self>) {
        if let Some(bridge) = &mut self.downloads.telegram
            && let Err(error) = bridge.send(json!({"action": "queue", "keys": keys}))
        {
            self.downloads.error = Some(error.to_string().into());
        }
        cx.notify();
    }

    pub(super) fn stop_telegram_queue(&mut self, cx: &mut Context<Self>) {
        self.telegram_action("cancel", cx);
    }

    /// Queue progress under the top bar, on every page, until dismissed.
    pub(super) fn render_telegram_banner(&self, cx: &mut Context<Self>) -> Option<gpui::Div> {
        let batch = &self.downloads.telegram_batch;
        if batch.total == 0 || self.downloads.telegram_batch_hidden {
            return None;
        }
        let palette = pal();
        let failed = batch.failed.len();
        let finished = batch.done + failed;
        let fraction = (finished as f32 / batch.total.max(1) as f32).clamp(0., 1.);
        let mut summary = format!("Telegram · {} : {} / {} reçus", batch.name, batch.done, batch.total);
        if batch.present > 0 {
            summary.push_str(&format!(" · {} déjà présents", batch.present));
        }
        if failed > 0 {
            summary.push_str(&format!(" · {failed} en échec"));
        }
        let detail = if batch.active {
            if batch.current.is_empty() {
                "En attente du bot…".to_owned()
            } else {
                batch.current.clone()
            }
        } else if failed > 0 {
            format!("Non trouvés : {}", batch.failed.join(" · "))
        } else {
            "Tout est dans la bibliothèque.".to_owned()
        };
        Some(
            div()
                .mx_8()
                .mb_2()
                .px_4()
                .py_2()
                .rounded(px(12.))
                .bg(palette.rise)
                .flex()
                .flex_col()
                .gap_1()
                .text_size(px(13.))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_3()
                        .child(div().flex_1().min_w_0().truncate().child(summary))
                        .when(self.screen != Screen::Downloads, |row| {
                            row.child(ghost("tg-batch-open", "Voir").on_click(cx.listener(
                                |this, _, _, cx| this.go(Screen::Downloads, cx),
                            )))
                        })
                        .child(if batch.active {
                            btn("tg-batch-stop", "Arrêter")
                                .on_click(cx.listener(|this, _, _, cx| this.stop_telegram_queue(cx)))
                        } else {
                            btn("tg-batch-close", "Fermer").on_click(cx.listener(|this, _, _, cx| {
                                this.downloads.telegram_batch_hidden = true;
                                cx.notify();
                            }))
                        }),
                )
                .child(
                    div()
                        .h(px(4.))
                        .w_full()
                        .rounded(px(2.))
                        .bg(palette.rise2)
                        .child(
                            div()
                                .h_full()
                                .w(relative(fraction))
                                .rounded(px(2.))
                                .bg(if failed > 0 && !batch.active { ERROR } else { palette.spark }),
                        ),
                )
                .child(
                    div()
                        .truncate()
                        .text_size(px(12.))
                        .text_color(palette.tx2)
                        .child(detail),
                ),
        )
    }

    fn render_telegram(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let state = self
            .downloads
            .telegram
            .as_ref()
            .map(|bridge| bridge.state.lock().unwrap().clone())
            .unwrap_or_default();
        let mut panel = div().id("telegram-panel").max_h(px(440.)).overflow_y_scroll().flex_shrink_0().flex().flex_col().gap_2().p_3().bg(pal().deep).text_sm()
            .child("Telegram · @DeezLoad2Bot")
            .child(format!("État : {}", if state.status.is_empty() { "Désactivé" } else { &state.status }));
        if !self.downloads.telegram_consent {
            return panel
                .child("Session locale donnant accès au compte Telegram. Le bot tiers reçoit vos recherches, liens et informations de profil exposées par Telegram. Conditions du bot et droits du catalogue non vérifiés : n’envoyez que des demandes que vous êtes autorisé à télécharger. Aucun code ni mot de passe 2FA demandé.")
                .child(
                div()
                    .flex()
                    .gap_2()
                    .child(
                        btn("tg-accept", "Accepter et connecter par QR").on_click(cx.listener(
                            |this, _, _, cx| {
                                this.downloads.telegram_consent = true;
                                this.telegram_action("connect", cx);
                            },
                        )),
                    )
                    .child(btn("tg-refuse", "Refuser").on_click(cx.listener(
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
        if !state.connected && !state.busy {
            actions = actions.child(
                btn("tg-connect", "Connecter")
                    .on_click(cx.listener(|this, _, _, cx| this.telegram_action("connect", cx))),
            );
        }
        for (id, label, action) in [
            ("tg-cancel", "Arrêter la réception", "cancel"),
            ("tg-disconnect", "Déconnecter", "disconnect"),
            ("tg-logout", "Révoquer la session", "logout"),
        ] {
            actions = actions.child(
                btn(id, label)
                    .on_click(cx.listener(move |this, _, _, cx| this.telegram_action(action, cx))),
            );
        }
        actions = actions.child(
            btn(
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
        for file in state.files.iter().rev().take(3) {
            let name = std::path::Path::new(file).file_name().map_or(file.as_str(), |name| name.to_str().unwrap_or(file));
            panel = panel.child(format!("Reçu : {name}"));
        }
        let picked: Vec<String> = state
            .buttons
            .iter()
            .filter(|choice| self.downloads.telegram_picked.contains(&choice.key))
            .map(|choice| choice.key.clone())
            .collect();
        let selectable = state.buttons.iter().any(|choice| choice.key.starts_with("inline:"));
        if selectable {
            let all: Vec<String> = state.buttons.iter().map(|choice| choice.key.clone()).collect();
            let count = picked.len();
            panel = panel.child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .text_color(pal().tx2)
                            .child("Clique sur un titre pour le télécharger, ou coche-en plusieurs."),
                    )
                    .when(count > 0, |bar| {
                        bar.child(
                            btn("tg-queue", format!("Télécharger la sélection ({count})")).on_click(
                                cx.listener(move |this, _, _, cx| {
                                    let keys = std::mem::take(&mut this.downloads.telegram_picked);
                                    this.telegram_queue(keys, cx);
                                }),
                            ),
                        )
                        .child(ghost("tg-unpick", "Vider").on_click(cx.listener(
                            |this, _, _, cx| {
                                this.downloads.telegram_picked.clear();
                                cx.notify();
                            },
                        )))
                    })
                    .when(count < all.len(), |bar| {
                        bar.child(ghost("tg-pick-all", "Tout cocher").on_click(cx.listener(
                            move |this, _, _, cx| {
                                this.downloads.telegram_picked = all.clone();
                                cx.notify();
                            },
                        )))
                    }),
            );
            if count > 1 {
                panel = panel.child(
                    div().text_color(pal().tx3).child(
                        "Le bot prépare les morceaux un par un (trois à la fois) : ça peut prendre quelques minutes. Tu peux changer de page, le suivi reste affiché en haut.",
                    ),
                );
            }
        }
        for (index, choice) in state.buttons.iter().enumerate() {
            let key = choice.key.clone();
            let pick_key = key.clone();
            let ticked = picked.contains(&key);
            let label = div()
                .id(("tg-menu", index))
                .flex_1()
                .min_w_0()
                .cursor_pointer()
                .flex()
                .flex_col()
                .gap_1()
                .child(choice.label.clone())
                .when(!choice.details.is_empty(), |row| {
                    row.child(
                        div()
                            .text_sm()
                            .text_color(pal().tx2)
                            .child(choice.details.clone()),
                    )
                })
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(bridge) = &mut this.downloads.telegram {
                        if let Err(error) = bridge.send(json!({"action": "click", "key": key})) {
                            this.downloads.error = Some(error.to_string().into());
                        }
                    }
                    cx.notify();
                }));
            let row = div()
                .flex()
                .items_center()
                .gap_3()
                .px_3()
                .py_2()
                .rounded_md()
                .bg(if ticked { pal().rise } else { pal().rise2 })
                .when(selectable, |row| {
                    row.child(
                        div()
                            .id(("tg-pick", index))
                            .flex_none()
                            .size(px(20.))
                            .rounded(px(5.))
                            .border_1()
                            .border_color(if ticked { pal().spark } else { pal().tx3 })
                            .when(ticked, |tick| tick.bg(pal().spark))
                            .cursor_pointer()
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_size(px(13.))
                            .text_color(pal().deep)
                            .when(ticked, |tick| tick.child("✓"))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                let picked = &mut this.downloads.telegram_picked;
                                if let Some(at) = picked.iter().position(|key| *key == pick_key) {
                                    picked.remove(at);
                                } else {
                                    picked.push(pick_key.clone());
                                }
                                cx.notify();
                            })),
                    )
                })
                .child(label);
            panel = panel.child(row);
        }
        panel
    }

    pub(super) fn toggle_downloads(&mut self, cx: &mut Context<Self>) {
        let target = if self.screen == Screen::Downloads {
            Screen::Home
        } else {
            Screen::Downloads
        };
        self.go(target, cx);
    }

    fn search_text(&mut self) -> Option<&mut String> {
        if self.naming.is_some() {
            self.naming.as_mut()
        } else if self.filling.is_some() {
            Some(&mut self.fill_query)
        } else if self.screen == Screen::Downloads {
            Some(&mut self.downloads.query)
        } else if self.library_typing {
            Some(&mut self.library_query)
        } else {
            None
        }
    }

    fn search_text_changed(&mut self, cx: &mut Context<Self>) {
        if self.naming.is_none() && self.filling.is_none() && self.screen != Screen::Downloads {
            self.refilter();
        }
        cx.notify();
    }

    pub(super) fn type_in_search(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        let modifiers = event.keystroke.modifiers;
        if modifiers.control || modifiers.alt || modifiers.platform {
            return;
        }
        let Some(text) = event.keystroke.key_char.clone() else {
            return;
        };
        if let Some(query) = self.search_text() {
            query.extend(text.chars().filter(|c| !c.is_control()));
            self.search_text_changed(cx);
        }
    }

    pub(super) fn erase_in_search(&mut self, cx: &mut Context<Self>) {
        if let Some(query) = self.search_text() {
            query.pop();
            self.search_text_changed(cx);
        }
    }

    pub(super) fn paste_in_search(&mut self, cx: &mut Context<Self>) {
        let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else {
            return;
        };
        if let Some(query) = self.search_text() {
            query.extend(text.chars().filter(|c| !c.is_control()));
            self.search_text_changed(cx);
        }
    }

    pub(super) fn submit_search(&mut self, cx: &mut Context<Self>) {
        if self.naming.is_some() {
            self.confirm_naming(cx);
        } else if self.filling.is_some() {
            self.add_first_match(cx);
        } else if self.screen == Screen::Downloads {
            self.search(cx);
        } else {
            self.library_typing = false;
            cx.notify();
        }
    }

    pub(super) fn leave_search(&mut self, cx: &mut Context<Self>) {
        if self.naming.is_some() {
            self.naming = None;
            cx.notify();
        } else if self.filling.is_some() {
            self.filling = None;
            cx.notify();
        } else if self.screen == Screen::Downloads {
            self.go(Screen::Home, cx);
        } else {
            self.library_typing = false;
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
            // A stopped bridge is restarted by telegram_action instead of being a dead end.
            let usable = self.downloads.telegram.as_mut().is_none_or(|bridge| {
                !bridge.is_alive() || bridge.state.lock().unwrap().connected
            });
            if self.downloads.telegram_consent && usable {
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

    pub(super) fn render_downloads(
        &mut self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let palette = pal();
        let narrow = window.viewport_size().width < px(COMPACT_WIDTH + 200.0);
        let state = &self.downloads;
        let source = &state.sources[state.selected];

        let row = |id: ElementId, label: String, detail: String, selected: bool| {
            div()
                .id(id)
                .flex()
                .flex_col()
                .px_3()
                .py_2()
                .rounded(px(10.))
                .cursor_pointer()
                .when(selected, |row| row.bg(palette.rise2))
                .when(!selected, |row| row.hover(|row| row.bg(pal().rise)))
                .child(div().font_weight(FontWeight::MEDIUM).child(label))
                .when(!detail.is_empty(), |row| {
                    row.child(mono(detail).text_color(palette.tx3))
                })
        };
        let mut sources = div().flex().flex_col().gap_1();
        for (index, choice) in state.sources.iter().enumerate() {
            let selected = !state.telegram_selected && index == state.selected;
            sources = sources.child(
                row(
                    ("source", index).into(),
                    choice.name().to_owned(),
                    choice.quality().to_owned(),
                    selected,
                )
                .on_click(cx.listener(move |this, _, _, cx| this.select_source(index, cx))),
            );
        }
        sources = sources.child(
            row(
                "source-telegram".into(),
                "Telegram".to_owned(),
                String::new(),
                state.telegram_selected,
            )
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
        let folder_panel = div()
            .flex()
            .flex_col()
            .gap_2()
            .pt_4()
            .child(mono("DOSSIER").text_color(palette.tx3))
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(palette.tx2)
                    .child(state.folder.display().to_string()),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_2()
                    .when(!self.shows_downloads_folder(), |actions| {
                        actions.child(ghost("open-downloads", "Ouvrir").on_click(
                            cx.listener(|this, _, _, cx| this.open_downloads_folder(cx)),
                        ))
                    })
                    .child(ghost("download-folder", "Changer").on_click(
                        cx.listener(|this, _, _, cx| this.pick_music_folder(cx)),
                    )),
            );

        let field = div()
            .flex()
            .flex_none()
            .items_center()
            .gap_2()
            .h(px(44.))
            .px_4()
            .rounded(px(10.))
            .bg(palette.rise)
            .border_1()
            .border_color(palette.tide)
            .cursor_text()
            .id("download-search")
            .on_click(cx.listener(|this, _, window, cx| {
                window.focus(&this.focus);
                cx.notify();
            }))
            .child(icon("search", 18., palette.tx2))
            .child(
                div()
                    .flex()
                    .min_w_0()
                    .child(div().truncate().child(format!("{}▏", state.query)))
                    .when(state.query.is_empty(), |line| {
                        line.child(
                            div()
                                .text_color(palette.tx3)
                                .child(if state.telegram_selected {
                                    "Titre, artiste ou lien de playlist Spotify, Deezer, SoundCloud"
                                } else {
                                    "Tape un titre ou un artiste, puis Entrée"
                                }),
                        )
                    }),
            );

        let mut rows = div().flex().flex_col();
        for (index, hit) in state.hits.iter().enumerate() {
            rows = rows.child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .w_full()
                    .h(px(48.))
                    .px_2()
                    .rounded(px(10.))
                    .hover(|row| row.bg(pal().rise))
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
                                    .child(hit.title.clone()),
                            )
                            .child(
                                div()
                                    .truncate()
                                    .text_size(px(12.))
                                    .text_color(palette.tx2)
                                    .child(hit.artist.clone()),
                            ),
                    )
                    .child(
                        mono(hit.seconds.map(format_time).unwrap_or_default())
                            .text_color(palette.tx3),
                    )
                    .child(
                        btn(("get", index), "Télécharger")
                            .on_click(cx.listener(move |this, _, _, cx| this.enqueue(index, cx))),
                    ),
            );
        }

        let status = if state.searching {
            Some("Recherche…")
        } else if state.searched && state.hits.is_empty() && state.error.is_none() {
            Some("Aucun résultat.")
        } else {
            None
        };

        let mut jobs = div().flex().flex_col().gap_3();
        for (index, job) in state.jobs.iter().enumerate().rev().take(30) {
            let (label, tint) = match &job.state {
                JobState::Waiting => ("En attente".to_owned(), palette.tx3),
                JobState::Running if job.transfer.is_cancelled() => {
                    ("Annulation…".to_owned(), palette.tx3)
                }
                JobState::Running => (
                    format!("{:.0} %", job.transfer.progress() * 100.0),
                    palette.spark,
                ),
                JobState::Done => ("Terminé".to_owned(), GOOD),
                JobState::Cancelled => ("Annulé".to_owned(), palette.tx3),
                JobState::Failed(reason) => (format!("Échec : {reason}"), ERROR),
            };
            let fraction = match job.state {
                JobState::Running => job.transfer.progress(),
                JobState::Done | JobState::Failed(_) => 1.0,
                _ => 0.0,
            };
            let cancellable = matches!(job.state, JobState::Waiting | JobState::Running)
                && !job.transfer.is_cancelled();
            jobs = jobs.child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_size(px(13.))
                                    .child(job.hit.title.clone()),
                            )
                            .when(cancellable, |row| {
                                row.child(
                                    icon_small(("cancel", index), "close").on_click(cx.listener(
                                        move |this, _, _, cx| this.cancel_download(index, cx),
                                    )),
                                )
                            }),
                    )
                    .child(
                        div()
                            .h(px(4.))
                            .w_full()
                            .rounded(px(2.))
                            .bg(palette.rise2)
                            .child(
                                div()
                                    .h_full()
                                    .rounded(px(2.))
                                    .w(relative(fraction as f32))
                                    .bg(tint),
                            ),
                    )
                    .child(mono(label).text_color(tint)),
            );
        }

        let results = div()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .flex()
            .flex_col()
            .gap_3()
            .when(state.telegram_selected, |panel| {
                panel.child(self.render_telegram(cx))
            })
            .child(field)
            .when_some(state.error.clone(), |panel, error| {
                panel.child(div().text_size(px(13.)).text_color(ERROR).child(error))
            })
            .when_some(status, |panel, status| {
                panel.child(div().text_size(px(13.)).text_color(palette.tx2).child(status))
            })
            .child(
                div()
                    .id("hits")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(rows),
            );
        let quality = if state.telegram_selected {
            "Telegram : Entrée affiche les résultats. Un clic sur une musique la télécharge dans le dossier musical.".to_owned()
        } else {
            format!("Qualité : {}", source.quality())
        };

        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .gap_4()
            .px_8()
            .pt_2()
            .pb(px(40.))
            .child(
                div()
                    .flex()
                    .items_end()
                    .gap_4()
                    .child(display("Téléchargements", 40.))
                    .child(
                        mono(quality)
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .pb_2()
                            .text_color(palette.tx3),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .gap_6()
                    .child(
                        div()
                            .w(px(if narrow { 180. } else { 232. }))
                            .flex_none()
                            .flex()
                            .flex_col()
                            .child(sources)
                            .child(folder_panel),
                    )
                    .child(results)
                    .when(!narrow, |page| {
                        page.child(
                            div()
                                .id("jobs")
                                .w(px(320.))
                                .flex_none()
                                .p_4()
                                .rounded(px(16.))
                                .bg(palette.rise)
                                .overflow_y_scroll()
                                .flex()
                                .flex_col()
                                .gap_3()
                                .child(h2("En cours"))
                                .when(state.jobs.is_empty(), |panel| {
                                    panel.child(
                                        div()
                                            .text_size(px(13.))
                                            .text_color(palette.tx2)
                                            .child("Ce que tu télécharges s’affiche ici."),
                                    )
                                })
                                .child(jobs),
                        )
                    }),
            )
    }
}
