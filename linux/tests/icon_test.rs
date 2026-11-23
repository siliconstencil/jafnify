//! The shared J icon: one shape for the window, tray and launcher.
//!
//! The state glyphs are checked by shape, not colour: every shape is drawn in
//! the same colour and the resulting masks must differ.

use std::collections::HashSet;

use jafnify_core::snapshot::GlyphShape;
use jafnify_core::tokens;
use jafnify_linux::icon::{self, accent, argb, rgba, state_argb, WHITE};
use jafnify_linux::palette::{rgba as to_rgba, Colours};

fn rgba_pixel(buf: &[u8], size: u32, x: u32, y: u32) -> [u8; 4] {
    let i = ((y * size + x) * 4) as usize;
    [buf[i], buf[i + 1], buf[i + 2], buf[i + 3]]
}

/// StatusNotifierItem uses ARGB32 in network byte order: A, R, G, B.
fn argb_pixel(buf: &[u8], size: u32, x: u32, y: u32) -> [u8; 4] {
    let i = ((y * size + x) * 4) as usize;
    [buf[i + 1], buf[i + 2], buf[i + 3], buf[i]]
}

#[test]
fn the_window_icon_is_the_mark_on_graphite() {
    let size = 64;
    let buf = rgba(size);
    let dark = Colours::dark();
    assert_eq!(buf.len(), (size * size * 4) as usize);
    assert_eq!(
        rgba_pixel(&buf, size, 42, 20),
        to_rgba(dark.fg),
        "the J uses the near-white dark foreground"
    );
    assert_eq!(
        rgba_pixel(&buf, size, 0, 0),
        [0, 0, 0, 0],
        "outside the rounded square"
    );
    assert_eq!(
        rgba_pixel(&buf, size, 32, 8),
        to_rgba(dark.surface),
        "the neutral graphite tile"
    );
    let arc = to_rgba(dark.accent);
    assert!(
        buf.chunks_exact(4).any(|px| px == arc),
        "the product mark carries the accent arcs"
    );
    let light = tokens::light();
    assert_eq!(
        ((accent()[0] as u32) << 16) | ((accent()[1] as u32) << 8) | accent()[2] as u32,
        light.accent,
        "the accent helper still names the token accent"
    );
}

#[test]
fn the_rasteriser_antialiases_the_mark() {
    let size = 32;
    let buf = rgba(size);
    let dark = Colours::dark();
    let tile = to_rgba(dark.surface);
    let glyph = to_rgba(dark.fg);
    let mixed = buf.chunks_exact(4).any(|px| {
        let rgb = [px[0], px[1], px[2]];
        px[3] == 255
            && rgb != [tile[0], tile[1], tile[2]]
            && rgb != [glyph[0], glyph[1], glyph[2]]
            && rgb
                != [
                    to_rgba(dark.accent)[0],
                    to_rgba(dark.accent)[1],
                    to_rgba(dark.accent)[2],
                ]
    });
    assert!(
        mixed,
        "a J edge is a blend of glyph and tile, not a hard step"
    );
}

#[test]
fn the_legacy_argb_dot_is_still_drawable() {
    let size = 32;
    let connected = jafnify_linux::tray::TrayVisual::Connected.color();
    let buf = argb(size, accent(), Some(connected));
    let dot = argb_pixel(&buf, size, 26, 26);
    assert_eq!(dot, connected, "the optional dot");
    let stem = argb_pixel(&buf, size, 20, 9);
    assert_eq!(stem, WHITE, "the J stays readable");
    assert_eq!(
        argb_pixel(&buf, size, 0, 0)[3],
        0,
        "transparent outside the icon"
    );
}

#[test]
fn every_state_shape_is_distinct_at_every_size() {
    let shapes = [
        GlyphShape::NoArcHollowDot,
        GlyphShape::OneArcHollowDot,
        GlyphShape::OneArcFilledDot,
        GlyphShape::TwoArcsFilledDot,
        GlyphShape::OneArcHollowDotSlash,
        GlyphShape::Exclamation,
    ];
    let colour = [255, 0, 255, 255];
    for size in [16, 22, 32, 48] {
        let mut seen: HashSet<Vec<u8>> = HashSet::new();
        for shape in shapes {
            let buf = state_argb(size, shape, colour);
            assert_eq!(buf.len(), (size * size * 4) as usize);
            assert!(
                seen.insert(buf.clone()),
                "{:?} is the same pixels as another shape at {size} px",
                shape
            );
        }
        assert_eq!(seen.len(), shapes.len(), "all shapes differ at {size} px");
    }
}

