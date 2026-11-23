//! The contrast of the text the laptop window draws, computed from the same
//! palette the window uses. The tile facts and captions are `muted` on the
//! card `surface`; a body row is `fg` on `surface`. Both must clear WCAG AA
//! for normal text (4.5:1) in the dark and the light theme.

use jafnify_core::tokens::{contrast_ratio, dark, light, Palette, BODY_TEXT_MIN_RATIO};

fn check(palette: Palette, theme: &str) {
    for (label, fg, bg) in [
        ("facts and captions on a tile", palette.muted, palette.surface),
        ("facts and captions on the page", palette.muted, palette.bg),
        ("a value on a tile", palette.fg, palette.surface),
        ("a value on the page", palette.fg, palette.bg),
        ("a warning value", palette.wait_text, palette.surface),
        ("the primary button label", palette.primary, palette.primary_on),
    ] {
        let ratio = contrast_ratio(fg, bg);
        assert!(
            ratio >= BODY_TEXT_MIN_RATIO,
            "{theme}: {label} is {ratio:.2}:1, below {}:1",
            BODY_TEXT_MIN_RATIO
        );
    }
}

#[test]
fn the_dark_tile_text_clears_wcag_aa() {
    check(dark(), "dark");
}

#[test]
fn the_light_tile_text_clears_wcag_aa() {
    check(light(), "light");
}

#[test]
fn the_muted_tile_fact_is_well_above_the_floor() {
    // The reported dimness is the tiny size and antialiasing, not the
    // contrast: the dark muted on surface is about 6.75:1.
    let ratio = contrast_ratio(dark().muted, dark().surface);
    assert!(ratio > 6.0, "dark muted/surface is {ratio:.2}:1");
}
