//! Telling about new versions of Onde, and installing them when the user says so.

use gpui::Div;

use super::widgets::{btn, chip, h2, mono};
use super::*;
use crate::update::{self, Available};

const FIRST_CHECK: Duration = Duration::from_secs(15);
const NEXT_CHECK: Duration = Duration::from_secs(6 * 3600);

#[derive(Default)]
pub(super) enum UpdateState {
    #[default]
    Idle,
    Checking,
    UpToDate,
    Available(Available),
    Installing(String),
    Failed(String),
}

impl Onde {
    pub(super) fn start_update_checks(&self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(FIRST_CHECK).await;
            loop {
                if !config::load().no_update_check {
                    let found = cx.background_spawn(async { update::check() }).await;
                    // Failing quietly: no network is no reason to bother anyone.
                    let told = this.update(cx, |this, cx| {
                        if let (Ok(Some(found)), false) =
                            (found, matches!(this.update, UpdateState::Installing(_)))
                        {
                            this.update = UpdateState::Available(found);
                            cx.notify();
                        }
                    });
                    if told.is_err() {
                        return;
                    }
                }
                cx.background_executor().timer(NEXT_CHECK).await;
            }
        })
        .detach();
    }

    fn check_for_update(&mut self, cx: &mut Context<Self>) {
        self.update = UpdateState::Checking;
        self.update_dismissed = false;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let found = cx.background_spawn(async { update::check() }).await;
            let _ = this.update(cx, |this, cx| {
                this.update = match found {
                    Ok(Some(found)) => UpdateState::Available(found),
                    Ok(None) => UpdateState::UpToDate,
                    Err(error) => UpdateState::Failed(format!("{error:#}")),
                };
                cx.notify();
            });
        })
        .detach();
    }

    fn install_update(&mut self, cx: &mut Context<Self>) {
        let UpdateState::Available(found) = &self.update else {
            return;
        };
        let found = found.clone();
        self.update_dismissed = false;
        if self.downloads.is_running() {
            self.update =
                UpdateState::Failed("Un téléchargement est en cours : réessaie après.".to_owned());
            cx.notify();
            return;
        }
        self.update = UpdateState::Installing(found.version.clone());
        cx.notify();
        cx.spawn(async move |this, cx| {
            let installed = cx
                .background_spawn(async move { update::install(&found) })
                .await;
            let _ = this.update(cx, |this, cx| match installed {
                Ok(exe) => this.restart_into(&exe, cx),
                Err(error) => {
                    this.update = UpdateState::Failed(format!("{error:#}"));
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn restart_into(&mut self, exe: &std::path::Path, cx: &mut Context<Self>) {
        // The next launch needs the Telegram session this one still holds.
        self.stop_telegram();
        match std::process::Command::new(exe).spawn() {
            Ok(_) => cx.quit(),
            Err(_) => {
                self.update = UpdateState::Failed(
                    "Mise à jour installée : relance Onde pour en profiter.".to_owned(),
                );
                cx.notify();
            }
        }
    }

    fn toggle_update_check(&mut self, cx: &mut Context<Self>) {
        let mut settings = config::load();
        settings.no_update_check = !settings.no_update_check;
        if let Err(error) = config::save(&settings) {
            self.message = Some(format!("Réglage non enregistré : {error:#}").into());
        }
        cx.notify();
    }

    pub(super) fn render_update_banner(&self, cx: &mut Context<Self>) -> Option<Div> {
        if self.update_dismissed {
            return None;
        }
        let palette = pal();
        let row = div()
            .mx_8()
            .mb_2()
            .px_4()
            .py_2()
            .rounded(px(12.))
            .bg(palette.rise)
            .flex()
            .items_center()
            .gap_3()
            .text_size(px(13.));
        let later = |id: &'static str, cx: &mut Context<Self>| {
            btn(id, "Plus tard").on_click(cx.listener(|this, _, _, cx| {
                this.update_dismissed = true;
                cx.notify();
            }))
        };
        match &self.update {
            UpdateState::Available(found) => Some(
                row.child(
                    div()
                        .flex_1()
                        .child(format!("Onde {} est disponible.", found.version)),
                )
                .child(
                    btn("update-install", "Installer et redémarrer")
                        .on_click(cx.listener(|this, _, _, cx| this.install_update(cx))),
                )
                .child(later("update-later", cx)),
            ),
            UpdateState::Installing(version) => {
                Some(row.child(format!("Installation d’Onde {version}… La musique s’arrêtera au redémarrage.")))
            }
            UpdateState::Failed(reason) => Some(
                row.child(div().flex_1().text_color(ERROR).child(reason.clone()))
                    .child(later("update-close", cx)),
            ),
            _ => None,
        }
    }

    pub(super) fn render_updates(&self, cx: &mut Context<Self>) -> Div {
        let palette = pal();
        let automatic = !config::load().no_update_check;
        let status = match &self.update {
            UpdateState::Idle => "Pas encore vérifié".to_owned(),
            UpdateState::Checking => "Vérification…".to_owned(),
            UpdateState::UpToDate => "Onde est à jour".to_owned(),
            UpdateState::Available(found) => format!("Onde {} est disponible", found.version),
            UpdateState::Installing(version) => format!("Installation d’Onde {version}…"),
            UpdateState::Failed(reason) => reason.clone(),
        };
        let mut actions = div().flex().gap_2();
        match self.update {
            UpdateState::Available(_) => {
                actions = actions.child(
                    btn("settings-update-install", "Installer et redémarrer")
                        .on_click(cx.listener(|this, _, _, cx| this.install_update(cx))),
                );
            }
            UpdateState::Checking | UpdateState::Installing(_) => {}
            _ => {
                actions = actions.child(
                    btn("settings-update-check", "Vérifier maintenant")
                        .on_click(cx.listener(|this, _, _, cx| this.check_for_update(cx))),
                );
            }
        }
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(h2("Mises à jour"))
                    .child(
                        chip(
                            "update-auto",
                            if automatic { "Automatique" } else { "Coupée" },
                            automatic,
                        )
                        .on_click(cx.listener(|this, _, _, cx| this.toggle_update_check(cx))),
                    )
                    .child(mono(format!("Version {}", update::CURRENT)).text_color(palette.tx3)),
            )
            .child(
                div()
                    .text_size(px(13.))
                    .text_color(palette.tx2)
                    .child(format!("{status}. Onde interroge GitHub au lancement puis toutes les 6 heures, et n’installe rien sans ton accord. Les versions sont signées : une version sans signature valide est refusée.")),
            )
            .child(actions)
    }
}
