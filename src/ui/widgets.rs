//! The small parts everything is built from.

use std::sync::Arc;

use gpui::{
    BoxShadow, Div, ElementId, FontWeight, ObjectFit, Rgba, SharedString, Stateful, Svg, div, img,
    linear_color_stop, linear_gradient, point, prelude::*, px, svg,
};

use super::covers::CoverArt;
use crate::theme::rad;
use crate::theme::{alpha, pal};

pub const GOOD: Rgba = Rgba {
    r: 0.486,
    g: 0.941,
    b: 0.698,
    a: 1.0,
};
pub const ERROR: Rgba = Rgba {
    r: 1.0,
    g: 0.478,
    b: 0.478,
    a: 1.0,
};

pub fn icon(name: &str, size: f32, color: Rgba) -> Svg {
    svg()
        .path(SharedString::from(format!("icons/{name}.svg")))
        .size(px(size))
        .flex_none()
        .text_color(color)
}

pub fn icon_button(id: impl Into<ElementId>, name: &str, on: bool) -> Stateful<Div> {
    let palette = pal();
    div()
        .id(id)
        .size(px(36.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(rad(10.))
        .cursor_pointer()
        .when(on, |button| button.bg(palette.rise2))
        .hover(|button| button.bg(pal().rise2))
        .active(|button| button.bg(pal().tide))
        .child(icon(name, 20., if on { palette.spark } else { palette.tx }))
}

pub fn icon_small(id: impl Into<ElementId>, name: &str) -> Stateful<Div> {
    div()
        .id(id)
        .size(px(28.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(rad(6.))
        .cursor_pointer()
        .hover(|button| button.bg(pal().rise2))
        .child(icon(name, 16., pal().tx2))
}

fn pill(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Stateful<Div> {
    div()
        .id(id)
        .h(px(36.))
        .flex_none()
        .flex()
        .items_center()
        .gap_2()
        .px_4()
        .rounded_full()
        .text_size(px(14.))
        .font_weight(FontWeight::MEDIUM)
        .cursor_pointer()
        .child(label.into())
}

/// The usual button.
pub fn btn(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Stateful<Div> {
    let palette = pal();
    pill(id, label)
        .bg(palette.rise2)
        .text_color(palette.tx)
        .hover(|button| button.bg(pal().tide))
}

/// The one main action of a page.
pub fn primary(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Stateful<Div> {
    let palette = pal();
    pill(id, label)
        .bg(palette.spark)
        .text_color(palette.ink)
        .shadow(vec![BoxShadow {
            color: alpha(palette.spark, 0.5).into(),
            offset: point(px(0.), px(0.)),
            blur_radius: px(18.),
            spread_radius: px(0.),
        }])
        .hover(|button| button.opacity(0.9))
}

pub fn ghost(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Stateful<Div> {
    let palette = pal();
    pill(id, label)
        .border_1()
        .border_color(palette.line)
        .text_color(palette.tx)
        .hover(|button| button.bg(pal().rise))
}

pub fn chip(id: impl Into<ElementId>, label: impl Into<SharedString>, on: bool) -> Stateful<Div> {
    let palette = pal();
    div()
        .id(id)
        .h(px(32.))
        .flex_none()
        .flex()
        .items_center()
        .px_3()
        .rounded_full()
        .text_size(px(13.))
        .cursor_pointer()
        .when(on, |chip| chip.bg(palette.tx).text_color(palette.ink))
        .when(!on, |chip| {
            chip.bg(palette.rise2)
                .text_color(palette.tx)
                .hover(|chip| chip.bg(pal().tide))
        })
        .child(label.into())
}

pub fn tag(label: impl Into<SharedString>) -> Div {
    let palette = pal();
    div()
        .h(px(18.))
        .flex_none()
        .flex()
        .items_center()
        .px_1p5()
        .rounded(rad(6.))
        .bg(palette.rise2)
        .text_color(palette.tx)
        .font_family("DM Mono")
        .text_size(px(11.))
        .child(label.into())
}

pub fn display(text: impl Into<SharedString>, size: f32) -> Div {
    div()
        .font_family("Bricolage Grotesque")
        .font_weight(FontWeight::EXTRA_BOLD)
        .text_size(px(size))
        .line_height(px(size * 1.05))
        .child(text.into())
}

pub fn h2(text: impl Into<SharedString>) -> Div {
    div()
        .font_family("Bricolage Grotesque")
        .font_weight(FontWeight::BOLD)
        .text_size(px(22.))
        .line_height(px(28.))
        .child(text.into())
}

pub fn mono(text: impl Into<SharedString>) -> Div {
    div()
        .font_family("DM Mono")
        .text_size(px(12.))
        .line_height(px(16.))
        .child(text.into())
}

/// The cover, or a tide of color while there is none.
pub fn cover(art: Option<&Arc<CoverArt>>, size: f32, radius: f32) -> Div {
    let palette = pal();
    let round = |element: Div| {
        if crate::theme::look().round_covers {
            element.rounded_full()
        } else {
            element.rounded(rad(radius))
        }
    };
    let frame = round(div().size(px(size)).flex_none());
    match art {
        Some(art) => frame.child(
            img(art.image.clone())
                .size(px(size))
                .when(crate::theme::look().round_covers, |image| {
                    image.rounded_full()
                })
                .when(!crate::theme::look().round_covers, |image| {
                    image.rounded(rad(radius))
                })
                .object_fit(ObjectFit::Cover),
        ),
        None => frame
            .flex()
            .items_center()
            .justify_center()
            .bg(linear_gradient(
                160.,
                linear_color_stop(palette.tide, 0.),
                linear_color_stop(palette.rise, 1.),
            ))
            .child(icon("wave", size * 0.42, alpha(palette.tx, 0.5))),
    }
}
