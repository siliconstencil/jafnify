// linux/tests/theme_test.rs
use std::fs;
use std::path::{Path, PathBuf};

use jafnify_linux::palette::{rgba, Colours};
use jafnify_linux::theme::{metrics, resolve, Theme, ThemeMode};

fn kde_file(dir: &Path, name: &str, body: &str) -> PathBuf {
    let path = dir.join(name);
    fs::write(&path, body).unwrap();
    path
}

#[test]
fn theme_mode_defaults_to_system() {
    assert_eq!(ThemeMode::default(), ThemeMode::System);
}

#[test]
fn theme_mode_is_lowercase_in_json() {
    assert_eq!(serde_json::to_string(&ThemeMode::Dark).unwrap(), "\"dark\"");
    assert_eq!(
        serde_json::from_str::<ThemeMode>("\"light\"").unwrap(),
        ThemeMode::Light
    );
}

#[test]
fn explicit_modes_do_not_consult_the_portal_or_kde() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("missing-kdeglobals");
    assert_eq!(resolve(ThemeMode::Dark, Some(2), &missing), Theme::Dark);
    assert_eq!(resolve(ThemeMode::Light, Some(1), &missing), Theme::Light);
}

#[test]
fn system_uses_portal_one_for_dark_and_two_for_light() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("missing-kdeglobals");
    assert_eq!(resolve(ThemeMode::System, Some(1), &missing), Theme::Dark);
    assert_eq!(resolve(ThemeMode::System, Some(2), &missing), Theme::Light);
}

#[test]
fn system_with_no_portal_preference_falls_back_to_kde() {
    let dir = tempfile::tempdir().unwrap();
    let dark = kde_file(
        dir.path(),
        "dark-kdeglobals",
        "[General]\nColorScheme=BreezeDark\n",
    );
    let light = kde_file(
        dir.path(),
        "light-kdeglobals",
        "[General]\nColorScheme=BreezeLight\n",
    );
    assert_eq!(resolve(ThemeMode::System, Some(0), &dark), Theme::Dark);
    assert_eq!(resolve(ThemeMode::System, None, &light), Theme::Light);
}

#[test]
fn kde_color_scheme_match_is_case_insensitive() {
    let dir = tempfile::tempdir().unwrap();
    let dark = kde_file(
        dir.path(),
        "dark-kdeglobals",
        "[General]\nColorScheme=MyCustomDARKTheme\n",
    );
    let light = kde_file(
        dir.path(),
        "light-kdeglobals",
        "[General]\nColorScheme=MyCustomLightTheme\n",
    );
    assert_eq!(resolve(ThemeMode::System, None, &dark), Theme::Dark);
    assert_eq!(resolve(ThemeMode::System, None, &light), Theme::Light);
}

#[test]
fn kde_background_luminance_is_the_secondary_fallback() {
    let dir = tempfile::tempdir().unwrap();
    let dark = kde_file(
        dir.path(),
        "dark-kdeglobals",
        "[Colors:Window]\nBackgroundNormal=20,22,26\n",
    );
    let light = kde_file(
        dir.path(),
        "light-kdeglobals",
        "[Colors:Window]\nBackgroundNormal=250,250,250\n",
    );
    assert_eq!(resolve(ThemeMode::System, Some(0), &dark), Theme::Dark);
    assert_eq!(resolve(ThemeMode::System, Some(0), &light), Theme::Light);
}

#[test]
fn kde_color_scheme_without_dark_or_light_uses_background_luminance() {
    let dir = tempfile::tempdir().unwrap();
    let dark = kde_file(
        dir.path(),
        "dark-kdeglobals",
        "[General]\nColorScheme=Custom\n[Colors:Window]\nBackgroundNormal=10,10,10\n",
    );
    let light = kde_file(
        dir.path(),
        "light-kdeglobals",
        "[General]\nColorScheme=Custom\n[Colors:Window]\nBackgroundNormal=240,240,240\n",
    );
    assert_eq!(resolve(ThemeMode::System, Some(0), &dark), Theme::Dark);
    assert_eq!(resolve(ThemeMode::System, Some(0), &light), Theme::Light);
}

#[test]
fn malformed_or_missing_kde_defaults_to_dark() {
    let dir = tempfile::tempdir().unwrap();
    let malformed = kde_file(
        dir.path(),
        "malformed-kdeglobals",
        "[General]\nColorScheme = ???\n[Colors:Window]\nBackgroundNormal = not,rgb\n",
    );
    assert_eq!(resolve(ThemeMode::System, None, &malformed), Theme::Dark);
    assert_eq!(
        resolve(ThemeMode::System, None, &dir.path().join("missing")),
        Theme::Dark
    );
}

#[test]
fn explicit_mode_wins_over_portal_and_kde() {
    let dir = tempfile::tempdir().unwrap();
    let light = kde_file(
        dir.path(),
        "light-kdeglobals",
        "[General]\nColorScheme=BreezeLight\n",
    );
    assert_eq!(resolve(ThemeMode::Dark, Some(2), &light), Theme::Dark);
    assert_eq!(resolve(ThemeMode::Light, Some(1), &light), Theme::Light);
}

#[test]
fn visuals_match_the_resolved_theme() {
    assert!(Theme::Dark.visuals().dark_mode);
    assert!(!Theme::Light.visuals().dark_mode);
}

#[test]
fn the_card_frame_uses_the_surface_line_and_elevation_tokens() {
    let m = metrics();
    for theme in [Theme::Light, Theme::Dark] {
        let colours = Colours::for_theme(theme);
        let frame = theme.card_frame();
        assert_eq!(frame.fill, colours.surface, "{theme:?} card fill");
        assert_eq!(frame.stroke.color, colours.line, "{theme:?} card border");
        assert_eq!(
            frame.corner_radius.nw as f32, m.radius.lg,
            "{theme:?} card radius"
        );
        assert_eq!(
            frame.inner_margin.left as f32, m.spacing.lg,
            "{theme:?} card padding"
        );
        assert_eq!(
            frame.shadow.offset[1] as f32, m.elevation.low,
            "{theme:?} card elevation"
        );
    }
    assert_eq!(rgba(Colours::light().surface), [255, 255, 255, 255]);
}

#[test]
fn the_linux_metrics_are_the_generated_scales() {
    let m = metrics();
    assert_eq!(
        [
            m.spacing.xs,
            m.spacing.sm,
            m.spacing.md,
            m.spacing.lg,
            m.spacing.xl,
        ],
        [4.0, 8.0, 12.0, 16.0, 24.0]
    );
    assert_eq!(
        [
            m.type_scale.caption,
            m.type_scale.body,
            m.type_scale.body_large,
            m.type_scale.title,
            m.type_scale.display,
        ],
        [11.0, 14.0, 17.0, 22.0, 34.0]
    );
    assert_eq!(
        [
            m.line_height.caption,
            m.line_height.body,
            m.line_height.body_large,
            m.line_height.title,
            m.line_height.display,
        ],
        [16.0, 20.0, 24.0, 28.0, 40.0]
    );
    assert_eq!([m.radius.sm, m.radius.md, m.radius.lg], [8.0, 12.0, 16.0]);
    assert_eq!(
        [m.elevation.none, m.elevation.low, m.elevation.high],
        [0.0, 1.0, 2.0]
    );
}
