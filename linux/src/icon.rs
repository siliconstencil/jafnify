//! The J-Signal mark: the shared core geometry drawn with the Linux palette.
//!
//! The SVG parse, the arc plan and the state glyphs live in
//! `jafnify_core::icon`, so the Linux window, the tray and any future port
//! cannot drift from the Android vectors. Colours come from the design tokens,
//! never from literals in this file. This module keeps only the raster.

use jafnify_core::icon::{distance_to_segment, in_shape};
use jafnify_core::snapshot::GlyphShape;

use crate::palette::{rgba as to_rgba, Colours};

pub use jafnify_core::icon::{arc_plan, mark, MARK_SVG};

/// The glyph stays near-white on the graphite tile.
pub const WHITE: [u8; 4] = [255, 255, 255, 255];

/// The light-theme accent, kept as a public helper for callers that still
/// name the token by role.
pub fn accent() -> [u8; 4] {
    to_rgba(Colours::light().accent)
}

/// The neutral graphite tile shared by the window and the tray.
pub fn tile() -> [u8; 4] {
    to_rgba(Colours::dark().surface)
}

/// The near-white J shared by the window and the tray.
pub fn glyph() -> [u8; 4] {
    to_rgba(Colours::dark().fg)
}

/// RGBA for eframe's window icon: the product mark on the graphite tile.
pub fn rgba(size: u32) -> Vec<u8> {
    let dark = Colours::dark();
    draw_rgba(
        size,
        to_rgba(dark.surface),
        to_rgba(dark.fg),
        None,
        Some(to_rgba(dark.accent)),
        None,
    )
}

/// ARGB32 in network byte order, as StatusNotifierItem expects.
///
/// This is the generic form with the optional bottom-right dot kept for the
/// tests and for callers that only need a J on a background.
pub fn argb(size: u32, background: [u8; 4], dot: Option<[u8; 4]>) -> Vec<u8> {
    let rgba = draw_rgba(size, background, WHITE, dot, None, None);
    to_argb(&rgba)
}

/// A tray state on the graphite tile: the arcs and the shape glyph carry the
/// state colour, the J stays near-white.
pub fn state_argb(size: u32, shape: GlyphShape, colour: [u8; 4]) -> Vec<u8> {
    let dark = Colours::dark();
    let rgba = draw_rgba(
        size,
        to_rgba(dark.surface),
        to_rgba(dark.fg),
        None,
        Some(colour),
        Some((shape, colour)),
    );
    to_argb(&rgba)
}

/// The window icon data for eframe.
pub fn icon_data(size: u32) -> eframe::egui::IconData {
    eframe::egui::IconData {
        rgba: rgba(size),
        width: size,
        height: size,
    }
}

/// Four subsamples per axis, so each pixel is a 16-sample coverage.
const SUBSAMPLES: u32 = 4;

#[allow(clippy::too_many_arguments)]
fn draw_rgba(
    size: u32,
    background: [u8; 4],
    foreground: [u8; 4],
    dot: Option<[u8; 4]>,
    arcs: Option<[u8; 4]>,
    state: Option<(GlyphShape, [u8; 4])>,
) -> Vec<u8> {
    let mark = mark();
    let bands = arc_plan(size);
    let mut out = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            let mut red = 0u32;
            let mut green = 0u32;
            let mut blue = 0u32;
            let mut alpha = 0u32;
            for sy in 0..SUBSAMPLES {
                for sx in 0..SUBSAMPLES {
                    let u = (x as f64 + (sx as f64 + 0.5) / SUBSAMPLES as f64) * 64.0 / size as f64;
                    let v = (y as f64 + (sy as f64 + 0.5) / SUBSAMPLES as f64) * 64.0 / size as f64;
                    let Some(px) =
                        sample(u, v, mark, &bands, background, foreground, dot, arcs, state)
                    else {
                        continue;
                    };
                    let a = px[3] as u32;
                    red += px[0] as u32 * a;
                    green += px[1] as u32 * a;
                    blue += px[2] as u32 * a;
                    alpha += a;
                }
            }
            let total = SUBSAMPLES * SUBSAMPLES;
            let out_alpha = ((alpha + total / 2) / total) as u8;
            let (r, g, b) = if alpha == 0 {
                (0, 0, 0)
            } else {
                (
                    (red + alpha / 2).checked_div(alpha).unwrap_or(0) as u8,
                    (green + alpha / 2).checked_div(alpha).unwrap_or(0) as u8,
                    (blue + alpha / 2).checked_div(alpha).unwrap_or(0) as u8,
                )
            };
            out.extend_from_slice(&[r, g, b, out_alpha]);
        }
    }
    out
}

#[allow(clippy::too_many_arguments)]
fn sample(
    u: f64,
    v: f64,
    mark: &jafnify_core::icon::Mark,
    bands: &[(f64, Vec<(f64, f64)>)],
    background: [u8; 4],
    foreground: [u8; 4],
    dot: Option<[u8; 4]>,
    arcs: Option<[u8; 4]>,
    state: Option<(GlyphShape, [u8; 4])>,
) -> Option<[u8; 4]> {
    if !inside_rounded(u, v, 2.0, 2.0, 62.0, 62.0, 13.0) {
        return None;
    }
    let mut px = background;
    if near_polyline(&mark.body.points, u, v) <= mark.body.width / 2.0 {
        px = foreground;
    }
    if let Some(colour) = arcs {
        if bands
            .iter()
            .any(|(width, points)| near_polyline(points, u, v) <= width / 2.0)
        {
            px = colour;
        }
    }
    if let Some((shape, colour)) = state {
        if in_shape(shape, u, v) {
            px = colour;
        }
    }
    if let Some(colour) = dot {
        let distance = (u - 53.0).powi(2) + (v - 53.0).powi(2);
        if distance <= 8.0 * 8.0 {
            px = colour;
        } else if distance <= 10.0 * 10.0 {
            px = background;
        }
    }
    Some(px)
}

fn near_polyline(points: &[(f64, f64)], x: f64, y: f64) -> f64 {
    let mut best = f64::INFINITY;
    for pair in points.windows(2) {
        let d = distance_to_segment(x, y, pair[0].0, pair[0].1, pair[1].0, pair[1].1);
        if d < best {
            best = d;
        }
    }
    best
}

fn to_argb(rgba: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(rgba.len());
    for px in rgba.chunks_exact(4) {
        out.extend_from_slice(&[px[3], px[0], px[1], px[2]]);
    }
    out
}

fn inside_rounded(x: f64, y: f64, x0: f64, y0: f64, x1: f64, y1: f64, r: f64) -> bool {
    if x < x0 || x > x1 || y < y0 || y > y1 {
        return false;
    }
    let cx = x.clamp(x0 + r, x1 - r);
    let cy = y.clamp(y0 + r, y1 - r);
    (x - cx).powi(2) + (y - cy).powi(2) <= r * r
}