#[test]
fn the_tray_states_use_the_graphite_tile_and_the_dark_palette() {
    let size = 32;
    let dark = Colours::dark();
    let colour = to_rgba(dark.ok);
    let buf = state_argb(size, GlyphShape::TwoArcsFilledDot, colour);
    assert_eq!(
        argb_pixel(&buf, size, 0, 0)[3],
        0,
        "transparent outside the icon"
    );
    assert_eq!(
        argb_pixel(&buf, size, 16, 4),
        to_rgba(dark.surface),
        "the tile stays neutral graphite"
    );
    assert_eq!(
        argb_pixel(&buf, size, 19, 17),
        to_rgba(dark.fg),
        "the J stays near-white"
    );
    assert!(
        buf.chunks_exact(4).any(|px| px[1..] == colour[..3]),
        "the state colour reaches the glyph"
    );
}

#[test]
fn the_svg_sources_and_desktop_entry_exist() {
    for path in [
        "assets/icon/jafnify.svg",
        "assets/icon/jafnify-symbolic.svg",
        "assets/linux/jafnify.desktop",
    ] {
        let text = std::fs::read_to_string(format!("../{path}")).unwrap();
        assert!(!text.is_empty(), "{path}");
    }
    let svg = std::fs::read_to_string("../assets/icon/jafnify.svg").unwrap();
    assert!(
        svg.contains("M40 16 V36"),
        "the Linux mark follows the SVG stroked J"
    );
    assert!(
        svg.contains(" A10 10 ") && svg.contains(" A17 17 "),
        "the SVG carries the two signal arcs"
    );
    assert!(!svg.contains("#E65100"), "the old ember tile is gone");
    let dark = tokens::dark();
    let surface = format!("#{:06X}", dark.role("surface").unwrap());
    let fg = format!("#{:06X}", dark.role("fg").unwrap());
    let accent = format!("#{:06X}", dark.role("accent").unwrap());
    assert!(
        svg.contains(&surface),
        "the SVG tile is the dark surface token"
    );
    assert!(svg.contains(&fg), "the SVG J is the dark fg token");
    assert!(svg.contains(&accent), "the arcs are the accent token");
    assert!(
        !svg.contains(&format!("fill=\"{accent}\"")),
        "the accent is the arc stroke, never a fill"
    );
    let desktop = std::fs::read_to_string("../assets/linux/jafnify.desktop").unwrap();
    assert!(desktop.contains("Icon=jafnify"));
    assert!(desktop.contains("Exec=/usr/local/bin/jafnify-gui"));
    assert!(
        desktop.contains("StartupWMClass=jafnify"),
        "KDE Wayland maps the window by the lowercase app id"
    );
    let gui = std::fs::read_to_string("src/bin/jafnify-gui.rs").unwrap();
    // The app id comes from the flavor; in the product build it is the
    // desktop entry's StartupWMClass.
    assert!(
        gui.contains(".with_app_id(jafnify_linux::flavor::NAME)"),
        "Wayland ignores with_icon; app_id must match the desktop entry"
    );
    if !jafnify_linux::flavor::IS_TEST {
        assert_eq!(jafnify_linux::flavor::NAME, "jafnify");
    }
    assert!(
        icon::MARK_SVG.is_ascii(),
        "the rasteriser path data is ASCII"
    );
}

#[test]
fn the_tray_arcs_take_the_state_colour() {
    let size = 32;
    let dark = Colours::dark();
    let colour = to_rgba(dark.err);
    let buf = state_argb(size, GlyphShape::Exclamation, colour);
    assert!(
        buf.chunks_exact(4)
            .filter(|px| px[1..] == colour[..3])
            .count()
            > 0,
        "the arcs and the error glyph are the state colour"
    );
    let window = rgba(size);
    let accent = to_rgba(dark.accent);
    assert!(
        window.chunks_exact(4).any(|px| px[..3] == accent[..3]),
        "the window arcs are the accent token"
    );
}
