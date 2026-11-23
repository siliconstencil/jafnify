//! System/Dark/Light theme selection.
//!
//! `ThemeMode` is the saved preference. `resolve` turns it into the theme the
//! window draws: an explicit mode wins, System asks the freedesktop color
//! scheme portal first and falls back to the KDE color scheme file.
//! Resolution is a pure function so tests inject the portal value and file;
//! only `detect_portal_color_scheme` runs `gdbus`, once at window start.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

/// How long the portal query may take before the KDE fallback is used.
const PORTAL_TIMEOUT: Duration = Duration::from_millis(800);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ThemeMode {
    #[default]
    System,
    Dark,
    Light,
}

impl ThemeMode {
    /// Stable wire/state spelling (`system`, `dark`, `light`).
    pub fn as_str(self) -> &'static str {
        match self {
            ThemeMode::System => "system",
            ThemeMode::Dark => "dark",
            ThemeMode::Light => "light",
        }
    }

    /// Parse a control-channel value (trimmed, case-insensitive). Saved state
    /// uses the strict serde enum, so this is not used for the state file.
    /// Unknown values are rejected rather than silently becoming System.
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "system" => Some(ThemeMode::System),
            "dark" => Some(ThemeMode::Dark),
            "light" => Some(ThemeMode::Light),
            _ => None,
        }
    }
}

/// The theme actually drawn, after resolving System.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Theme {
    Dark,
    Light,
}

impl Theme {
    /// The egui visuals for this theme, built from the design tokens. Kept
    /// separate so the mapping can be asserted without an egui context.
    pub fn visuals(self) -> eframe::egui::Visuals {
        use eframe::egui::{Stroke, Visuals};

        let colours = crate::palette::Colours::for_theme(self);
        let mut visuals = match self {
            Theme::Dark => Visuals::dark(),
            Theme::Light => Visuals::light(),
        };
        visuals.panel_fill = colours.bg;
        visuals.window_fill = colours.bg;
        visuals.extreme_bg_color = colours.bg;
        visuals.faint_bg_color = colours.surface;
        visuals.override_text_color = Some(colours.fg);
        visuals.window_stroke = Stroke::new(1.0_f32, colours.line);
        for widget in [
            &mut visuals.widgets.noninteractive,
            &mut visuals.widgets.inactive,
            &mut visuals.widgets.hovered,
            &mut visuals.widgets.active,
        ] {
            widget.bg_fill = colours.surface;
            widget.weak_bg_fill = colours.surface;
            widget.bg_stroke = Stroke::new(1.0_f32, colours.line);
        }
        visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0_f32, colours.muted);
        visuals.selection.bg_fill = colours.primary;
        visuals.selection.stroke = Stroke::new(1.0_f32, colours.primary_on);
        visuals
    }

    /// The card's visible ground: the token `surface`, the `line` border and
    /// the low elevation, with the generated radius and padding.
    pub fn card_frame(self) -> eframe::egui::Frame {
        use eframe::egui::{Color32, CornerRadius, Margin, Stroke};

        let metrics = metrics();
        let colours = crate::palette::Colours::for_theme(self);
        eframe::egui::Frame {
            inner_margin: Margin::same(metrics.spacing.lg as i8),
            outer_margin: Margin::same(0),
            corner_radius: CornerRadius::same(metrics.radius.lg as u8),
            shadow: eframe::egui::epaint::Shadow {
                offset: [0, metrics.elevation.low as i8],
                blur: (metrics.elevation.low * 2.0) as u8,
                spread: 0,
                color: Color32::from_black_alpha(40),
            },
            fill: colours.surface,
            stroke: Stroke::new(1.0_f32, colours.line),
        }
    }
}

/// The spacing scale, in logical pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Spacing {
    pub xs: f32,
    pub sm: f32,
    pub md: f32,
    pub lg: f32,
    pub xl: f32,
}

