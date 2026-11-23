//! Tray state mapping and the visibility setting.

use std::collections::HashSet;

use jafnify_core::snapshot::GlyphShape;
use jafnify_core::ConnectionState;
use jafnify_linux::control::Request;
use jafnify_linux::palette::{rgba, Colours};
use jafnify_linux::tray::{
    tray_should_be_visible, TrayCommand, TrayVisual, COPY_DIAGNOSTICS_LABEL,
};

#[test]
fn the_icon_shows_every_state() {
    assert_eq!(
        TrayVisual::from_state(ConnectionState::Idle),
        TrayVisual::Off
    );
    assert_eq!(
        TrayVisual::from_state(ConnectionState::Searching),
        TrayVisual::Connecting
    );
    assert_eq!(
        TrayVisual::from_state(ConnectionState::Pairing),
        TrayVisual::Connecting
    );
    assert_eq!(
        TrayVisual::from_state(ConnectionState::Connecting),
        TrayVisual::Connecting
    );
    assert_eq!(
        TrayVisual::from_state(ConnectionState::Retry),
        TrayVisual::Retrying
    );
    assert_eq!(
        TrayVisual::from_state(ConnectionState::Connected),
        TrayVisual::Connected
    );
    assert_eq!(
        TrayVisual::from_state_and_error(ConnectionState::Idle, true),
        TrayVisual::Error
    );
    assert_eq!(TrayVisual::Error.label(), "Error");
    assert_eq!(TrayVisual::Retrying.label(), "Retrying");
    assert_eq!(TrayVisual::Connected.label(), "Connected");
}

#[test]
fn every_tray_state_has_its_own_shape() {
    let visuals = [
        TrayVisual::Off,
        TrayVisual::Connecting,
        TrayVisual::Connected,
        TrayVisual::Retrying,
        TrayVisual::Error,
    ];
    let mut names = HashSet::new();
    for visual in visuals {
        names.insert(visual.shape().name());
    }
    assert_eq!(
        names.len(),
        visuals.len(),
        "no two tray states share a shape"
    );
    assert_eq!(
        TrayVisual::Off.shape(),
        GlyphShape::NoArcHollowDot,
        "idle has no arc and a hollow dot"
    );
    assert_eq!(
        TrayVisual::Connecting.shape(),
        GlyphShape::OneArcFilledDot,
        "connecting has one arc and a filled dot"
    );
    assert_eq!(
        TrayVisual::Connected.shape(),
        GlyphShape::TwoArcsFilledDot,
        "connected has two arcs and a filled dot"
    );
    assert_eq!(
        TrayVisual::Retrying.shape(),
        GlyphShape::OneArcHollowDotSlash,
        "retrying has a slash"
    );
    assert_eq!(
        TrayVisual::Error.shape(),
        GlyphShape::Exclamation,
        "the error state differs by glyph, not colour alone"
    );
}

#[test]
fn the_tray_colours_are_the_dark_state_palette() {
    let dark = Colours::dark();
    assert_eq!(TrayVisual::Off.color(), rgba(dark.muted));
    assert_eq!(TrayVisual::Connecting.color(), rgba(dark.wait));
    assert_eq!(TrayVisual::Connected.color(), rgba(dark.ok));
    assert_eq!(TrayVisual::Retrying.color(), rgba(dark.wait));
    assert_eq!(TrayVisual::Error.color(), rgba(dark.err));
    assert_ne!(TrayVisual::Off.color(), TrayVisual::Connected.color());
}

#[test]
fn the_copy_diagnostics_entry_asks_for_the_diagnostics_text() {
    assert_eq!(COPY_DIAGNOSTICS_LABEL, "Copy diagnostics");
    assert_eq!(
        TrayCommand::CopyDiagnostics.request(),
        Some(Request::Diagnostics)
    );
    assert_eq!(TrayCommand::Open.request(), None);
}

#[test]
fn the_tray_is_visible_by_default_even_with_the_window_open() {
    assert!(tray_should_be_visible(false, false));
    assert!(tray_should_be_visible(false, true));
}

#[test]
fn hiding_the_tray_while_the_window_is_open_is_opt_in() {
    assert!(tray_should_be_visible(true, false));
    assert!(!tray_should_be_visible(true, true));
}
