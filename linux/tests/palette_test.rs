//! The Linux palette is exactly the core design tokens, and the egui visuals
//! use it (bg for panels, fg for text, muted for weak text, line for strokes).

use eframe::egui::Color32;
use jafnify_core::tokens::{self, Palette};
use jafnify_linux::palette::{rgba, Colours};
use jafnify_linux::theme::{Theme, ThemeMode};

fn rgb(colour: Color32) -> u32 {
    ((colour.r() as u32) << 16) | ((colour.g() as u32) << 8) | colour.b() as u32
}

fn role(colours: &Colours, name: &str) -> Color32 {
    match name {
        "bg" => colours.bg,
        "surface" => colours.surface,
        "fg" => colours.fg,
        "muted" => colours.muted,
        "line" => colours.line,
        "primary" => colours.primary,
        "primary_on" => colours.primary_on,
        "accent" => colours.accent,
        "ok" => colours.ok,
        "wait" => colours.wait,
        "wait_text" => colours.wait_text,
        "err" => colours.err,
        other => panic!("unknown role {other}"),
    }
}

fn assert_palette(colours: &Colours, palette: &Palette) {
    for (name, value) in palette.entries() {
        assert_eq!(rgb(role(colours, name)), value, "role {name}");
    }
}

#[test]
fn the_light_and_dark_colours_are_the_core_tokens() {
    assert_palette(&Colours::light(), &tokens::light());
    assert_palette(&Colours::dark(), &tokens::dark());
}

#[test]
fn a_resolved_theme_selects_its_palette() {
    assert_eq!(Colours::for_theme(Theme::Light), Colours::light());
    assert_eq!(Colours::for_theme(Theme::Dark), Colours::dark());
    assert_eq!(
        Colours::for_mode(ThemeMode::System, Theme::Light),
        Colours::light()
    );
    assert_eq!(
        Colours::for_mode(ThemeMode::System, Theme::Dark),
        Colours::dark()
    );
    assert_eq!(
        Colours::for_mode(ThemeMode::Dark, Theme::Light),
        Colours::dark(),
        "an explicit mode wins over the resolved theme"
    );
}

#[test]
fn the_egui_visuals_use_the_palette() {
    for theme in [Theme::Light, Theme::Dark] {
        let colours = Colours::for_theme(theme);
        let visuals = theme.visuals();
        assert_eq!(visuals.panel_fill, colours.bg, "{theme:?} panel");
        assert_eq!(visuals.window_fill, colours.bg, "{theme:?} window");
        assert_eq!(
            visuals.override_text_color,
            Some(colours.fg),
            "{theme:?} text"
        );
        assert_eq!(
            visuals.widgets.inactive.bg_fill, colours.surface,
            "{theme:?} widget fill"
        );
        assert_eq!(
            visuals.widgets.noninteractive.fg_stroke.color, colours.muted,
            "{theme:?} weak text"
        );
        assert_eq!(
            visuals.window_stroke.color, colours.line,
            "{theme:?} stroke"
        );
    }
}

#[test]
fn the_mark_uses_the_light_accent_and_rgba_keeps_the_alpha() {
    let accent = Colours::light().accent;
    assert_eq!(rgb(accent), tokens::light().accent);
    assert_eq!(rgba(accent), [accent.r(), accent.g(), accent.b(), 255]);
}
