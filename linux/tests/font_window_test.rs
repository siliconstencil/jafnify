//! The window is the fixed phone box, the fonts are the embedded Inter
//! and JetBrains Mono faces, and the page header stays outside the scroll.

fn gui() -> String {
    std::fs::read_to_string("src/bin/jafnify-gui.rs").expect("the window source")
}

fn between<'a>(haystack: &'a str, start: &str, end: &str) -> &'a str {
    let from = haystack.find(start).expect("the start marker");
    let rest = &haystack[from..];
    let to = rest.find(end).expect("the end marker");
    &rest[..to]
}

#[test]
fn the_fonts_are_embedded_from_the_tree() {
    let source = gui();
    for file in [
        "../../assets/fonts/Inter-Regular.ttf",
        "../../assets/fonts/Inter-SemiBold.ttf",
        "../../assets/fonts/JetBrainsMono-Regular.ttf",
    ] {
        assert!(
            source.contains(&format!("include_bytes!(\"{file}\")")),
            "the window embeds {file}"
        );
        let path = format!("assets/fonts/{}", file.rsplit('/').next().unwrap());
        let size = std::fs::metadata(&path)
            .map(|meta| meta.len())
            .unwrap_or(0);
        assert!(size > 100_000, "the font file {path} is present");
    }
}

#[test]
fn the_fonts_are_installed_as_regular_semibold_and_mono() {
    let source = gui();
    for part in [
        "install_fonts(&ctx)",
        "FontFamily::Proportional",
        "FontFamily::Name(\"semibold\".into())",
        "FontFamily::Monospace",
        "\"inter-regular\"",
        "\"inter-semibold\"",
        "\"jetbrains-mono\"",
    ] {
        assert!(source.contains(part), "the font setup has {part}");
    }
}

#[test]
fn the_big_values_use_the_semibold_face() {
    let source = gui();
    assert!(
        source.contains(".font(semibold(self.text.title))"),
        "the big values use the semibold family"
    );
    assert!(
        !source.contains("FontId::monospace(self.text.title)"),
        "the tile values are not the log's monospace face"
    );
}

#[test]
fn the_window_cannot_be_resized() {
    let source = gui();
    for part in [
        ".with_inner_size(scale::WINDOW_SIZE)",
        ".with_min_inner_size(scale::WINDOW_SIZE)",
        ".with_max_inner_size(scale::WINDOW_SIZE)",
        ".with_resizable(false)",
        ".with_maximize_button(false)",
    ] {
        assert!(source.contains(part), "the viewport has {part}");
    }
}

#[test]
fn a_saved_size_is_ignored_and_only_the_position_is_restored() {
    let source = gui();
    let main = between(&source, "fn main()", "#[derive(Clone, Copy, PartialEq)]");
    assert!(!main.contains("window.width"), "the saved width is ignored");
    assert!(!main.contains("window.height"), "the saved height is ignored");
    assert!(main.contains("window.x"), "the saved x is read");
    assert!(main.contains("window.y"), "the saved y is read");
}

#[test]
fn the_page_header_stays_outside_the_scroll() {
    let source = gui();
    let ui = between(&source, "// The Back header stays put", "self.confirm_window");
    let header = ui.find("self.page_header(ui)").expect("the page header");
    let scroll = ui
        .find("egui::ScrollArea::vertical()")
        .expect("the page body scroll");
    assert!(header < scroll, "the header is above the scroll area");
}

#[test]
fn the_scroll_bar_allocates_its_own_width() {
    let source = gui();
    assert!(
        source.contains("style.spacing.scroll.floating = false;"),
        "the bar is not floating over the controls"
    );
    assert!(
        source.contains("style.spacing.scroll.bar_width = 10.0;"),
        "the bar has a reserved width"
    );
}

#[test]
fn the_keyboard_cannot_zoom_the_fixed_window() {
    let source = gui();
    assert!(
        source.contains("zoom_with_keyboard = false"),
        "Ctrl+Plus/Minus cannot scale the fixed UI"
    );
    for part in [
        "ViewportCommand::MinInnerSize(scaled)",
        "ViewportCommand::MaxInnerSize(scaled)",
        "ViewportCommand::InnerSize(scaled)",
    ] {
        assert!(
            source.contains(part),
            "the one-time start-up fit resizes the native window with {part}"
        );
    }
}

#[test]
fn the_fonts_are_in_the_notices() {
    let source = gui();
    let notices = between(&source, "const THIRD_PARTY_NOTICES", "];");
    assert!(notices.contains("\"Inter\""), "the notices name Inter");
    assert!(
        notices.contains("\"JetBrains Mono\""),
        "the notices name JetBrains Mono"
    );
    let script = std::fs::read_to_string("../tools/release/notices.sh")
        .expect("the release notices script");
    assert!(
        script.contains("Inter (Regular and SemiBold)"),
        "the release notices include Inter"
    );
    assert!(
        script.contains("JetBrains Mono (Regular)"),
        "the release notices include JetBrains Mono"
    );
    for licence in ["assets/fonts/Inter-OFL.txt", "assets/fonts/JetBrainsMono-OFL.txt"] {
        let text = std::fs::read_to_string(licence).expect("the OFL text");
        assert!(
            text.contains("SIL OPEN FONT LICENSE"),
            "{licence} carries the OFL text"
        );
    }
}