/// The type scale, in logical pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TypeScale {
    pub caption: f32,
    pub body: f32,
    pub body_large: f32,
    pub title: f32,
    pub display: f32,
}

/// The line heights that go with [`TypeScale`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LineHeights {
    pub caption: f32,
    pub body: f32,
    pub body_large: f32,
    pub title: f32,
    pub display: f32,
}

/// The corner radii, in logical pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Radius {
    pub sm: f32,
    pub md: f32,
    pub lg: f32,
}

/// The elevation scale, in logical pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Elevation {
    pub none: f32,
    pub low: f32,
    pub high: f32,
}

/// Every non-colour design scale, as the Linux GUI draws them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Metrics {
    pub spacing: Spacing,
    pub type_scale: TypeScale,
    pub line_height: LineHeights,
    pub radius: Radius,
    pub elevation: Elevation,
}

/// The design scales from `design/tokens.toml`, in logical pixels.
///
/// The GUI takes spacing, type, radius and elevation from here instead of
/// writing its own numbers.
pub fn metrics() -> Metrics {
    use jafnify_core::tokens;

    Metrics {
        spacing: Spacing {
            xs: tokens::SPACING_XS as f32,
            sm: tokens::SPACING_SM as f32,
            md: tokens::SPACING_MD as f32,
            lg: tokens::SPACING_LG as f32,
            xl: tokens::SPACING_XL as f32,
        },
        type_scale: TypeScale {
            caption: tokens::TYPE_CAPTION as f32,
            body: tokens::TYPE_BODY as f32,
            body_large: tokens::TYPE_BODY_LARGE as f32,
            title: tokens::TYPE_TITLE as f32,
            display: tokens::TYPE_DISPLAY as f32,
        },
        line_height: LineHeights {
            caption: tokens::LINE_HEIGHT_CAPTION as f32,
            body: tokens::LINE_HEIGHT_BODY as f32,
            body_large: tokens::LINE_HEIGHT_BODY_LARGE as f32,
            title: tokens::LINE_HEIGHT_TITLE as f32,
            display: tokens::LINE_HEIGHT_DISPLAY as f32,
        },
        radius: Radius {
            sm: tokens::RADIUS_SM as f32,
            md: tokens::RADIUS_MD as f32,
            lg: tokens::RADIUS_LG as f32,
        },
        elevation: Elevation {
            none: tokens::ELEVATION_NONE as f32,
            low: tokens::ELEVATION_LOW as f32,
            high: tokens::ELEVATION_HIGH as f32,
        },
    }
}

/// Where the KDE color scheme usually lives. `dirs::config_dir()` is
/// `~/.config` on a normal desktop.
pub fn kdeglobals_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("kdeglobals")
}

/// Resolve a saved mode into the theme to draw.
///
/// Explicit Dark/Light always win. For System, portal `1` is dark, `2` is
/// light, and `0`/absent/malformed values fall back to KDE. A missing or
/// unreadable KDE file is not an error: it resolves to dark, the manual
/// target's default desktop.
pub fn resolve(mode: ThemeMode, portal_value: Option<u32>, kdeglobals: &Path) -> Theme {
    match mode {
        ThemeMode::Dark => Theme::Dark,
        ThemeMode::Light => Theme::Light,
        ThemeMode::System => match portal_value {
            Some(1) => Theme::Dark,
            Some(2) => Theme::Light,
            _ => kde_theme(kdeglobals),
        },
    }
}

/// Apply a resolved theme to the window. Called when the resolved theme
/// changes, not every frame.
pub fn apply(ctx: &eframe::egui::Context, theme: Theme) {
    ctx.set_visuals(theme.visuals());
}

