//! The five visuals: everything comes from what is really being played.
//! Glows are soft shadows behind discs: the renderer has no blur for paths.

use std::f32::consts::{PI, TAU};

use gpui::{
    BorderStyle, Bounds, BoxShadow, ContentMask, Corners, Edges, PathBuilder, Pixels, Rgba, Window,
    linear_color_stop, linear_gradient, point, px, quad, size, transparent_black,
};

use crate::analysis::{Analyzer, Part};
use crate::theme::{Palette, alpha, mix};

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Visual {
    Maree,
    Ricochets,
    Banc,
    Meduse,
    Eclats,
}

pub const VISUALS: [Visual; 5] = [
    Visual::Maree,
    Visual::Ricochets,
    Visual::Banc,
    Visual::Meduse,
    Visual::Eclats,
];

impl Visual {
    pub fn name(self) -> &'static str {
        match self {
            Visual::Maree => "Marée",
            Visual::Ricochets => "Ricochets",
            Visual::Banc => "Banc",
            Visual::Meduse => "Méduse",
            Visual::Eclats => "Éclats",
        }
    }

    pub fn describe(self) -> &'static str {
        match self {
            Visual::Maree => "Une estampe de mer : soleil, houle, embruns.",
            Visual::Ricochets => "La surface vue d'en haut : les ondes s'entrecroisent.",
            Visual::Banc => "320 écailles qui changent de figure selon la partie du morceau.",
            Visual::Meduse => "Une créature qui respire avec la basse.",
            Visual::Eclats => "La pochette elle-même, en mosaïque vivante.",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        VISUALS.into_iter().find(|visual| visual.name() == name)
    }
}

/// A 32 × 32 reduction of the cover.
pub const GRID: usize = 32;
pub type CoverGrid = Vec<Rgba>;

const WAVE_SEGMENTS: usize = 56;
const MAX_SPRAY: usize = 40;
const SHOAL: usize = 320;

struct Swell {
    x: f32,
    strength: f32,
}

struct Spray {
    x: f32,
    y: f32,
    vx: f32,
    vy: f32,
    life: f32,
}

struct Ring {
    x: f32,
    y: f32,
    radius: f32,
    speed: f32,
    life: f32,
    strength: f32,
}

struct Pulse {
    radius: f32,
    strength: f32,
}

/// What a visual remembers between frames.
pub struct VisualState {
    time: f32,
    last_time: Option<std::time::Instant>,
    seen: (u64, u64, u64),
    seed: u32,
    swells: Vec<Swell>,
    spray: Vec<Spray>,
    rings: Vec<Ring>,
    pulses: Vec<Pulse>,
    shoal_x: Vec<f32>,
    shoal_y: Vec<f32>,
    formation: [f32; 4],
    lyrics: f32,
}

impl Default for VisualState {
    fn default() -> Self {
        Self {
            time: 0.0,
            last_time: None,
            seen: (0, 0, 0),
            seed: 1,
            swells: Vec::new(),
            spray: Vec::new(),
            rings: Vec::new(),
            pulses: Vec::new(),
            shoal_x: Vec::new(),
            shoal_y: Vec::new(),
            formation: [1.0, 0.0, 0.0, 0.0],
            lyrics: 0.0,
        }
    }
}

/// The same numbers every time, so nothing flickers without reason.
fn hash(n: u32, s: u32) -> f32 {
    let mut x =
        n.wrapping_add(1).wrapping_mul(2_654_435_761) ^ s.wrapping_add(7).wrapping_mul(40_503);
    x ^= x >> 13;
    x = x.wrapping_mul(1_274_126_177);
    (x ^ (x >> 16)) as f32 / 4_294_967_296.0
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

fn smooth(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

struct Canvas<'a> {
    window: &'a mut Window,
    x: f32,
    y: f32,
}

impl Canvas<'_> {
    fn at(&self, x: f32, y: f32) -> gpui::Point<Pixels> {
        point(px(self.x + x), px(self.y + y))
    }

    fn rounded(&mut self, x: f32, y: f32, w: f32, h: f32, radius: f32, color: Rgba) {
        if w <= 0.2 || h <= 0.2 || color.a <= 0.003 {
            return;
        }
        let bounds = Bounds::new(self.at(x, y), size(px(w), px(h)));
        self.window.paint_quad(quad(
            bounds,
            Corners::all(px(radius.min(w / 2.0).min(h / 2.0).max(0.0))),
            color,
            Edges::default(),
            transparent_black(),
            BorderStyle::default(),
        ));
    }

    fn disc(&mut self, x: f32, y: f32, radius: f32, color: Rgba) {
        self.rounded(
            x - radius,
            y - radius,
            radius * 2.0,
            radius * 2.0,
            radius,
            color,
        );
    }

    fn glow(&mut self, x: f32, y: f32, radius: f32, blur: f32, color: Rgba) {
        if color.a <= 0.003 || radius <= 0.1 {
            return;
        }
        let bounds = Bounds::new(
            self.at(x - radius, y - radius),
            size(px(radius * 2.0), px(radius * 2.0)),
        );
        self.window.paint_shadows(
            bounds,
            Corners::all(px(radius)),
            &[BoxShadow {
                color: color.into(),
                offset: point(px(0.), px(0.)),
                blur_radius: px(blur),
                spread_radius: px(0.),
            }],
        );
    }

    fn shape(&mut self, points: &[(f32, f32)], color: Rgba) {
        if color.a <= 0.003 {
            return;
        }
        if let Some(path) = self.build(points) {
            self.window.paint_path(path, color);
        }
    }

    /// `from` at the top of the shape, `to` at the bottom.
    fn shape_fading(&mut self, points: &[(f32, f32)], from: Rgba, to: Rgba) {
        if let Some(path) = self.build(points) {
            self.window.paint_path(
                path,
                linear_gradient(180., linear_color_stop(from, 0.), linear_color_stop(to, 1.)),
            );
        }
    }

    fn build(&self, points: &[(f32, f32)]) -> Option<gpui::Path<Pixels>> {
        let mut builder = PathBuilder::fill();
        let (first, rest) = points.split_first()?;
        builder.move_to(self.at(first.0, first.1));
        for (x, y) in rest {
            builder.line_to(self.at(*x, *y));
        }
        builder.close();
        builder.build().ok()
    }

    fn background(&mut self, w: f32, h: f32, palette: &Palette) {
        let bounds = Bounds::new(self.at(0.0, 0.0), size(px(w), px(h)));
        self.window.paint_quad(quad(
            bounds,
            Corners::default(),
            linear_gradient(
                180.,
                linear_color_stop(palette.deep, 0.),
                linear_color_stop(palette.ink, 1.),
            ),
            Edges::default(),
            transparent_black(),
            BorderStyle::default(),
        ));
    }
}

