//! The fixed Jafnify palette, as egui colours.
//!
//! Every value comes from `jafnify_core::tokens` by role. No colour is written
//! here: the design tokens are the one source for Linux and the phone.

use eframe::egui::Color32;
use jafnify_core::tokens::{self, Palette};

use crate::theme::{Theme, ThemeMode};

/// The colour roles the window and the mark use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Colours {
    pub bg: Color32,
    pub surface: Color32,
    pub fg: Color32,
    pub muted: Color32,
    pub line: Color32,
    pub primary: Color32,
    pub primary_on: Color32,
    pub accent: Color32,
    pub ok: Color32,
    pub wait: Color32,
    pub wait_text: Color32,
    pub err: Color32,
}

impl Colours {
    pub fn light() -> Self {
        Self::from_palette(&tokens::light())
    }

    pub fn dark() -> Self {
        Self::from_palette(&tokens::dark())
    }

    fn from_palette(p: &Palette) -> Self {
        Self {
            bg: colour(p.bg),
            surface: colour(p.surface),
            fg: colour(p.fg),
            muted: colour(p.muted),
            line: colour(p.line),
            primary: colour(p.primary),
            primary_on: colour(p.primary_on),
            accent: colour(p.accent),
            ok: colour(p.ok),
            wait: colour(p.wait),
            wait_text: colour(p.wait_text),
            err: colour(p.err),
        }
    }

    /// The colours of a theme that `theme::resolve` already decided.
    pub fn for_theme(theme: Theme) -> Self {
        match theme {
            Theme::Dark => Self::dark(),
            Theme::Light => Self::light(),
        }
    }

    /// Pick by saved mode and resolved theme: an explicit mode wins, System
    /// uses the theme the caller resolved.
    pub fn for_mode(mode: ThemeMode, resolved: Theme) -> Self {
        match mode {
            ThemeMode::Dark => Self::dark(),
            ThemeMode::Light => Self::light(),
            ThemeMode::System => Self::for_theme(resolved),
        }
    }
}

/// One `0xRRGGBB` design token as an opaque egui colour.
fn colour(value: u32) -> Color32 {
    Color32::from_rgb(
        ((value >> 16) & 255) as u8,
        ((value >> 8) & 255) as u8,
        (value & 255) as u8,
    )
}

/// RGBA bytes for an egui colour, for the tray's ARGB icon.
pub fn rgba(colour: Color32) -> [u8; 4] {
    [colour.r(), colour.g(), colour.b(), colour.a()]
}
