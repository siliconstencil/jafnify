//! The palette is fixed, shared by every frontend, and readable enough.
//!
//! The values come from `design/tokens.toml`; these tests guard the fixed
//! palette and the contrast rules that go with it.

use jafnify_core::tokens::{
    android_colours_xml, android_dimens_xml, contrast_ratio, dark, light, parse_colours_xml,
    Palette, BODY_TEXT_MIN_RATIO, BODY_TEXT_PAIRS, ELEVATION_SCALE, INDICATOR_MIN_RATIO,
    INDICATOR_PAIRS, LINE_HEIGHT_SCALE, RADIUS_SCALE, ROLES, SPACING_SCALE, TYPE_SCALE,
};

fn themes() -> [(&'static str, Palette); 2] {
    [("light", light()), ("dark", dark())]
}

#[test]
fn the_palette_is_the_fixed_one_from_the_decisions() {
    let l = light();
    assert_eq!(l.bg, 0xF4F5F6);
    assert_eq!(l.surface, 0xFFFFFF);
    assert_eq!(l.fg, 0x17191C);
    assert_eq!(l.muted, 0x5F656D);
    assert_eq!(l.line, 0xDDE0E4);
    assert_eq!(l.primary, 0x1F2328);
    assert_eq!(l.primary_on, 0xFFFFFF);
    assert_eq!(l.accent, 0xB0263A);
    assert_eq!(l.ok, 0x1A6E3B);
    assert_eq!(l.wait, 0xB26A00);
    assert_eq!(l.wait_text, 0x8A5200);
    assert_eq!(l.err, 0xB3261E);

    let d = dark();
    assert_eq!(d.bg, 0x0E0F11);
    assert_eq!(d.surface, 0x17191C);
    assert_eq!(d.fg, 0xE6E8EB);
    assert_eq!(d.muted, 0x9AA1AA);
    assert_eq!(d.line, 0x2A2E33);
    assert_eq!(d.primary, 0xE6E8EB);
    assert_eq!(d.primary_on, 0x15171A);
    assert_eq!(d.accent, 0xFF5C6C);
    assert_eq!(d.ok, 0x6FD99A);
    assert_eq!(d.wait, 0xF2B35B);
    assert_eq!(d.wait_text, 0xF2B35B);
    assert_eq!(d.err, 0xFF8A80);
}

#[test]
fn the_scales_are_the_one_source_of_spacing_type_radius_and_elevation() {
    assert_eq!(
        SPACING_SCALE,
        [("xs", 4), ("sm", 8), ("md", 12), ("lg", 16), ("xl", 24)]
    );
    assert_eq!(
        TYPE_SCALE,
        [
            ("caption", 11),
            ("body", 14),
            ("body_large", 17),
            ("title", 22),
            ("display", 34),
        ]
    );
    assert_eq!(
        LINE_HEIGHT_SCALE,
        [
            ("caption", 16),
            ("body", 20),
            ("body_large", 24),
            ("title", 28),
            ("display", 40),
        ]
    );
    assert_eq!(RADIUS_SCALE, [("sm", 8), ("md", 12), ("lg", 16)]);
    assert_eq!(ELEVATION_SCALE, [("none", 0), ("low", 1), ("high", 2)]);
}

#[test]
fn body_text_clears_the_aa_ratio_in_both_themes() {
    for (name, theme) in themes() {
        for (fg, bg) in BODY_TEXT_PAIRS {
            let ratio = contrast_ratio(
                theme.role(fg).expect("body text role exists"),
                theme.role(bg).expect("body background role exists"),
            );
            assert!(
                ratio >= BODY_TEXT_MIN_RATIO,
                "{name}: {fg} on {bg} is {ratio:.2}:1, needs {BODY_TEXT_MIN_RATIO}:1"
            );
        }
    }
}

#[test]
fn state_indicators_clear_the_non_text_ratio_in_both_themes() {
    for (name, theme) in themes() {
        for (fg, bg) in INDICATOR_PAIRS {
            let ratio = contrast_ratio(
                theme.role(fg).expect("indicator role exists"),
                theme.role(bg).expect("indicator background role exists"),
            );
            assert!(
                ratio >= INDICATOR_MIN_RATIO,
                "{name}: {fg} on {bg} is {ratio:.2}:1, needs {INDICATOR_MIN_RATIO}:1"
            );
        }
    }
}

#[test]
fn the_wait_colour_is_never_used_for_body_text() {
    for (fg, _) in BODY_TEXT_PAIRS {
        assert_ne!(
            *fg, *"wait",
            "the wait colour does not reach 4.5:1 in light"
        );
    }
    for (_, bg) in BODY_TEXT_PAIRS {
        assert_ne!(*bg, *"wait");
    }
    assert!(
        INDICATOR_PAIRS.iter().any(|(fg, _)| *fg == "wait"),
        "the wait colour is used for indicators"
    );
}

#[test]
fn the_wait_text_label_is_legal_for_body_text() {
    for (name, theme) in themes() {
        for background in ["bg", "surface"] {
            let ratio = contrast_ratio(
                theme.wait_text,
                theme.role(background).expect("a background role"),
            );
            assert!(
                ratio >= BODY_TEXT_MIN_RATIO,
                "{name}: wait_text on {background} is {ratio:.2}:1, needs {BODY_TEXT_MIN_RATIO}:1"
            );
        }
    }
}

#[test]
fn every_role_is_written_out() {
    for theme in themes() {
        let entries = theme.1.entries();
        assert_eq!(entries.len(), ROLES.len());
        for (role, _) in entries {
            assert!(ROLES.contains(&role), "{role} is a known role");
        }
    }
}

#[test]
fn the_android_colours_match_the_tokens() {
    let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../app/app/src/main/res");
    for (path, theme) in [
        (format!("{root}/values/colors.xml"), light()),
        (format!("{root}/values-night/colors.xml"), dark()),
    ] {
        let raw = std::fs::read_to_string(&path).expect("the generated colour file exists");
        let parsed = parse_colours_xml(&raw);
        for (role, value) in theme.entries() {
            let name = format!("jafnify_{role}");
            let found = parsed
                .iter()
                .find(|(found, _)| *found == name)
                .map(|(_, value)| *value);
            assert_eq!(found, Some(value), "{path} has the current {role} colour");
        }
        assert_eq!(
            parsed.len(),
            theme.entries().len(),
            "{path} has no extra colours"
        );
    }
}

#[test]
fn the_android_dimens_match_the_tokens() {
    let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../app/app/src/main/res");
    let path = format!("{root}/values/dimens.xml");
    let raw = std::fs::read_to_string(&path).expect("the generated dimens file exists");
    assert_eq!(raw, android_dimens_xml(), "{path} is the generated scale");
}

#[test]
fn the_generated_android_dimens_are_reproducible() {
    let xml = android_dimens_xml();
    assert!(xml.contains("<dimen name=\"jafnify_spacing_xs\">4dp</dimen>"));
    assert!(xml.contains("<dimen name=\"jafnify_type_body\">14sp</dimen>"));
    assert!(xml.contains("<dimen name=\"jafnify_line_height_body\">20sp</dimen>"));
    assert!(xml.contains("<dimen name=\"jafnify_radius_md\">12dp</dimen>"));
    assert!(xml.contains("<dimen name=\"jafnify_elevation_high\">2dp</dimen>"));
}

#[test]
fn the_generated_android_colours_are_reproducible() {
    for theme in themes() {
        let xml = android_colours_xml(&theme.1);
        assert!(xml.contains("<color name=\"jafnify_bg\">"));
        assert!(!xml.contains("purple_"));
        assert!(!xml.contains("teal_"));
    }
}
