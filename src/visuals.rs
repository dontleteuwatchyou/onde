//! The background: a glow that breathes with the bass, bars that follow the spectrum.

use gpui::{
    BorderStyle, Bounds, BoxShadow, Corners, Edges, Hsla, Pixels, Window, point, px, quad, rgb,
    size, transparent_black,
};

use crate::analysis::BANDS;

const ACCENT: u32 = 0x3fd0c0;
/// Kept low so text on top stays readable.
const GLOW_ALPHA: f32 = 0.6;
const BAR_MIN_ALPHA: f32 = 0.06;
const BAR_MAX_ALPHA: f32 = 0.34;
const BAR_MAX_HEIGHT: f32 = 0.42;
const BAR_GAP: f32 = 0.35;
const LEVEL_VISIBLE: f32 = 0.01;

fn accent(alpha: f32) -> Hsla {
    let mut color: Hsla = rgb(ACCENT).into();
    color.a = alpha;
    color
}

/// Bars mirror around the center, bass in the middle. Returns their left edge
/// as a fraction of the width, from the center outwards, on both sides.
fn bar_positions(band: usize) -> (f32, f32) {
    let step = 0.5 / BANDS as f32;
    let offset = band as f32 * step;
    (0.5 + offset, (0.5 - offset - step).max(0.0))
}

pub fn paint(bounds: Bounds<Pixels>, levels: &[f32; BANDS], bass: f32, window: &mut Window) {
    if levels.iter().all(|level| *level < LEVEL_VISIBLE) {
        return;
    }
    let (width, height) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
    paint_glow(bounds, width, height, bass, window);

    let bar_width = width * 0.5 / BANDS as f32 * (1.0 - BAR_GAP);
    let bottom = f32::from(bounds.origin.y) + height;
    for (band, level) in levels.iter().enumerate() {
        if *level < LEVEL_VISIBLE {
            continue;
        }
        let bar_height = level * height * BAR_MAX_HEIGHT;
        let alpha = BAR_MIN_ALPHA + (BAR_MAX_ALPHA - BAR_MIN_ALPHA) * level;
        let (right, left) = bar_positions(band);
        for fraction in [right, left] {
            let origin = point(
                bounds.origin.x + px(fraction * width),
                px(bottom - bar_height),
            );
            window.paint_quad(quad(
                Bounds::new(origin, size(px(bar_width), px(bar_height))),
                Corners {
                    top_left: px(bar_width / 2.0),
                    top_right: px(bar_width / 2.0),
                    bottom_left: px(0.),
                    bottom_right: px(0.),
                },
                accent(alpha),
                Edges::default(),
                transparent_black(),
                BorderStyle::default(),
            ));
        }
    }
}

fn paint_glow(bounds: Bounds<Pixels>, width: f32, height: f32, bass: f32, window: &mut Window) {
    let glow_width = width * (0.25 + 0.2 * bass);
    let glow_height = height * 0.12;
    let origin = point(
        bounds.origin.x + px((width - glow_width) / 2.0),
        bounds.origin.y + px(height - glow_height / 2.0),
    );
    window.paint_shadows(
        Bounds::new(origin, size(px(glow_width), px(glow_height))),
        Corners::all(px(glow_height / 2.0)),
        &[BoxShadow {
            color: accent(GLOW_ALPHA * bass),
            offset: point(px(0.), px(0.)),
            blur_radius: px(height * (0.12 + 0.18 * bass)),
            spread_radius: px(0.),
        }],
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bars_start_at_the_center_and_stay_on_screen() {
        assert_eq!(bar_positions(0).0, 0.5);
        for band in 0..BANDS {
            let (right, left) = bar_positions(band);
            assert!((0.5..1.0).contains(&right));
            assert!((0.0..0.5).contains(&left));
            let step = 0.5 / BANDS as f32;
            assert!(
                ((0.5 - (left + step)) - (right - 0.5)).abs() < 1e-5,
                "symétrie"
            );
        }
    }
}
