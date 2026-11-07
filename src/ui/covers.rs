//! Covers, reduced once and kept: a library can hold thousands of them.

use std::sync::Arc;

use gpui::{RenderImage, Rgba};
use image::imageops::FilterType;
use image::{Frame, RgbaImage};
use smallvec::SmallVec;

use super::*;
use crate::visuals::{CoverGrid, GRID};

const SIDE: u32 = 320;
/// More would hammer a remote server while scrolling.
const AT_ONCE: usize = 4;

pub struct CoverArt {
    pub image: Arc<RenderImage>,
    pub grid: CoverGrid,
    /// Its most vivid color, for page headers.
    pub accent: Rgba,
}

fn color_of(pixel: &image::Rgb<u8>) -> Rgba {
    Rgba {
        r: f32::from(pixel[0]) / 255.0,
        g: f32::from(pixel[1]) / 255.0,
        b: f32::from(pixel[2]) / 255.0,
        a: 1.0,
    }
}

fn vividness(color: &Rgba) -> f32 {
    let high = color.r.max(color.g).max(color.b);
    let low = color.r.min(color.g).min(color.b);
    // Saturation counts more, and near-black pixels shouldn't win.
    (high - low) * (0.3 + high)
}

pub fn build_art(bytes: &[u8]) -> Option<CoverArt> {
    let decoded = image::load_from_memory(bytes).ok()?;
    let small = decoded
        .resize_exact(GRID as u32, GRID as u32, FilterType::Triangle)
        .into_rgb8();
    let grid: CoverGrid = small.pixels().map(color_of).collect();
    let accent = grid
        .iter()
        .copied()
        .max_by(|a, b| vividness(a).total_cmp(&vividness(b)))?;
    let mut thumb: RgbaImage = decoded
        .resize_to_fill(SIDE, SIDE, FilterType::Triangle)
        .into_rgba8();
    // The renderer wants its channels in BGRA order.
    for pixel in thumb.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }
    let frames: SmallVec<[Frame; 1]> = SmallVec::from_elem(Frame::new(thumb), 1);
    Some(CoverArt {
        image: Arc::new(RenderImage::new(frames)),
        grid,
        accent,
    })
}

impl Onde {
    /// The cover of an album: if it isn't there yet it is fetched, and the
    /// window redraws when it arrives.
    pub(super) fn art(&mut self, key: &str, cx: &mut Context<Self>) -> Option<Arc<CoverArt>> {
        if let Some(known) = self.covers.get(key) {
            return known.clone();
        }
        self.covers.insert(key.to_owned(), None);
        self.covers_wanted.push(key.to_owned());
        self.fetch_covers(cx);
        None
    }

    pub(super) fn current_art(&mut self, cx: &mut Context<Self>) -> Option<Arc<CoverArt>> {
        let key = album_key(self.queue.current_track()?);
        self.art(&key, cx)
    }

    fn fetch_covers(&mut self, cx: &mut Context<Self>) {
        while self.covers_running < AT_ONCE {
            // The latest wanted is what's on screen now.
            let Some(key) = self.covers_wanted.pop() else {
                return;
            };
            let track = self
                .catalog
                .album(&key)
                .and_then(|album| album.tracks.first())
                .and_then(|&index| self.queue.tracks.get(index))
                .cloned();
            let (Some(track), Some(music)) = (track, self.source.clone()) else {
                continue;
            };
            self.covers_running += 1;
            cx.spawn(async move |this, cx| {
                let art = cx
                    .background_spawn(async move {
                        music
                            .cover(&track)
                            .ok()
                            .flatten()
                            .and_then(|bytes| build_art(&bytes))
                            .map(Arc::new)
                    })
                    .await;
                let _ = this.update(cx, |this, cx| {
                    this.covers_running -= 1;
                    this.covers.insert(key, art);
                    this.fetch_covers(cx);
                    cx.notify();
                });
            })
            .detach();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(width: u32, height: u32, color: [u8; 3]) -> Vec<u8> {
        let picture = image::RgbImage::from_pixel(width, height, image::Rgb(color));
        let mut bytes = std::io::Cursor::new(Vec::new());
        picture
            .write_to(&mut bytes, image::ImageFormat::Png)
            .unwrap();
        bytes.into_inner()
    }

    #[test]
    fn reduces_any_picture_to_a_square_and_a_grid() {
        let art = build_art(&png(900, 500, [200, 40, 20])).unwrap();
        assert_eq!(art.grid.len(), GRID * GRID);
        assert!((art.accent.r - 200.0 / 255.0).abs() < 0.02);
        let bytes = art.image.as_bytes(0).unwrap();
        assert_eq!(bytes.len(), (SIDE * SIDE * 4) as usize);
        assert_eq!(bytes[2], 200, "canaux en BGRA");
    }

    #[test]
    fn what_is_not_a_picture_gives_nothing() {
        assert!(build_art(b"<html>").is_none());
    }

    #[test]
    fn prefers_vivid_colors_over_grey_ones() {
        let grey = Rgba {
            r: 0.8,
            g: 0.8,
            b: 0.8,
            a: 1.0,
        };
        let red = Rgba {
            r: 0.7,
            g: 0.1,
            b: 0.1,
            a: 1.0,
        };
        assert!(vividness(&red) > vividness(&grey));
    }
}