/// Paints inside `bounds`. `lyrics` tells the visual to make room for text.
pub fn paint(
    visual: Visual,
    bounds: Bounds<Pixels>,
    sound: &Analyzer,
    palette: &Palette,
    state: &mut VisualState,
    grid: Option<&CoverGrid>,
    lyrics: bool,
    window: &mut Window,
) {
    let now = std::time::Instant::now();
    let dt = state
        .last_time
        .map_or(0.0, |last| now.duration_since(last).as_secs_f32())
        .min(0.1);
    state.last_time = Some(now);
    state.time += dt * (0.2 + 0.8 * sound.activity());
    state.lyrics += (f32::from(u8::from(lyrics)) - state.lyrics) * (1.0 - (-dt / 0.3).exp());
    let (kick, snare, hat) = sound.attacks();
    let hits = Hits {
        kick: kick != state.seen.0,
        snare: snare != state.seen.1,
        hat: hat != state.seen.2,
    };
    state.seen = (kick, snare, hat);

    let (w, h) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
    if w < 1.0 || h < 1.0 {
        return;
    }
    let (x, y) = (f32::from(bounds.origin.x), f32::from(bounds.origin.y));
    window.with_content_mask(Some(ContentMask { bounds }), |window| {
        let mut canvas = Canvas { window, x, y };
        canvas.background(w, h, palette);
        let frame = Frame {
            w,
            h,
            dt,
            sound,
            palette,
            hits,
        };
        match visual {
            Visual::Maree => maree(&mut canvas, &frame, state),
            Visual::Ricochets => ricochets(&mut canvas, &frame, state),
            Visual::Banc => banc(&mut canvas, &frame, state),
            Visual::Meduse => meduse(&mut canvas, &frame, state),
            Visual::Eclats => eclats(&mut canvas, &frame, state, grid),
        }
    });
}

struct Hits {
    kick: bool,
    snare: bool,
    hat: bool,
}

struct Frame<'a> {
    w: f32,
    h: f32,
    dt: f32,
    sound: &'a Analyzer,
    palette: &'a Palette,
    hits: Hits,
}

fn next_random(state: &mut VisualState) -> f32 {
    state.seed = state.seed.wrapping_add(1);
    hash(state.seed, 17)
}

