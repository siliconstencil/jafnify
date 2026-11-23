//! The selected Role/Theme choice was unreadable (fg text on the light
//! `primary` fill in the dark theme). Selected choices use `primary_on`;
//! pin that it is readable (WCAG AA, 4.5:1) in both themes.

use jafnify_linux::palette::Colours;
use jafnify_linux::theme::Theme;

fn luminance(c: eframe::egui::Color32) -> f64 {
    let ch = |v: u8| {
        let s = v as f64 / 255.0;
        if s <= 0.03928 {
            s / 12.92
        } else {
            ((s + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * ch(c.r()) + 0.7152 * ch(c.g()) + 0.0722 * ch(c.b())
}

fn contrast(a: eframe::egui::Color32, b: eframe::egui::Color32) -> f64 {
    let (x, y) = (luminance(a), luminance(b));
    (x.max(y) + 0.05) / (x.min(y) + 0.05)
}

#[test]
fn selected_choice_text_is_readable_in_both_themes() {
    for theme in [Theme::Dark, Theme::Light] {
        let c = Colours::for_theme(theme);
        let ratio = contrast(c.primary_on, c.primary);
        assert!(
            ratio >= 4.5,
            "{theme:?}: primary_on on primary is {ratio:.2}:1"
        );
    }
}

#[test]
fn the_old_dark_theme_pairing_was_unreadable() {
    // Documents the bug this guards: fg on primary in the dark theme.
    let c = Colours::for_theme(Theme::Dark);
    assert!(contrast(c.fg, c.primary) < 2.0);
}
