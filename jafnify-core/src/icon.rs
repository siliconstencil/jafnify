//! The J-Signal mark geometry: an SVG parser and the state glyph shapes.
//!
//! The SVG is the single source of the geometry; this module understands the
//! command subset it uses (M, H, V, L, C, A, Z), so every frontend draws the
//! same mark as the Android vectors. It is pure geometry: colours, egui and
//! tray rasters stay with the platform.

use std::sync::OnceLock;

use crate::snapshot::GlyphShape;

/// The single source of the mark, embedded so the binary needs no asset path.
pub const MARK_SVG: &str = include_str!("../../assets/icon/jafnify.svg");

/// An SVG arc in the 64-unit design space.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ArcGeom {
    pub centre: (f64, f64),
    pub radius: f64,
    pub start: f64,
    pub delta: f64,
}

/// One stroked path from the SVG, flattened to a polyline.
#[derive(Debug, Clone)]
pub struct MarkPath {
    pub points: Vec<(f64, f64)>,
    pub width: f64,
    pub arc: Option<ArcGeom>,
}

/// The parsed mark: the J body plus the signal arcs.
#[derive(Debug, Clone)]
pub struct Mark {
    pub body: MarkPath,
    pub arcs: Vec<MarkPath>,
}

/// The parsed SVG. Parsed once; the geometry cannot change at runtime.
pub fn mark() -> &'static Mark {
    static MARK: OnceLock<Mark> = OnceLock::new();
    MARK.get_or_init(|| parse_mark(MARK_SVG))
}

/// The arcs to draw at `size`: the parsed strokes at >= 32 px, and the small
/// variant below that (one arc at the mid radius, stroke 6), so the two arcs
/// never merge into a blob on a 16/22 px tray.
pub fn arc_plan(size: u32) -> Vec<(f64, Vec<(f64, f64)>)> {
    let mark = mark();
    if size < 32 && mark.arcs.len() >= 2 {
        let inner = &mark.arcs[0];
        let outer = &mark.arcs[1];
        if let (Some(a), Some(b)) = (inner.arc, outer.arc) {
            let radius = (a.radius + b.radius) / 2.0;
            let steps = 24;
            let mut points = Vec::with_capacity(steps + 1);
            for step in 0..=steps {
                let angle = a.start + a.delta * step as f64 / steps as f64;
                points.push((
                    a.centre.0 + radius * angle.cos(),
                    a.centre.1 + radius * angle.sin(),
                ));
            }
            return vec![(6.0, points)];
        }
    }
    mark.arcs
        .iter()
        .filter_map(|path| {
            if path.points.is_empty() {
                None
            } else {
                Some((path.width, path.points.clone()))
            }
        })
        .collect()
}

/// The geometry of the state glyph in the 64-unit viewBox.
const ARC_CENTRE: (f64, f64) = (40.0, 16.0);
const DOT_RADIUS: f64 = 3.4;
const RING_OUTER: f64 = 4.2;
const RING_INNER: f64 = 2.2;
const HALO_OUTER: f64 = 7.6;
const HALO_INNER: f64 = 6.0;

/// The state vocabulary's glyph. The signal arcs are always drawn (in the
/// state colour), so the glyphs below only have to stay distinct from each
/// other: ring, dot, halo and slash in different combinations.
pub fn in_shape(shape: GlyphShape, u: f64, v: f64) -> bool {
    match shape {
        GlyphShape::NoArcHollowDot => in_ring(u, v),
        GlyphShape::OneArcHollowDot => in_ring(u, v) || in_dot(u, v),
        GlyphShape::OneArcFilledDot => in_dot(u, v),
        GlyphShape::TwoArcsFilledDot => in_dot(u, v) || in_halo(u, v),
        GlyphShape::OneArcHollowDotSlash => in_ring(u, v) || in_slash(u, v),
        GlyphShape::Exclamation => in_exclamation(u, v),
    }
}

fn in_dot(u: f64, v: f64) -> bool {
    (u - ARC_CENTRE.0).powi(2) + (v - ARC_CENTRE.1).powi(2) <= DOT_RADIUS * DOT_RADIUS
}

fn in_ring(u: f64, v: f64) -> bool {
    let distance = ((u - ARC_CENTRE.0).powi(2) + (v - ARC_CENTRE.1).powi(2)).sqrt();
    (RING_INNER..=RING_OUTER).contains(&distance)
}

