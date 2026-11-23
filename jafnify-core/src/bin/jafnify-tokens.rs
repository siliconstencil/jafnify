//! Writes the Android colour resources from the fixed palette.
//!
//! `design/tokens.toml` is the single source of the palette. The core build
//! script turns it into Rust tokens; this binary turns the same tokens into
//! the Android colour resources. `--check` fails when the checked-in files
//! have drifted away from the palette, `--write` updates them, and `--ratios`
//! prints the WCAG contrast of every body-text and indicator pair.

use std::path::PathBuf;

use jafnify_core::tokens::{
    android_colours_xml, android_dimens_xml, contrast_ratio, dark, light, Palette,
    BODY_TEXT_MIN_RATIO, BODY_TEXT_PAIRS, INDICATOR_MIN_RATIO, INDICATOR_PAIRS,
};

fn main() {
    let mut write = false;
    let mut ratios = false;
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "--write" => write = true,
            "--check" => {}
            "--ratios" => ratios = true,
            other => {
                eprintln!("unknown argument: {other}");
                eprintln!("usage: jafnify-tokens [--write|--check|--ratios]");
                std::process::exit(2);
            }
        }
    }
    if ratios {
        print_ratios();
        return;
    }

    let res = res_dir();
    let files = [
        (res.join("values/colors.xml"), android_colours_xml(&light())),
        (
            res.join("values-night/colors.xml"),
            android_colours_xml(&dark()),
        ),
        (res.join("values/dimens.xml"), android_dimens_xml()),
    ];

    let mut drift = false;
    for (path, wanted) in &files {
        let current = std::fs::read_to_string(path).ok();
        if write {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).expect("the resource directory can be created");
            }
            std::fs::write(path, wanted).expect("the colour resource is written");
            println!("wrote {}", path.display());
        } else {
            match &current {
                Some(current) if current == wanted => {
                    println!("ok {}", path.display());
                }
                Some(_) => {
                    drift = true;
                    eprintln!("drifted {}", path.display());
                }
                None => {
                    drift = true;
                    eprintln!("missing {}", path.display());
                }
            }
        }
    }

    if drift && !write {
        eprintln!("the Android colours are not the palette of design/tokens.toml");
        eprintln!("run: cargo run --manifest-path jafnify-core/Cargo.toml --bin jafnify-tokens -- --write");
        std::process::exit(1);
    }
}

/// Prints the contrast ratio of every pair the palette is allowed to use.
fn print_ratios() {
    for (theme, palette) in [("light", light()), ("dark", dark())] {
        println!("{theme} theme");
        for (fg, bg) in BODY_TEXT_PAIRS {
            print_pair(theme, &palette, fg, bg, "body text", BODY_TEXT_MIN_RATIO);
        }
        for (fg, bg) in INDICATOR_PAIRS {
            print_pair(theme, &palette, fg, bg, "indicator", INDICATOR_MIN_RATIO);
        }
    }
}

fn print_pair(theme: &str, palette: &Palette, fg: &str, bg: &str, kind: &str, min: f64) {
    let fg_value = palette
        .role(fg)
        .expect("body text and indicator roles exist");
    let bg_value = palette
        .role(bg)
        .expect("body text and indicator roles exist");
    let ratio = contrast_ratio(fg_value, bg_value);
    let verdict = if ratio >= min { "PASS" } else { "FAIL" };
    println!("  {theme}: {fg} on {bg} = {ratio:.2}:1 (needs {min}:1) {verdict} [{kind}]");
}

/// The Android resource directory of the app.
fn res_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../app/app/src/main/res")
}
