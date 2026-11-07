//! Fonts and icons built into the program.

use std::borrow::Cow;

use anyhow::Result;
use gpui::{App, AssetSource, SharedString};

const ICONS: &[(&str, &[u8])] = &[
    ("icons/play.svg", include_bytes!("../assets/icons/play.svg")),
    ("icons/pause.svg", include_bytes!("../assets/icons/pause.svg")),
    ("icons/prev.svg", include_bytes!("../assets/icons/prev.svg")),
    ("icons/next.svg", include_bytes!("../assets/icons/next.svg")),
    ("icons/vol.svg", include_bytes!("../assets/icons/vol.svg")),
    ("icons/volx.svg", include_bytes!("../assets/icons/volx.svg")),
    ("icons/lyr.svg", include_bytes!("../assets/icons/lyr.svg")),
    ("icons/queue.svg", include_bytes!("../assets/icons/queue.svg")),
    ("icons/wave.svg", include_bytes!("../assets/icons/wave.svg")),
    ("icons/mini.svg", include_bytes!("../assets/icons/mini.svg")),
    ("icons/expand.svg", include_bytes!("../assets/icons/expand.svg")),
    ("icons/close.svg", include_bytes!("../assets/icons/close.svg")),
    ("icons/back.svg", include_bytes!("../assets/icons/back.svg")),
    ("icons/search.svg", include_bytes!("../assets/icons/search.svg")),
    ("icons/plus.svg", include_bytes!("../assets/icons/plus.svg")),
    ("icons/grip.svg", include_bytes!("../assets/icons/grip.svg")),
    ("icons/folder.svg", include_bytes!("../assets/icons/folder.svg")),
    ("icons/server.svg", include_bytes!("../assets/icons/server.svg")),
    ("icons/cloud.svg", include_bytes!("../assets/icons/cloud.svg")),
    ("icons/video.svg", include_bytes!("../assets/icons/video.svg")),
    ("icons/disc.svg", include_bytes!("../assets/icons/disc.svg")),
    ("icons/home.svg", include_bytes!("../assets/icons/home.svg")),
    ("icons/lib.svg", include_bytes!("../assets/icons/lib.svg")),
    ("icons/sliders.svg", include_bytes!("../assets/icons/sliders.svg")),
    ("icons/beaker.svg", include_bytes!("../assets/icons/beaker.svg")),
    ("icons/spec.svg", include_bytes!("../assets/icons/spec.svg")),
    ("icons/trash.svg", include_bytes!("../assets/icons/trash.svg")),
    ("icons/palette.svg", include_bytes!("../assets/icons/palette.svg")),
    ("icons/listplus.svg", include_bytes!("../assets/icons/listplus.svg")),
    ("icons/heart.svg", include_bytes!("../assets/icons/heart.svg")),
    ("icons/heart-fill.svg", include_bytes!("../assets/icons/heart-fill.svg")),
    ("icons/shuffle.svg", include_bytes!("../assets/icons/shuffle.svg")),
    ("icons/repeat.svg", include_bytes!("../assets/icons/repeat.svg")),
    ("icons/repeat1.svg", include_bytes!("../assets/icons/repeat1.svg")),
    ("icons/dl.svg", include_bytes!("../assets/icons/dl.svg")),
    ("icons/logo-top.svg", include_bytes!("../assets/icons/logo-top.svg")),
    ("icons/logo-bottom.svg", include_bytes!("../assets/icons/logo-bottom.svg")),
];

const FONTS: &[&[u8]] = &[
    include_bytes!("../assets/fonts/Bricolage-700.ttf"),
    include_bytes!("../assets/fonts/Bricolage-800.ttf"),
    include_bytes!("../assets/fonts/InstrumentSans-400.ttf"),
    include_bytes!("../assets/fonts/InstrumentSans-500.ttf"),
    include_bytes!("../assets/fonts/InstrumentSans-600.ttf"),
    include_bytes!("../assets/fonts/DMMono-Regular.ttf"),
    include_bytes!("../assets/fonts/DMMono-Medium.ttf"),
];

pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        Ok(ICONS
            .iter()
            .find(|(name, _)| *name == path)
            .map(|(_, bytes)| Cow::Borrowed(*bytes)))
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        Ok(ICONS
            .iter()
            .filter(|(name, _)| name.starts_with(path))
            .map(|(name, _)| SharedString::new_static(name))
            .collect())
    }
}

pub fn load_fonts(cx: &App) -> Result<()> {
    cx.text_system()
        .add_fonts(FONTS.iter().map(|font| Cow::Borrowed(*font)).collect())
}