/// Sea and sun: low bands raise the near waves, high ones the far ones.
fn maree(c: &mut Canvas, f: &Frame, s: &mut VisualState) {
    let (w, h, a, p) = (f.w, f.h, f.sound, f.palette);
    let (b, m, l, t) = (a.levels(), w.min(h), s.lyrics, s.time);
    let (act, bass, mid, high) = (a.activity(), a.punch(), a.mid(), a.high());
    let horizon = h * (0.5 + 0.16 * l - 0.06 * a.lift() * (1.0 - l));

    for i in 0..30 {
        let twinkle = hash(i, (t * 6.0) as u32 + 1);
        let size = 1.5 + 2.5 * hash(i, 1);
        let shade = alpha(p.tx, (0.12 + 0.75 * high * twinkle) * (1.0 - 0.55 * l));
        c.rounded(
            hash(i, 2) * w,
            hash(i, 3) * horizon * 0.92,
            size,
            size,
            size,
            shade,
        );
    }

    let sun_x = w * lerp(0.62, 0.78, l);
    let sun_y = horizon - m * 0.1;
    let sun_r = m * (0.12 + 0.04 * bass);
    c.glow(
        sun_x,
        sun_y,
        sun_r,
        24.0 + 130.0 * bass,
        alpha(p.spark, 0.5 + 0.5 * bass),
    );
    c.disc(
        sun_x,
        sun_y,
        sun_r * 1.34,
        alpha(p.spark, 0.16 + 0.2 * bass),
    );
    c.disc(sun_x, sun_y, sun_r, p.spark);

    if f.hits.kick {
        s.swells.push(Swell {
            x: -0.1,
            strength: 0.55 + 0.45 * a.energy().min(1.0),
        });
        if s.swells.len() > 6 {
            s.swells.remove(0);
        }
    }
    for swell in &mut s.swells {
        swell.x += f.dt * 0.55 * (0.2 + act);
        swell.strength *= 0.6_f32.powf(f.dt);
    }
    s.swells.retain(|swell| swell.x <= 1.2);

    let mut crest = [0.0_f32; WAVE_SEGMENTS + 1];
    let mut front = [0.0_f32; WAVE_SEGMENTS + 1];
    let mut points = Vec::with_capacity(WAVE_SEGMENTS * 2 + 4);
    for layer in 0..8 {
        let d = layer as f32 / 7.0;
        let base = horizon + (h - horizon) * d.powf(1.25) * 0.82;
        let band = lerp(38.0, 2.0, d) as usize;
        let amp = (3.0 + m * 0.006 * (1.0 - d * 0.3) + m * (0.012 + 0.075 * d) * b[band])
            * (1.0 - 0.45 * l);
        let f1 = lerp(0.021, 0.0052, d);
        let f2 = f1 * 2.3;
        let speed = lerp(1.3, 0.7, d);
        points.clear();
        points.push((0.0, h));
        for (i, height) in crest.iter_mut().enumerate() {
            let u = i as f32 / WAVE_SEGMENTS as f32;
            let x = u * w;
            let phase = layer as f32;
            let mut y = base
                - amp
                    * ((x * f1 + t * speed + phase * 1.7).sin()
                        + 0.45 * (x * f2 - t * speed * 0.7 + phase).sin());
            for swell in &s.swells {
                let z = (u - swell.x) / 0.11;
                y -= swell.strength * m * 0.07 * d * d * (-0.5 * z * z).exp() * act;
            }
            *height = y;
            points.push((x, y));
        }
        points.push((w, h));
        c.shape_fading(
            &points,
            mix(p.tide, p.ink, d * 0.82),
            mix(p.tide, p.deep, (d + 0.25).min(1.0)),
        );
        if layer >= 3 {
            let thick = 1.2 + 7.0 * mid * d;
            points.clear();
            for (i, y) in crest.iter().enumerate() {
                points.push((i as f32 / WAVE_SEGMENTS as f32 * w, *y));
            }
            for (i, y) in crest.iter().enumerate().rev() {
                let wobble = 0.5 + 0.5 * (i as f32 * 0.45 + t * 2.0 + layer as f32).sin();
                points.push((i as f32 / WAVE_SEGMENTS as f32 * w, y + thick * wobble));
            }
            c.shape(
                &points,
                alpha(p.tx, (0.16 + 0.5 * high * d) * (1.0 - 0.4 * l)),
            );
        }
        if layer == 1 {
            for j in 0..14 {
                let u = j as f32 / 14.0;
                let width = sun_r * 2.0 * (1.0 - u * 0.7) * (0.35 + 0.9 * b[j * 3]);
                let reflect = alpha(p.spark, 0.6 * (1.0 - u) * (1.0 - 0.45 * l));
                c.rounded(
                    sun_x - width / 2.0 + (t * 1.3 + j as f32).sin() * 6.0 * act,
                    horizon + 6.0 + u * h * 0.24,
                    width,
                    3.0 + u * 3.0,
                    3.0,
                    reflect,
                );
            }
        }
        if layer == 6 {
            front = crest;
        }
    }

    if f.hits.kick && a.energy() > 0.25 {
        for _ in 0..5 {
            let u = next_random(s);
            let (spread, rise) = (next_random(s), next_random(s));
            s.spray.push(Spray {
                x: u * w,
                y: front[(u * WAVE_SEGMENTS as f32) as usize],
                vx: (spread - 0.5) * 60.0,
                vy: -120.0 - rise * 220.0 * bass,
                life: 1.0,
            });
        }
    }
    if s.spray.len() > MAX_SPRAY {
        let extra = s.spray.len() - MAX_SPRAY;
        s.spray.drain(..extra);
    }
    let step = f.dt * (0.15 + act * 0.85);
    for drop in &mut s.spray {
        drop.x += drop.vx * step;
        drop.y += drop.vy * step;
        drop.vy += 420.0 * step;
        drop.life -= step * 0.9;
        let size = 2.0 + 3.0 * drop.life;
        c.rounded(
            drop.x,
            drop.y,
            size,
            size * 1.6,
            size,
            alpha(p.tx, drop.life * 0.85 * (1.0 - 0.5 * l)),
        );
    }
    s.spray.retain(|drop| drop.life > 0.0);
}

