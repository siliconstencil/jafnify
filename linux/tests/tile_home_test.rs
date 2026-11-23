//! The tile home is the standard window. It shows the status card, the tiles in
//! the saved grid, the strip of the hours and the two buttons; the old card is
//! only the branch a broken tile home falls back to. The data comes from the
//! daemon in the snapshot; the window only draws it.
//!

use std::fs;

use jafnify_core::flags::{self, Bucket};
use jafnify_core::tiles::{HomeModel, StatusCard, Tone};
use jafnify_linux::control::Snapshot;
use jafnify_linux::palette::Colours;
use jafnify_linux::theme::Theme;

fn gui_source() -> String {
    fs::read_to_string("src/bin/jafnify-gui.rs").unwrap()
}

/// The part of the source after a marker, up to the next marker.
fn between<'a>(source: &'a str, start: &str, end: &str) -> &'a str {
    let after = source
        .split(start)
        .nth(1)
        .unwrap_or_else(|| panic!("the source has {start}"));
    after
        .split(end)
        .next()
        .unwrap_or_else(|| panic!("the part after {start} ends with {end}"))
}

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
fn the_tile_home_is_standard_and_always_on() {
    let flag = flags::find("tile_home").expect("the tile home is declared");
    assert_eq!(flag.bucket, Bucket::Standard);
    assert!(flag.default, "the tile home is on for every install");
    assert!(flags::standard_id("tile_home"));
}

#[test]
fn the_old_window_is_only_the_fallback_branch() {
    let gui = gui_source();
    let frame = between(&gui, "fn ui(&mut self", "fn state_from");
    assert!(
        frame.contains("if self.tile_home_on()"),
        "the frame follows the standard row"
    );
    let old = between(frame, "if self.tile_home_on()", "self.tile_buttons(ui)");
    let old = old
        .split("} else {")
        .nth(1)
        .expect("the old window is the other branch");
    assert!(
        old.contains("self.card(ui, &v)"),
        "the old card stays as the fallback"
    );
    assert!(
        old.contains("ui.add_space(12.0)"),
        "the old spacing stays as it was"
    );
    assert!(
        old.contains("self.home_actions(ui, &colours, &v, &mut actions, true)"),
        "the old actions stay as they were"
    );
}

#[test]
fn the_old_window_keeps_every_part_it_had() {
    // The old action block was moved into a helper, so that the tile home can
    // leave the big button to the card. The helper must still hold every part
    // of the old block.
    let gui = gui_source();
    let helper = between(&gui, "fn home_actions", "fn page_header");
    for part in [
        "v.show_code_entry",
        "self.code_boxes(ui)",
        "self.code_error",
        "Allow on this Wi-Fi interface",
        "v.polkit_note",
        "v.primary.label()",
        "egui::vec2(220.0, 40.0)",
        "Pair new device",
        "Forget device",
        "Cancel",
        "Pairing code",
    ] {
        assert!(helper.contains(part), "the old window still has {part}");
    }
}

#[test]
fn the_tile_home_draws_the_sections_in_order() {
    let gui = gui_source();
    let home = between(&gui, "fn tile_home(", "fn tile_status_card");
    let card = home
        .find("self.tile_status_card(")
        .expect("the card is drawn");
    let grid = home.find("self.tile_grid(").expect("the tiles are drawn");
    let buttons = home
        .find("self.tile_buttons(")
        .expect("the page row is drawn at the bottom");
    assert!(card < grid, "the card comes above the grid: {card} {grid}");
    assert!(
        grid < buttons,
        "the page row is under the grid: {grid} {buttons}"
    );
    // The fixed home has no hours strip; the hours are a page.
    assert!(!home.contains("self.hours_strip("), "the hours are a page");
}

#[test]
fn the_window_only_draws_the_tiles_the_daemon_sends() {
    let gui = gui_source();
    assert!(
        gui.contains("self.snapshot.home"),
        "the window reads the home of the snapshot"
    );
    for builder in [
        "dns_tile(",
        "signal_tile(",
        "phone_tile(",
        "today_tile(",
        "wifi_tile(",
        "drops_tile(",
        "home_model(",
        "hot_hour_evidence(",
    ] {
        assert!(
            !gui.contains(builder),
            "the window must not build the tiles itself: {builder}"
        );
    }
}

#[test]
fn the_window_never_opens_a_file() {
    let gui = gui_source();
    for forbidden in [
        "File::open",
        "fs::read",
        "read_to_string",
        "fs::File",
        "OpenOptions",
    ] {
        assert!(
            !gui.contains(forbidden),
            "the window must not read a file: {forbidden}"
        );
    }
}

