//! Colors of the interface. A theme is four colors, the rest is derived.

use gpui::Rgba;

#[derive(Clone, Copy)]
pub struct Palette {
    pub name: &'static str,
    pub ink: Rgba,
    pub deep: Rgba,
    pub rise: Rgba,
    pub rise2: Rgba,
    pub line: Rgba,
    pub tide: Rgba,
    pub spark: Rgba,
    pub tx: Rgba,
    pub tx2: Rgba,
    pub tx3: Rgba,
    /// From the tide to the text to the spark, for what follows a level.
    pub ramp: [Rgba; 8],
}

/// Name, ink, tide, spark, text. All reach 4.5:1 for text and spark.
pub const THEMES: &[(&str, u32, u32, u32, u32)] = &[
    ("Abysse", 0x090E33, 0x2B3FD6, 0xFF6A45, 0xF4F0E6),
    ("Aurore", 0x120930, 0x6A2BE0, 0xFF6FB5, 0xF8EFF7),
    ("Lagon", 0x031F25, 0x0C9A97, 0xFFD35C, 0xE9F8F4),
    ("Braise", 0x1C0905, 0xC93A1A, 0xFFB84D, 0xFCF0E0),
    ("Jade", 0x041B14, 0x0F9A66, 0xC2F76E, 0xEDF7EA),
    ("Minuit", 0x070A17, 0x2F55A8, 0x78DCFF, 0xEDF4FB),
    ("Cerise", 0x250619, 0xC2185B, 0xFFC9D6, 0xFDF0F3),
    ("Papier", 0xF3EEE2, 0x8FBCD0, 0xC2361B, 0x14213D),
];

thread_local! {
    static CURRENT: std::cell::Cell<Palette> = std::cell::Cell::new(Palette::new(&THEMES[0]));
}

/// The active theme. The interface only ever runs on one thread.
pub fn pal() -> Palette {
    CURRENT.get()
}

pub fn set_palette(palette: Palette) {
    CURRENT.set(palette);
}

fn color(hex: u32) -> Rgba {
    Rgba {
        r: ((hex >> 16) & 255) as f32 / 255.0,
        g: ((hex >> 8) & 255) as f32 / 255.0,
        b: (hex & 255) as f32 / 255.0,
        a: 1.0,
    }
}

pub fn mix(from: Rgba, to: Rgba, amount: f32) -> Rgba {
    let lerp = |a: f32, b: f32| a + (b - a) * amount;
    Rgba {
        r: lerp(from.r, to.r),
        g: lerp(from.g, to.g),
        b: lerp(from.b, to.b),
        a: lerp(from.a, to.a),
    }
}

pub fn alpha(color: Rgba, a: f32) -> Rgba {
    Rgba {
        a: a.clamp(0.0, 1.0),
        ..color
    }
}

fn luminance(color: Rgba) -> f32 {
    let channel = |v: f32| {
        if v <= 0.03928 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * channel(color.r) + 0.7152 * channel(color.g) + 0.0722 * channel(color.b)
}

impl Palette {
    pub fn new(theme: &(&'static str, u32, u32, u32, u32)) -> Self {
        let (name, ink, tide, spark, tx) = *theme;
        let (ink, tide, spark, tx) = (color(ink), color(tide), color(spark), color(tx));
        let black = color(0);
        let light = luminance(ink) > 0.4;
        let mut ramp = [ink; 8];
        for (index, slot) in ramp.iter_mut().enumerate() {
            let u = index as f32 / 7.0;
            *slot = if u < 0.5 {
                mix(tide, tx, u * 2.0)
            } else {
                mix(tx, spark, (u - 0.5) * 2.0)
            };
        }
        Self {
            name,
            ink,
            deep: mix(ink, black, if light { 0.1 } else { 0.45 }),
            rise: mix(ink, tx, 0.07),
            rise2: mix(ink, tx, 0.15),
            line: alpha(tx, 0.16),
            tide,
            spark,
            tx,
            tx2: mix(tx, ink, 0.22),
            tx3: mix(tx, ink, 0.36),
            ramp,
        }
    }

    pub fn named(name: &str) -> Self {
        let theme = THEMES
            .iter()
            .find(|theme| theme.0 == name)
            .unwrap_or(&THEMES[0]);
        Self::new(theme)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn contrast(a: Rgba, b: Rgba) -> f32 {
        let (x, y) = (luminance(a), luminance(b));
        (x.max(y) + 0.05) / (x.min(y) + 0.05)
    }

    #[test]
    fn every_theme_stays_readable() {
        for theme in THEMES {
            let palette = Palette::new(theme);
            assert!(contrast(palette.tx, palette.ink) >= 4.5, "{} texte", theme.0);
            assert!(contrast(palette.spark, palette.ink) >= 3.0, "{} étincelle", theme.0);
        }
    }

    #[test]
    fn an_unknown_name_falls_back_to_the_first_theme() {
        assert_eq!(Palette::named("n'existe pas").name, "Abysse");
        assert_eq!(Palette::named("Jade").name, "Jade");
    }
}