/// Stones in water: rings add up over a grid, so interference really shows.
fn ricochets(c: &mut Canvas, f: &Frame, s: &mut VisualState) {
    let (w, h, a, p) = (f.w, f.h, f.sound, f.palette);
    let (b, m, l, t) = (a.levels(), w.min(h), s.lyrics, s.time);
    let zone_x = lerp(0.5, 0.76, l);
    let zone_w = lerp(0.42, 0.2, l);
    if f.hits.kick {
        s.rings.push(Ring {
            x: w * (zone_x + zone_w * (t * 0.37).sin()),
            y: h * (0.46 + 0.34 * (t * 0.53 + 1.0).sin()),
            radius: 4.0,
            speed: m * 0.62,
            life: 1.0,
            strength: 1.0,
        });
    }
    if f.hits.snare {
        s.seed = s.seed.wrapping_add(1);
        let k = s.seed as f32 * 2.399;
        s.rings.push(Ring {
            x: w * (zone_x + zone_w * k.cos()),
            y: h * (0.5 + 0.4 * k.sin()),
            radius: 2.0,
            speed: m * 0.42,
            life: 0.8,
            strength: 0.65,
        });
    }
    if s.rings.len() > 14 {
        let extra = s.rings.len() - 14;
        s.rings.drain(..extra);
    }
    let step = f.dt * (0.3 + a.activity() * 0.7);
    for ring in &mut s.rings {
        ring.radius += ring.speed * step;
        ring.speed *= 0.72_f32.powf(step);
        ring.life -= step * 0.42;
    }
    s.rings.retain(|ring| ring.life > 0.0);

    let columns = ((190.0 * w / h).sqrt().round() as usize).max(6);
    let rows = (190 / columns).max(4);
    let (cell_w, cell_h) = (w / columns as f32, h / rows as f32);
    let sigma = m * 0.045;
    for row in 0..rows {
        for column in 0..columns {
            let cx = (column as f32 + 0.5) * cell_w;
            let cy = (row as f32 + 0.5) * cell_h;
            let mut value = 0.0;
            for ring in &s.rings {
                let (dx, dy) = (cx - ring.x, cy - ring.y);
                let d = (dx * dx + dy * dy).sqrt() - ring.radius;
                value += ring.strength
                    * ring.life
                    * (-(d * d) / (2.0 * sigma * sigma)).exp()
                    * (d * 0.05).cos();
            }
            let height = value.clamp(-0.5, 1.3);
            let scale = (0.1 + 0.05 * (cx * 0.01 + cy * 0.013 + t).sin() + 0.62 * height.max(0.0))
                .clamp(0.06, 1.05);
            let dot = cell_w.min(cell_h) * scale;
            let drop = (-height).max(0.0) * 6.0;
            let zone = lerp(1.0, 0.12 + 0.88 * smooth((cx / w - 0.42) / 0.28), l);
            let shade = p.ramp[(1.0 + height * 5.0).clamp(0.0, 7.0) as usize];
            c.rounded(
                cx - dot / 2.0,
                cy - dot / 2.0 + drop,
                dot,
                dot,
                dot * (0.5 - 0.22 * height.clamp(0.0, 1.0)),
                alpha(shade, (0.3 + 0.7 * height.abs()).min(1.0) * zone),
            );
        }
    }
    let mut points = Vec::with_capacity(114);
    for ring in &s.rings {
        let thick = 1.5 + m * 0.018 * ring.life * ring.strength;
        points.clear();
        let bump = |k: usize| b[((k as f32 / 56.0 * 2.0 - 1.0).abs() * 47.0) as usize];
        for k in 0..=56 {
            let angle = k as f32 / 56.0 * TAU;
            let r = ring.radius + thick + ring.radius * 0.06 * bump(k) * ring.life;
            points.push((ring.x + angle.cos() * r, ring.y + angle.sin() * r));
        }
        for k in (0..=56).rev() {
            let angle = k as f32 / 56.0 * TAU;
            let r = (ring.radius + ring.radius * 0.06 * bump(k) * ring.life).max(0.0);
            points.push((ring.x + angle.cos() * r, ring.y + angle.sin() * r));
        }
        c.shape(
            &points,
            alpha(p.tx, ring.life * ring.life * 0.7 * (1.0 - 0.35 * l)),
        );
    }
    if let Some(ring) = s.rings.last() {
        let radius = 2.0 + m * 0.02 * a.kick_envelope();
        c.glow(
            ring.x,
            ring.y,
            radius,
            20.0 + 50.0 * a.punch(),
            alpha(p.spark, ring.life),
        );
        c.disc(ring.x, ring.y, radius, alpha(p.spark, ring.life));
    }
}