#[test]
fn the_buttons_are_big_enough_for_a_finger() {
    let gui = gui_source();
    let card = between(&gui, "fn tile_status_card", "fn tile_grid");
    assert!(
        card.contains("egui::vec2(160.0, 44.0)"),
        "the button of the card is 44 pixels high"
    );
    let buttons = between(&gui, "fn tile_buttons", "fn home_actions");
    assert!(
        buttons.contains("44.0"),
        "the buttons under the tiles are 44 pixels high"
    );
    let actions = between(&gui, "fn home_actions", "fn page_header");
    assert!(
        actions.contains("44.0"),
        "the firewall button of the tile home is 44 pixels high"
    );
}

#[test]
fn the_snapshot_carries_the_tiles() {
    let mut snapshot = Snapshot::default();
    assert!(
        snapshot.home.is_none(),
        "there is no home without the switch"
    );
    snapshot.home = Some(HomeModel {
        status: StatusCard {
            connected: true,
            tone: Tone::Ok,
            title: "Connected to Icy Parrot".to_string(),
            subline: Some("Direct over hotspot - up 1:12:04".to_string()),
            stop: true,
            notice: None,
            action: None,
        },
        tiles: Vec::new(),
        hours: None,
    });
    let text = serde_json::to_string(&snapshot).unwrap();
    assert!(
        text.contains("\"home\""),
        "the home travels in the snapshot"
    );
    let back: Snapshot = serde_json::from_str(&text).unwrap();
    assert_eq!(back.home, snapshot.home, "the home survives the round trip");
    assert_eq!(back.home.unwrap().status.title, "Connected to Icy Parrot");
}

#[test]
fn an_older_snapshot_without_the_home_field_still_loads() {
    let mut value = serde_json::to_value(Snapshot::default()).unwrap();
    value.as_object_mut().unwrap().remove("home");
    let old: Snapshot = serde_json::from_value(value).unwrap();
    assert!(old.home.is_none());
}

#[test]
fn the_text_of_the_tiles_is_readable_in_both_themes() {
    // The pairs the tile home draws its text with: the value and the label
    // on the card, the dim facts, the good-time chip and the three tones.
    for theme in [Theme::Dark, Theme::Light] {
        let c = Colours::for_theme(theme);
        let pairs: [(&str, eframe::egui::Color32, eframe::egui::Color32); 8] = [
            ("the value on the card", c.fg, c.surface),
            ("the value on the background", c.fg, c.bg),
            ("the dim facts on the card", c.muted, c.surface),
            ("the dim facts on the background", c.muted, c.bg),
            ("the chip on the card", c.ok, c.surface),
            ("the waiting tone on the card", c.wait_text, c.surface),
            ("the error tone on the card", c.err, c.surface),
            ("the muted tone on the card", c.fg, c.surface),
        ];
        for (what, fg, bg) in pairs {
            let ratio = contrast(fg, bg);
            assert!(ratio >= 4.5, "{theme:?}: {what} is {ratio:.2}:1");
        }
    }
}

#[test]
fn the_laptop_can_draw_the_fill_bar_of_the_battery() {
    // The phone draws the battery bar with its own view; the laptop must be
    // able to draw the same kind of bar, so the model stays portable.
    let gui = gui_source();
    assert!(
        gui.contains("TrendKind::Fill"),
        "the laptop draws the fill bar of the battery tile"
    );
    let arm = between(&gui, "TrendKind::Fill", "TrendKind::Hours");
    assert!(
        arm.contains("colours.primary") && arm.contains("colours.line"),
        "the fill bar has a base and a filled part"
    );
}

#[test]
fn the_cells_of_a_strip_are_drawn_in_the_order_of_the_list() {
    // The strip of the drops runs over the last 24 hours, so its cells are not
    // the hours of the day: the first cell is the oldest hour. The renderer
    // has to use the place of the cell in the list, not the hour it carries.
    let gui = gui_source();
    let cells = between(&gui, "fn hour_cells", "fn dashed_line");
    assert!(
        cells.contains("enumerate()"),
        "the cells are placed by their place in the list"
    );
    assert!(
        !cells.contains("cell.hour"),
        "the cells are not placed by the hour they carry"
    );
}

#[test]
fn the_marks_of_the_strips_are_visible_in_both_themes() {
    // A mark is not text, so it only has to be seen: 3:1 is the rule of the
    // indicators.
    for theme in [Theme::Dark, Theme::Light] {
        let c = Colours::for_theme(theme);
        let marks: [(&str, eframe::egui::Color32); 4] = [
            ("a quick hour", c.ok),
            ("a busy hour", c.wait),
            ("an hour without a verdict", c.muted),
            ("an hour with a drop", c.err),
        ];
        for (what, mark) in marks {
            for (where_, back) in [("card", c.surface), ("background", c.bg)] {
                let ratio = contrast(mark, back);
                assert!(
                    ratio >= 3.0,
                    "{theme:?}: {what} on the {where_} is {ratio:.2}:1"
                );
            }
        }
    }
}