/// Read the freedesktop color-scheme portal with an unprivileged `gdbus`.
/// Returns `None` when gdbus is missing, fails, times out, or prints an
/// unexpected value; `resolve` then falls back to KDE.
pub fn detect_portal_color_scheme() -> Option<u32> {
    let mut child = Command::new("gdbus")
        .args([
            "call",
            "--session",
            "--dest",
            "org.freedesktop.portal.Desktop",
            "--object-path",
            "/org/freedesktop/portal/desktop",
            "--method",
            "org.freedesktop.portal.Settings.Read",
            "org.freedesktop.appearance",
            "color-scheme",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;

    let deadline = Instant::now() + PORTAL_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(10));
            }
            Ok(None) | Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }

    let output = child.wait_with_output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    parse_portal_uint(&text)
}

/// `gdbus` prints `(<uint32 1>,)`; pull out the `uint32` that follows.
fn parse_portal_uint(text: &str) -> Option<u32> {
    let marker = "uint32";
    let start = text.find(marker)? + marker.len();
    let digits: String = text[start..]
        .chars()
        .skip_while(|c| c.is_whitespace())
        .take_while(|c| c.is_ascii_digit())
        .collect();
    if digits.is_empty() {
        return None;
    }
    digits.parse().ok()
}

/// Parse `kdeglobals`: `[General] ColorScheme` wins, then
/// `[Colors:Window] BackgroundNormal` luminance. Malformed input is dark.
fn kde_theme(path: &Path) -> Theme {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Theme::Dark;
    };

    let mut section = "";
    let mut color_scheme: Option<&str> = None;
    let mut background: Option<&str> = None;
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            section = name.trim();
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match (section, key.trim()) {
            ("General", "ColorScheme") => color_scheme = Some(value.trim()),
            ("Colors:Window", "BackgroundNormal") => background = Some(value.trim()),
            _ => {}
        }
    }

    if let Some(scheme) = color_scheme {
        let lower = scheme.to_ascii_lowercase();
        if lower.contains("dark") {
            return Theme::Dark;
        }
        if lower.contains("light") {
            return Theme::Light;
        }
    }
    if let Some(value) = background {
        if let Some(theme) = luminance_theme(value) {
            return theme;
        }
    }
    Theme::Dark
}

/// `BackgroundNormal=r,g,b`; a light background means a light scheme.
fn luminance_theme(value: &str) -> Option<Theme> {
    let mut parts = value.split(',');
    let r = parts.next()?.trim().parse::<u32>().ok()?;
    let g = parts.next()?.trim().parse::<u32>().ok()?;
    let b = parts.next()?.trim().parse::<u32>().ok()?;
    if parts.next().is_some() || r > 255 || g > 255 || b > 255 {
        return None;
    }
    let luminance = 0.2126 * r as f64 + 0.7152 * g as f64 + 0.0722 * b as f64;
    Some(if luminance >= 128.0 {
        Theme::Light
    } else {
        Theme::Dark
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn theme_mode_parse_trims_and_ignores_case() {
        assert_eq!(ThemeMode::parse(" system "), Some(ThemeMode::System));
        assert_eq!(ThemeMode::parse("DARK"), Some(ThemeMode::Dark));
        assert_eq!(ThemeMode::parse("Light"), Some(ThemeMode::Light));
    }

    #[test]
    fn theme_mode_parse_rejects_unknown_values() {
        assert_eq!(ThemeMode::parse(""), None);
        assert_eq!(ThemeMode::parse("purple"), None);
        assert_eq!(ThemeMode::parse("dark mode"), None);
    }

    #[test]
    fn parse_portal_uint_reads_gdbus_output() {
        assert_eq!(parse_portal_uint("(<uint32 1>,)"), Some(1));
        assert_eq!(parse_portal_uint("(uint32 2)\n"), Some(2));
        assert_eq!(parse_portal_uint("(<uint32 0>,)"), Some(0));
    }

    #[test]
    fn parse_portal_uint_rejects_malformed_output() {
        assert_eq!(parse_portal_uint(""), None);
        assert_eq!(parse_portal_uint("(<<uint32>>)"), None);
        assert_eq!(parse_portal_uint("(error)"), None);
        assert_eq!(parse_portal_uint("uint32 nope"), None);
    }
}