/// A shoal whose figure follows the part of the song.
fn banc(c: &mut Canvas, f: &Frame, s: &mut VisualState) {
    let (w, h, a, p) = (f.w, f.h, f.sound, f.palette);
    let (b, m, l, t) = (a.levels(), w.min(h), s.lyrics, s.time);
    if s.shoal_x.is_empty() {
        s.shoal_x = vec![w / 2.0; SHOAL];
        s.shoal_y = vec![h / 2.0; SHOAL];
    }
    let target = match a.part() {
        Part::Chorus => 2,
        Part::Verse => 1,
        Part::Bridge => 3,
        Part::Calm => 0,
    };
    for (k, weight) in s.formation.iter_mut().enumerate() {
        let wanted = if k == target { 1.0 } else { 0.0 };
        *weight += (wanted - *weight) * (1.0 - (-f.dt * 1.4).exp());
    }
    let (cx, cy, scale) = (w * lerp(0.5, 0.74, l), h * 0.48, lerp(1.0, 0.72, l));
    let push = 1.0 + 0.34 * a.kick_envelope();
    let follow = 1.0 - 0.002_f32.powf(f.dt * (0.05 + a.activity()));
    let weights = s.formation;
    for i in 0..SHOAL {
        let index = i as u32;
        let u = i as f32 / SHOAL as f32;
        let level = b[i % b.len()];
        let (a0, r0) = (u * TAU * 34.0 + t * 0.5, u.sqrt() * m * 0.17);
        let a1 = u * TAU * 3.0 + t * 0.32;
        let r1 = m * (0.28 + 0.06 * (u * TAU * 7.0 + t).sin() + 0.05 * hash(index, 1));
        let a2 = u * TAU * 2.2 + (i % 3) as f32 * TAU / 3.0 + t * 0.6;
        let r2 = (0.06 + u * 0.48) * m;
        let line_x = (u - 0.5) * w * (1.0 - 0.3 * l);
        let line_y = (u * 9.0 + t * 1.1).sin() * h * 0.17 + (hash(index, 2) - 0.5) * m * 0.06;
        let radial = push * (1.0 + level * 0.2) * scale;
        let px_ = (weights[0] * a0.cos() * r0
            + weights[1] * a1.cos() * r1 * 1.35
            + weights[2] * a2.cos() * r2 * 1.25
            + weights[3] * line_x)
            * radial;
        let py_ = (weights[0] * a0.sin() * r0
            + weights[1] * a1.sin() * r1
            + weights[2] * a2.sin() * r2
            + weights[3] * line_y)
            * radial;
        let (old_x, old_y) = (s.shoal_x[i], s.shoal_y[i]);
        let new_x = old_x + (cx + px_ - old_x) * follow;
        let new_y = old_y + (cy + py_ - old_y) * follow;
        s.shoal_x[i] = new_x;
        s.shoal_y[i] = new_y;
        let (mut dx, mut dy) = (new_x - old_x, new_y - old_y);
        let mut moved = (dx * dx + dy * dy).sqrt();
        if moved < 0.05 {
            dx = (a1 + PI / 2.0).cos();
            dy = (a1 + PI / 2.0).sin();
            moved = 1.0;
        }
        dx /= moved;
        dy /= moved;
        let long = 5.0 + 22.0 * level + (moved * 0.6).min(10.0);
        let wide = 1.4 + 3.2 * level;
        let flash = f.hits.hat && hash(index, (t * 40.0) as u32) < 0.1;
        let shade = if flash {
            p.tx
        } else {
            p.ramp[(level * 2.0 + u * 2.5 + 0.4).clamp(0.0, 7.0) as usize]
        };
        let opacity = if flash { 1.0 } else { 0.5 + 0.5 * level } * (1.0 - 0.42 * l);
        c.shape(
            &[
                (new_x + dx * long, new_y + dy * long),
                (new_x - dy * wide, new_y + dx * wide),
                (new_x - dx * long * 0.7, new_y - dy * long * 0.7),
                (new_x + dy * wide, new_y - dx * wide),
            ],
            alpha(shade, opacity),
        );
    }
}