#[test]
fn a_cell_outside_the_window_of_the_tile_is_the_faintest_cell() {
    // The hours outside the window of the tile are not counted: they are
    // drawn as a low hairline in the line colour, fainter than an hour of
    // the window without a verdict and never like an absence (muted) or a
    // drop (err).
    let gui = gui_source();
    let cells = between(&gui, "fn hour_cells", "fn dashed_line");
    let outside = cells
        .find("!cell.in_scope")
        .expect("the renderer reads the scope of the cell");
    let filled = cells
        .find("cell_colour(colours, cell)")
        .expect("a cell of the window keeps the colour of its mark");
    assert!(outside < filled, "the outside cell is handled first");
    let branch = &cells[outside..filled];
    assert!(
        branch.contains("OUTSIDE_CELL_HEIGHT") && branch.contains("colours.line"),
        "an outside cell is a low hairline in the line colour"
    );
    assert!(
        !branch.contains("colours.muted") && !branch.contains("colours.err"),
        "an outside cell never looks like an absence or a drop"
    );
    let colour = between(&gui, "fn cell_colour", "fn hour_cells");
    assert!(
        !colour.contains("colours.muted"),
        "the colour of a cell of the window is the colour of its mark"
    );
}

#[test]
fn the_caption_of_the_strip_is_above_the_strip() {
    // The caption says why the strip looks the way it does: the learning
    // line, or the line that says no hour has a verdict yet. It sits above
    // the strip, where it is always seen.
    let gui = gui_source();
    let strip = between(&gui, "fn hours_strip(", "fn home_actions");
    let caption = strip
        .find("strip.caption")
        .expect("the strip draws the caption of the core");
    let cells = strip
        .find("hour_cells(")
        .expect("the strip draws its cells");
    assert!(
        caption < cells,
        "the caption comes above the strip: {caption} {cells}"
    );
    assert!(
        strip.contains("strip.learning"),
        "an older daemon without a caption keeps its learning line"
    );
}

#[test]
fn the_heading_of_the_strip_comes_from_the_core() {
    // The wording of the heading belongs to the core: the window only draws
    // it.
    let gui = gui_source();
    let strip = between(&gui, "fn hours_strip(", "fn home_actions");
    assert!(
        strip.contains("strip.title"),
        "the window draws the heading of the core"
    );
    assert!(
        !strip.contains("GOOD HOURS"),
        "the window has no heading of its own"
    );
}

#[test]
fn the_whole_tile_is_the_target_of_the_tap() {
    // The tile that sums over time changes its scope when it is tapped, so
    // the whole tile is the target and the pointer says what a tap does.
    let gui = gui_source();
    let card = between(&gui, "fn tile_card", "fn hours_strip");
    assert!(
        card.contains("ui.interact("),
        "the tile itself is the target of the tap"
    );
    assert!(
        card.contains("ui.max_rect()"),
        "the target is the whole tile"
    );
    assert!(
        card.contains("Action::TileScope"),
        "the tap asks for another scope"
    );
    assert!(
        card.contains("TAP_TO_CHANGE"),
        "the pointer says what the tap does"
    );
}

#[test]
fn the_tap_asks_the_core_for_the_next_scope() {
    // The core owns the cycle: the window reads the stored choice and asks
    // the core for the next one.
    let gui = gui_source();
    let helper = between(&gui, "fn next_tile_scope", "fn tile_status_card");
    assert!(
        helper.contains("TileScope::for_tile("),
        "the window reads the stored scope: {helper}"
    );
    assert!(
        helper.contains(".next("),
        "the core owns the order of the cycle: {helper}"
    );
    let handle = between(&gui, "fn handle(", "fn input(");
    assert!(
        handle.contains("Request::SetTileScope"),
        "the window asks the daemon to store the scope: {handle}"
    );
}

#[test]
fn the_scopes_are_stored_outside_the_state_file() {
    // `state.toml` is read with `deny_unknown_fields`: a new key there would
    // make an older build set the file aside and start unpaired.
    let settings = fs::read_to_string("src/settings.rs").unwrap();
    assert!(
        !settings.contains("tile_scopes"),
        "the scopes must not be a key of the state file"
    );
    let scopes = fs::read_to_string("src/tile_scopes.rs").unwrap();
    assert!(
        scopes.contains("pub const FILE_NAME"),
        "the scopes live in their own file"
    );
}