/// A wider halo between the dot and the inner signal arc.
fn in_halo(u: f64, v: f64) -> bool {
    let distance = ((u - ARC_CENTRE.0).powi(2) + (v - ARC_CENTRE.1).powi(2)).sqrt();
    (HALO_INNER..=HALO_OUTER).contains(&distance)
}

/// A slash from the upper left to the lower right across the dot.
fn in_slash(u: f64, v: f64) -> bool {
    let (x0, y0) = (ARC_CENTRE.0 - 6.0, ARC_CENTRE.1 - 6.0);
    let (x1, y1) = (ARC_CENTRE.0 + 6.0, ARC_CENTRE.1 + 6.0);
    distance_to_segment(u, v, x0, y0, x1, y1) <= 1.2
}

/// The error glyph: a bar and a dot, to the right of the J.
fn in_exclamation(u: f64, v: f64) -> bool {
    let bar = (50.5..=53.5).contains(&u) && (12.0..=20.0).contains(&v);
    let dot = (u - 52.0).powi(2) + (v - 24.0).powi(2) <= 1.9 * 1.9;
    bar || dot
}

pub fn distance_to_segment(x: f64, y: f64, x0: f64, y0: f64, x1: f64, y1: f64) -> f64 {
    let dx = x1 - x0;
    let dy = y1 - y0;
    let length = dx * dx + dy * dy;
    if length == 0.0 {
        return ((x - x0).powi(2) + (y - y0).powi(2)).sqrt();
    }
    let t = (((x - x0) * dx + (y - y0) * dy) / length).clamp(0.0, 1.0);
    ((x - (x0 + t * dx)).powi(2) + (y - (y0 + t * dy)).powi(2)).sqrt()
}

// ------------------------------------------------------------------ SVG ----

/// Parse the mark out of the SVG: the first `<path>` is the J body, the rest
/// are the signal arcs.
fn parse_mark(svg: &str) -> Mark {
    let mut paths = Vec::new();
    let mut rest = svg;
    while let Some(start) = rest.find("<path") {
        let tail = &rest[start..];
        let end = tail.find("/>").unwrap_or(tail.len());
        let tag = &tail[..end];
        let d = attribute(tag, "d").unwrap_or("");
        let width = attribute(tag, "stroke-width")
            .and_then(|value| value.trim().parse::<f64>().ok())
            .unwrap_or(7.0);
        let (points, arc) = flatten_path(d);
        paths.push(MarkPath { points, width, arc });
        rest = &tail[end..];
    }
    let mut iter = paths.into_iter();
    let body = iter.next().expect("the SVG carries the J path");
    Mark {
        body,
        arcs: iter.collect(),
    }
}

/// `name="value"` from a tag, with or without a leading space.
fn attribute<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let needle = format!(" {name}=\"");
    let start = tag.find(&needle).map(|i| i + needle.len())?;
    let end = tag[start..].find('"')? + start;
    Some(&tag[start..end])
}

enum Token {
    Command(char),
    Number(f64),
}

fn path_tokens(path: &str) -> Vec<Token> {
    let mut tokens = Vec::new();
    let mut chars = path.chars().peekable();
    while let Some(&c) = chars.peek() {
        if c.is_ascii_whitespace() || c == ',' {
            chars.next();
        } else if c.is_ascii_alphabetic() {
            tokens.push(Token::Command(c));
            chars.next();
        } else {
            let mut number = String::new();
            if c == '-' {
                number.push(c);
                chars.next();
            }
            while let Some(&digit) = chars.peek() {
                if digit.is_ascii_digit() || digit == '.' {
                    number.push(digit);
                    chars.next();
                } else {
                    break;
                }
            }
            if let Ok(value) = number.parse() {
                tokens.push(Token::Number(value));
            }
        }
    }
    tokens
}