/// A jellyfish: the kick contracts its bell, each tentacle follows a band.
fn meduse(c: &mut Canvas, f: &Frame, s: &mut VisualState) {
    let (w, h, a, p) = (f.w, f.h, f.sound, f.palette);
    let (b, m, l, t) = (a.levels(), w.min(h), s.lyrics, s.time);
    let (mid, high, bass, pulse) = (a.mid(), a.high(), a.punch(), a.kick_envelope());
    let cx = w * lerp(0.5, 0.76, l) + (t * 0.3).sin() * m * 0.04;
    let cy = h * (0.34 + 0.03 * (t * 0.5).sin()) - bass * m * 0.018;
    let fade = 1.0 - 0.32 * l;

    for i in 0..5 {
        let x0 = w * (0.12 + i as f32 * 0.2) + (t * 0.13 + i as f32).sin() * 30.0;
        let spread = w * 0.05 + i as f32 * 12.0;
        c.shape_fading(
            &[
                (x0, 0.0),
                (x0 + spread, 0.0),
                (x0 + spread * 2.4 - 40.0, h * 0.85),
                (x0 - spread * 0.6 - 40.0, h * 0.85),
            ],
            alpha(p.tx, (0.04 + 0.13 * mid) * fade),
            alpha(p.tx, 0.0),
        );
    }
    for i in 0..70_u32 {
        let speed = 0.3 + hash(i, 1);
        let x = (hash(i, 2) * w + t * 11.0 * speed).rem_euclid(w);
        let y = (hash(i, 3) * h - t * 18.0 * speed).rem_euclid(h);
        let (dx, dy) = (x - cx, y - cy);
        let d = (dx * dx + dy * dy).sqrt() + 1.0;
        let shove = pulse * (1.0 - d / (m * 0.5)).max(0.0) * 38.0;
        let size = 1.5 + 3.0 * high * hash(i, 4) + 1.5 * hash(i, 5);
        let shade = if i % 4 == 0 { p.spark } else { p.tx };
        c.rounded(
            x + dx / d * shove,
            y + dy / d * shove,
            size,
            size,
            size,
            alpha(
                shade,
                (0.18 + 0.7 * high * hash(i, (t * 5.0) as u32)) * fade,
            ),
        );
    }

    let bell_w = m * 0.215 * (1.0 - 0.12 * pulse) * (1.0 + 0.08 * mid);
    let bell_h = m * 0.175 * (1.0 + 0.16 * pulse);
    let rim_y = cy + bell_h * 0.08;
    let rim = |x: f32| rim_y + (x * 0.045 + t * 2.1).sin() * m * 0.008;

    const STEPS: usize = 12;
    let mut outline = Vec::with_capacity((STEPS + 1) * 2);
    let ribbon = |outline: &mut Vec<(f32, f32)>, spine: &dyn Fn(f32) -> (f32, f32, f32)| {
        outline.clear();
        for step in 0..=STEPS {
            let (x, y, half) = spine(step as f32 / STEPS as f32);
            outline.push((x - half, y));
        }
        for step in (0..=STEPS).rev() {
            let (x, y, half) = spine(step as f32 / STEPS as f32);
            outline.push((x + half, y));
        }
    };
    for i in 0..24_usize {
        let side = i as f32 / 23.0 * 2.0 - 1.0;
        let base_x = cx + side * bell_w * 0.9;
        let base_y = rim(base_x);
        let level = b[(i * 2) % b.len()];
        let long = m * (0.17 + 0.56 * level) * (1.0 - 0.3 * l);
        let spine = |v: f32| {
            let sample = a.wave((i * 83 + (v * STEPS as f32) as usize * 37) % 2048);
            (
                base_x
                    + side * long * 0.26 * v
                    + (v * 5.0 + t * 1.7 + i as f32).sin() * long * 0.1 * v
                    + sample * long * 0.12 * v,
                base_y + long * v,
                (1.0 - v) * (0.9 + 3.2 * level),
            )
        };
        ribbon(&mut outline, &spine);
        let shade = if i % 3 == 0 { p.spark } else { p.tx };
        c.shape(&outline, alpha(shade, (0.34 + 0.52 * level) * fade));
        if i % 2 == 0 {
            let (tip_x, tip_y, _) = spine(1.0);
            c.disc(
                tip_x,
                tip_y,
                1.4 + 2.6 * level,
                alpha(p.spark, (0.4 + 0.6 * level) * fade),
            );
        }
    }
    for k in 0..4 {
        let side = (k as f32 - 1.5) / 1.5;
        let base_x = cx + side * bell_w * 0.32;
        let long = m * (0.22 + 0.22 * mid);
        let spine = |v: f32| {
            (
                base_x
                    + (v * 7.0 + t * 1.2 + k as f32 * 2.0).sin() * m * 0.035 * v
                    + side * m * 0.02 * v,
                rim_y + long * v,
                (1.0 - v * 0.85) * m * 0.016,
            )
        };
        ribbon(&mut outline, &spine);
        c.shape(&outline, alpha(p.spark, 0.68 * fade));
    }
    let dome = |scale_x: f32, scale_y: f32| {
        let mut points = Vec::with_capacity(62);
        for i in 0..=36 {
            let angle = PI + i as f32 / 36.0 * PI;
            points.push((
                cx + angle.cos() * bell_w * scale_x,
                cy + angle.sin() * bell_h * scale_y,
            ));
        }
        for i in (0..=24).rev() {
            let x = cx + (i as f32 / 24.0 * 2.0 - 1.0) * bell_w * scale_x;
            points.push((x, rim(x)));
        }
        points
    };
    c.glow(
        cx,
        cy - bell_h * 0.3,
        bell_w * 0.8,
        (36.0 + 110.0 * bass) * fade,
        alpha(p.spark, (0.4 + 0.5 * bass) * fade),
    );
    c.shape_fading(
        &dome(1.0, 1.0),
        alpha(p.tx, 0.62 * fade),
        alpha(p.spark, 0.46 * fade),
    );
    c.shape(&dome(0.72, 0.74), alpha(p.tide, 0.62));
    for i in 0..9 {
        let side = (i as f32 - 4.0) / 4.0;
        let (top_x, top_y) = (cx + side * bell_w * 0.1, cy - bell_h * 0.92);
        let bottom = cx + side * bell_w * 0.86;
        let half = 1.5 + m * 0.012 * b[14 + i * 2];
        c.shape(
            &[
                (top_x, top_y),
                (bottom - half, rim(bottom)),
                (bottom + half, rim(bottom)),
            ],
            alpha(p.tx, (0.2 + 0.5 * mid) * fade),
        );
    }
}

