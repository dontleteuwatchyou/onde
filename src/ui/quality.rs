use super::*;

impl Onde {
    pub(super) fn inspect_track(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(track) = self.queue.tracks.get(index) else {
            return;
        };
        let Location::File(path) = &track.location else {
            self.message = Some("Vérification disponible pour les fichiers locaux.".into());
            cx.notify();
            return;
        };
        if self.quality_busy {
            return;
        }
        let path = path.clone();
        self.quality_open = true;
        self.quality_busy = true;
        self.quality_report = None;
        self.quality_title = format!("{} — {}", track.artist, track.title);
        self.quality_error = None;
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move { crate::quality::inspect(&path) })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.quality_busy = false;
                match result {
                    Ok(report) => this.quality_report = Some(report),
                    Err(error) => this.quality_error = Some(format!("{error:#}")),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(super) fn render_quality(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let mut panel = div()
            .id("quality-report")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .p_4()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                button("quality-close", "Retour à la bibliothèque").on_click(cx.listener(
                    |this, _, _, cx| {
                        this.quality_open = false;
                        cx.notify();
                    },
                )),
            )
            .child(self.quality_title.clone());
        if self.quality_busy {
            panel = panel.child("Vérification locale et calcul du spectre…");
        }
        if let Some(error) = &self.quality_error {
            panel = panel.child(error.clone());
        }
        if let Some(report) = &self.quality_report {
            panel = panel.child(report.summary.clone())
                .child(format!("Spectre du premier canal · {:.1} s analysées (maximum 300 s) · {} Hz", report.seconds, report.rate))
                .child("Spectre moyen · puissance par bande, échelle −120 à 0 dB relative au plein niveau") ;
            let mut bars = div()
                .flex()
                .items_end()
                .h(px(130.))
                .w_full()
                .bg(rgb(SURFACE))
                .flex_shrink_0();
            for db in &report.spectrum {
                bars = bars.child(div().flex_1().h(px((db + 120.).max(1.))).bg(rgb(ACCENT)));
            }
            panel = panel.child(bars).child(format!("0 Hz                                    Fréquence linéaire                                    {} Hz", report.rate / 2))
                .child("Spectrogramme · temps → · fréquence ↑ · sombre = −120 dB, clair = 0 dB");
            let history = report.history.clone();
            let grid = canvas(
                |_, _, _| {},
                move |bounds, _, window, _| {
                    use gpui::{
                        BorderStyle, Bounds, Corners, Edges, point, quad, size, transparent_black,
                    };
                    let width = f32::from(bounds.size.width) / history.len().max(1) as f32;
                    let height = f32::from(bounds.size.height) / 128.;
                    for (x, column) in history.iter().enumerate() {
                        for (y, db) in column.iter().rev().enumerate() {
                            let level = (((db + 120.) / 120.).clamp(0., 1.) * 255.) as u32;
                            window.paint_quad(quad(
                                Bounds::new(
                                    point(
                                        bounds.origin.x + px(x as f32 * width),
                                        bounds.origin.y + px(y as f32 * height),
                                    ),
                                    size(px(width + 0.2), px(height + 0.2)),
                                ),
                                Corners::default(),
                                rgb((level << 8) | level),
                                Edges::default(),
                                transparent_black(),
                                BorderStyle::default(),
                            ));
                        }
                    }
                },
            )
            .w_full()
            .h(px(128.))
            .flex_shrink_0();
            panel = panel.child(grid).child(format!(
                "Temps : 0 → {:.0} s · fréquence : 0 → {} Hz",
                report.history.len(),
                report.rate / 2
            ));
        }
        panel
    }
}