/// Flatten the path into a polyline, returning the last arc geometry when the
/// path contains an `A` command.
fn flatten_path(path: &str) -> (Vec<(f64, f64)>, Option<ArcGeom>) {
    let tokens = path_tokens(path);
    let mut points = Vec::new();
    let mut current = (0.0, 0.0);
    let mut start = (0.0, 0.0);
    let mut arc = None;
    let mut i = 0;
    while i < tokens.len() {
        let Token::Command(command) = tokens[i] else {
            i += 1;
            continue;
        };
        i += 1;
        let number = |index: usize| -> f64 {
            match tokens.get(index) {
                Some(Token::Number(value)) => *value,
                _ => 0.0,
            }
        };
        match command {
            'M' => {
                current = (number(i), number(i + 1));
                i += 2;
                start = current;
                points.push(current);
            }
            'H' => {
                current.0 = number(i);
                i += 1;
                points.push(current);
            }
            'V' => {
                current.1 = number(i);
                i += 1;
                points.push(current);
            }
            'L' => {
                current = (number(i), number(i + 1));
                i += 2;
                points.push(current);
            }
            'C' => {
                let c1 = (number(i), number(i + 1));
                let c2 = (number(i + 2), number(i + 3));
                let end = (number(i + 4), number(i + 5));
                i += 6;
                flatten_cubic(current, c1, c2, end, 16, &mut points);
                current = end;
            }
            'A' => {
                let rx = number(i);
                let ry = number(i + 1);
                let large = number(i + 3) != 0.0;
                let sweep = number(i + 4) != 0.0;
                let end = (number(i + 5), number(i + 6));
                i += 7;
                let (mut arc_points, geometry) = arc_points(current, rx, ry, large, sweep, end, 24);
                points.append(&mut arc_points);
                current = end;
                arc = Some(geometry);
            }
            'Z' | 'z' => {
                points.push(start);
                current = start;
            }
            _ => {}
        }
    }
    (points, arc)
}

/// SVG endpoint parameterisation, equal radii, no x-axis rotation (the mark's
/// arcs satisfy all three). Returns the sampled polyline and the geometry.
#[allow(clippy::too_many_arguments)]
fn arc_points(
    from: (f64, f64),
    rx: f64,
    ry: f64,
    large: bool,
    sweep: bool,
    to: (f64, f64),
    steps: usize,
) -> (Vec<(f64, f64)>, ArcGeom) {
    let (mut rx, mut ry) = (rx.abs(), ry.abs());
    let half = ((from.0 - to.0) / 2.0, (from.1 - to.1) / 2.0);
    let lambda = half.0 * half.0 / (rx * rx) + half.1 * half.1 / (ry * ry);
    if lambda > 1.0 {
        let scale = lambda.sqrt();
        rx *= scale;
        ry *= scale;
    }
    let num = rx * rx * ry * ry - rx * rx * half.1 * half.1 - ry * ry * half.0 * half.0;
    let den = rx * rx * half.1 * half.1 + ry * ry * half.0 * half.0;
    let mut factor = if den == 0.0 {
        0.0
    } else {
        (num / den).max(0.0).sqrt()
    };
    if large == sweep {
        factor = -factor;
    }
    let cx = factor * rx * half.1 / ry + (from.0 + to.0) / 2.0;
    let cy = -factor * ry * half.0 / rx + (from.1 + to.1) / 2.0;
    let u = (
        (half.0 - factor * rx * half.1 / ry) / rx,
        (half.1 + factor * ry * half.0 / rx) / ry,
    );
    let v = (
        (-half.0 - factor * rx * half.1 / ry) / rx,
        (-half.1 + factor * ry * half.0 / rx) / ry,
    );
    let start = u.1.atan2(u.0);
    let mut delta = (u.0 * v.1 - u.1 * v.0).atan2(u.0 * v.0 + u.1 * v.1);
    if !sweep && delta > 0.0 {
        delta -= std::f64::consts::TAU;
    } else if sweep && delta < 0.0 {
        delta += std::f64::consts::TAU;
    }
    let mut points = Vec::with_capacity(steps + 1);
    for step in 0..=steps {
        let angle = start + delta * step as f64 / steps as f64;
        points.push((cx + rx * angle.cos(), cy + ry * angle.sin()));
    }
    (
        points,
        ArcGeom {
            centre: (cx, cy),
            radius: rx,
            start,
            delta,
        },
    )
}

fn flatten_cubic(
    start: (f64, f64),
    c1: (f64, f64),
    c2: (f64, f64),
    end: (f64, f64),
    steps: usize,
    out: &mut Vec<(f64, f64)>,
) {
    for step in 1..=steps {
        let t = step as f64 / steps as f64;
        let u = 1.0 - t;
        let x = u * u * u * start.0
            + 3.0 * u * u * t * c1.0
            + 3.0 * u * t * t * c2.0
            + t * t * t * end.0;
        let y = u * u * u * start.1
            + 3.0 * u * u * t * c1.1
            + 3.0 * u * t * t * c2.1
            + t * t * t * end.1;
        out.push((x, y));
    }
}