/// The cover as a mosaic: from the center out, each ring of tiles is a band.
fn eclats(c: &mut Canvas, f: &Frame, s: &mut VisualState, grid: Option<&CoverGrid>) {
    let (w, h, a, p) = (f.w, f.h, f.sound, f.palette);
    let (b, l, t) = (a.levels(), s.lyrics, s.time);
    if f.hits.kick {
        s.pulses.push(Pulse {
            radius: 0.0,
            strength: 0.6 + 0.4 * a.energy().min(1.0),
        });
        if s.pulses.len() > 5 {
            s.pulses.remove(0);
        }
    }
    let step = f.dt * (0.3 + a.activity() * 0.7);
    for pulse in &mut s.pulses {
        pulse.radius += step * 1.5;
    }
    s.pulses.retain(|pulse| pulse.radius <= 1.5);

    let columns = ((400.0 * w / h).sqrt().round() as usize).max(8);
    let tile = w / columns as f32;
    let rows = (h / tile).ceil() as usize;
    let (cx, cy) = (w * lerp(0.5, 0.72, l), h * 0.5);
    let reach = (w * w + h * h).sqrt() * 0.56;
    let side = w.max(h);
    for row in 0..rows {
        for column in 0..columns {
            let x = (column as f32 + 0.5) * tile;
            let y = (row as f32 + 0.5) * tile;
            let (dx, dy) = (x - cx, y - cy);
            let distance = (dx * dx + dy * dy).sqrt();
            let d = distance / reach;
            let level = b[(d.clamp(0.0, 0.999) * b.len() as f32) as usize];
            let mut wave = 0.0;
            for pulse in &s.pulses {
                let z = (d - pulse.radius) / 0.07;
                wave += pulse.strength * (1.0 - pulse.radius / 1.5) * (-0.5 * z * z).exp();
            }
            let u = ((x - w / 2.0) / side + 0.5).clamp(0.0, 0.999);
            let v = ((y - h / 2.0) / side + 0.5).clamp(0.0, 0.999);
            let base = match grid {
                Some(grid) => grid[(v * GRID as f32) as usize * GRID + (u * GRID as f32) as usize],
                None => p.ramp[((u + v) * 3.5) as usize],
            };
            let dot = tile
                * (0.42 + 0.62 * level + 0.42 * wave + 0.05 * (t + d * 9.0).sin()).clamp(0.2, 1.22);
            let (nx, ny) = if distance > 0.0 {
                (dx / distance, dy / distance)
            } else {
                (0.0, 0.0)
            };
            let shift = wave * tile * 1.6;
            let opacity = lerp(1.0, 0.1 + 0.9 * smooth((x / w - 0.3) / 0.42), l);
            let shade = if wave > 0.55 { p.tx } else { base };
            c.rounded(
                x - dot / 2.0 + nx * shift,
                y - dot / 2.0 + ny * shift,
                dot,
                dot,
                dot * (0.5 - 0.36 * (level + wave).clamp(0.0, 1.0)),
                alpha(shade, opacity),
            );
        }
    }
}

#[derive(Default)]
pub struct WaterLine {
    time: f32,
    last_time: Option<std::time::Instant>,
    seen: u64,
    seed: u32,
    bubbles: Vec<Spray>,
}

/// The progress bar as a water line: the sound swells around the playhead.
pub fn paint_water_line(
    bounds: Bounds<Pixels>,
    sound: &Analyzer,
    palette: &Palette,
    progress: f32,
    state: &mut WaterLine,
    window: &mut Window,
) {
    let (w, h) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
    let (x, y) = (f32::from(bounds.origin.x), f32::from(bounds.origin.y));
    let now = std::time::Instant::now();
    let dt = state
        .last_time
        .map_or(0.0, |last| now.duration_since(last).as_secs_f32())
        .min(0.1);
    state.last_time = Some(now);
    let act = sound.activity();
    state.time += dt * act;
    let (levels, t, p) = (sound.levels(), state.time, progress.clamp(0.0, 1.0));
    let segments = ((w / 8.0) as usize).clamp(8, 150);
    let heights: Vec<f32> = (0..=segments)
        .map(|i| {
            let u = i as f32 / segments as f32;
            let gap = (u - p).abs();
            let level = levels[((gap * 90.0) as usize).min(47)];
            h - 11.0
                - level * 16.0 * (-gap * 5.0).exp()
                - ((u * 38.0 + t * 2.4).sin() + (u * 17.0 - t * 1.5).sin()) * 1.2 * act
        })
        .collect();
    let at = |i: usize| i as f32 / segments as f32 * w;
    let mut c = Canvas { window, x, y };
    let mut points: Vec<(f32, f32)> = vec![(0.0, h)];
    points.extend(
        heights
            .iter()
            .enumerate()
            .map(|(i, height)| (at(i), *height)),
    );
    points.push((w, h));
    c.shape(&points, palette.deep);

    let mut ribbon = |from: usize, to: usize, thick: f32, color: Rgba| {
        if from >= to {
            return;
        }
        let mut points: Vec<(f32, f32)> = (from..=to).map(|i| (at(i), heights[i])).collect();
        points.extend((from..=to).rev().map(|i| (at(i), heights[i] + thick)));
        c.shape(&points, color);
    };
    let head = ((p * segments as f32).round() as usize).min(segments);
    ribbon(head, segments, 1.5, alpha(palette.tx, 0.42));
    ribbon(0, head, 3.0, palette.spark);

    let bass = sound.punch();
    let (head_x, head_y) = (p * w, heights[head] + 1.0);
    let radius = 4.5 + 3.0 * bass;
    c.glow(head_x, head_y, radius, 10.0 + 26.0 * bass, palette.spark);
    c.disc(head_x, head_y, radius, palette.tx);

    let (kicks, _, _) = sound.attacks();
    if kicks != state.seen && state.bubbles.len() < 8 {
        state.seed = state.seed.wrapping_add(1);
        state.bubbles.push(Spray {
            x: head_x + (hash(state.seed, 1) - 0.5) * 26.0,
            y: head_y,
            vx: 0.0,
            vy: 18.0 + hash(state.seed, 2) * 22.0,
            life: 1.0,
        });
    }
    state.seen = kicks;
    for bubble in &mut state.bubbles {
        bubble.y -= bubble.vy * dt * (0.2 + act);
        bubble.life -= dt * 1.4 * (0.2 + act);
        c.disc(
            bubble.x,
            bubble.y,
            1.0 + 1.4 * bubble.life.max(0.0),
            alpha(palette.tx, bubble.life * 0.8),
        );
    }
    state.bubbles.retain(|bubble| bubble.life > 0.0);
}

/// The volume as rising bars, which breathe with the music.
pub fn paint_volume(
    bounds: Bounds<Pixels>,
    sound: &Analyzer,
    palette: &Palette,
    volume: f32,
    window: &mut Window,
) {
    const BARS: usize = 16;
    let (w, h) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
    let (x, y) = (f32::from(bounds.origin.x), f32::from(bounds.origin.y));
    let mut c = Canvas { window, x, y };
    let width = w / BARS as f32;
    for i in 0..BARS {
        let u = i as f32 / (BARS - 1) as f32;
        let on = (i as f32 + 0.5) / BARS as f32 <= volume;
        let base = h * (0.26 + 0.74 * u);
        let level = sound.levels()[(i * 3).min(47)];
        let height = if on {
            (base * lerp(1.0, 0.74 + 0.26 * (level * 1.6).min(1.0), sound.activity())).max(3.0)
        } else {
            base
        };
        let color = if on {
            mix(palette.tx, palette.spark, u)
        } else {
            alpha(palette.tx, 0.17)
        };
        c.rounded(
            i as f32 * width + 0.5,
            h - height,
            width - 2.0,
            height,
            2.0,
            color,
        );
    }
}

/// Clips can't be round: this paints the page's color over the corners instead.
pub fn paint_corners(bounds: Bounds<Pixels>, radius: f32, color: Rgba, window: &mut Window) {
    let (w, h) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
    let (x, y) = (f32::from(bounds.origin.x), f32::from(bounds.origin.y));
    let mut c = Canvas { window, x, y };
    for (corner_x, corner_y, sign_x, sign_y) in [
        (0.0, 0.0, 1.0, 1.0),
        (w, 0.0, -1.0, 1.0),
        (0.0, h, 1.0, -1.0),
        (w, h, -1.0, -1.0),
    ] {
        let mut points = vec![(corner_x, corner_y)];
        for step in 0..=10 {
            let angle = step as f32 / 10.0 * PI / 2.0;
            points.push((
                corner_x + sign_x * radius * (1.0 - angle.sin()),
                corner_y + sign_y * radius * (1.0 - angle.cos()),
            ));
        }
        c.shape(&points, color);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashes_stay_between_0_and_1_and_repeat() {
        for i in 0..2000 {
            let value = hash(i, 3);
            assert!((0.0..1.0).contains(&value));
            assert_eq!(value, hash(i, 3));
        }
    }

    #[test]
    fn every_visual_has_a_name_it_can_be_found_by() {
        for visual in VISUALS {
            assert_eq!(Visual::from_name(visual.name()), Some(visual));
            assert!(!visual.describe().is_empty());
        }
        assert_eq!(Visual::from_name("autre"), None);
    }
}

/// A wide band of bars mirrored around the middle, lit up to where the track is.
pub fn paint_wave(
    bounds: Bounds<Pixels>,
    sound: &Analyzer,
    palette: &Palette,
    progress: f32,
    window: &mut Window,
) {
    const BARS: usize = 96;
    let (w, h) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
    let (x, y) = (f32::from(bounds.origin.x), f32::from(bounds.origin.y));
    let mut c = Canvas { window, x, y };
    let width = w / BARS as f32;
    let levels = sound.levels();
    for i in 0..BARS {
        let u = i as f32 / (BARS - 1) as f32;
        // Folded so that the low bands sit in the middle, where the eye goes.
        let band = ((u - 0.5).abs() * 2.0 * (levels.len() - 1) as f32) as usize;
        let rest = 0.12 + 0.1 * (i as f32 * 0.7).sin().abs();
        let level = lerp(
            rest,
            (levels[band] * 1.5).min(1.0).max(rest),
            sound.activity(),
        );
        let height = (h * level).max(3.0);
        let color = if u <= progress {
            mix(palette.tide, palette.spark, u)
        } else {
            alpha(palette.tx, 0.18)
        };
        c.rounded(
            i as f32 * width + 1.0,
            (h - height) / 2.0,
            (width - 2.0).max(1.0),
            height,
            ((width - 2.0) / 2.0).max(1.0),
            color,
        );
    }
}
