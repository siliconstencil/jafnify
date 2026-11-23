//! Every page uses the settings row language, and the same fact reads
//! the same on every page. The window only draws the daemon's snapshot, so
//! these tests pin the source markers that keep the pages in that shape.

use std::fs;

fn gui_source() -> String {
    fs::read_to_string("src/bin/jafnify-gui.rs").unwrap()
}

fn daemon_source() -> String {
    fs::read_to_string("src/daemon.rs").unwrap()
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

#[test]
fn the_drops_page_is_rows_without_a_second_heading() {
    let gui = gui_source();
    let page = between(&gui, "fn drops_block", "fn drops_on");
    assert!(
        !page.contains("\"Drops\").strong()"),
        "the page title already says Drops"
    );
    assert!(page.contains("drop_summary_row"), "the summary is a row");
    assert!(page.contains("drop_parts"), "each drop is one row");
    assert!(page.contains("settings_value_row"), "the rows use the row language");
    assert!(
        page.contains("Clear drop history"),
        "the clear button is at the end"
    );
}

#[test]
fn the_best_spot_page_explains_itself_and_says_why_a_button_is_disabled() {
    let gui = gui_source();
    let page = between(&gui, "fn best_spot_block", "fn best_spot_command");
    assert!(!page.contains("\"Best spot\").strong()"), "no duplicate heading");
    assert!(page.contains("SESSION_SECS"), "the explanation names the length");
    assert!(page.contains("This spot is available"), "the disabled mark is explained");
    assert!(page.contains("Stop is available"), "the disabled stop is explained");
    assert!(page.contains("egui::vec2(120.0, 40.0)"), "the buttons are real");
}

#[test]
fn the_home_layout_rows_name_the_tiles_like_the_home_does() {
    let gui = gui_source();
    let page = between(&gui, "fn home_layout_block", "fn log_body");
    assert!(
        !page.contains("\"Home layout\").strong()"),
        "the page title already says Home layout"
    );
    assert!(page.contains("settings_control_row"), "the controls are rows");
    assert!(page.contains("home_slot_caption"), "the combo uses the home caption");
    assert!(page.contains(".width(180.0)"), "the combo has a fixed width");
}

#[test]
fn the_pairing_page_has_no_disconnect_and_real_pair_buttons() {
    let gui = gui_source();
    let page = between(&gui, "fn pairing_body", "fn notices_body");
    assert!(
        !page.contains("home_actions"),
        "the old block would draw Disconnect on this page"
    );
    assert!(page.contains("Paired with"), "a paired page names the device");
    assert!(page.contains("Pair new device"), "pair new is a real button");
    assert!(page.contains("Forget device"), "forget is a real button");
    assert!(
        !page.contains("Action::Primary(Primary::Disconnect)"),
        "Stop lives on Home"
    );
}

#[test]
fn the_statistics_page_has_one_heading_case_and_one_learning_line() {
    let gui = gui_source();
    let page = between(&gui, "fn statistics_body", "fn session_history_body");
    assert!(!page.contains("USUAL HOURS"), "the strip title is not repeated");
    assert!(page.contains("\"Busy hours\""), "one title-case heading");
    assert!(page.contains("usage_display_line"), "Moved replaces Shared");
    let history = between(&gui, "fn session_history_body", "fn copy_diagnostics");
    assert!(
        history.contains("history_headline"),
        "the history rows have no ids"
    );
}

#[test]
fn the_log_opens_on_the_newest_line() {
    let gui = gui_source();
    let page = between(&gui, "fn log_body", "fn statistics_body");
    assert!(
        page.contains("connection_log.iter().rev()"),
        "the newest line is first"
    );
}

#[test]
fn the_details_phone_block_is_rows_with_separated_links() {
    let gui = gui_source();
    let page = between(&gui, "fn details_body", "fn best_spot_on");
    assert!(page.contains("phone_block"), "the phone lines become rows");
    assert!(page.contains("settings_link_row"), "the links are rows");
    assert!(page.contains("\"Connection log\""), "the log link stays");
}

#[test]
fn the_link_test_title_and_button_do_not_repeat() {
    let gui = gui_source();
    let page = between(&gui, "fn settings_body", "fn home_layout_block");
    assert!(page.contains("\"Link test\""), "the row is named Link test");
    assert!(page.contains("egui::Button::new(egui::RichText::new(\"Run\")"), "the button says Run");
    assert!(
        page.contains("min_size(egui::vec2(84.0, 36.0))"),
        "Run is the standard 84x36 button"
    );
    assert!(page.contains("desired_width(240.0)"), "the name field is wide enough");
}

#[test]
fn the_statistics_usage_lines_are_body_size() {
    let gui = gui_source();
    let page = between(&gui, "fn statistics_body", "fn session_history_body");
    let at = page
        .find("usage_display_line")
        .expect("the usage lines are drawn");
    assert!(
        page[at..].contains(".size(text.body)"),
        "the usage data uses the body token, like the history rows"
    );
}

#[test]
fn the_monthly_allowance_is_a_caption_under_its_title() {
    let gui = gui_source();
    let page = between(&gui, "fn statistics_body", "fn session_history_body");
    assert!(
        !page.contains("settings_value_row"),
        "the allowance is not a stray right-hand value"
    );
    let at = page
        .find("\"Monthly allowance\"")
        .expect("the allowance keeps its title");
    assert!(
        page[at..].contains(".size(text.caption)"),
        "the allowance sentence is the muted caption under the title"
    );
}

#[test]
fn the_clear_good_hours_uses_the_standard_button() {
    let gui = gui_source();
    let page = between(&gui, "fn good_hours_block", "fn drops_block");
    assert!(page.contains("Clear good hours"), "the button is present");
    assert!(
        page.contains("min_size(egui::vec2(84.0, 36.0))"),
        "it is the same 84x36 button as Back and Run"
    );
}

#[test]
fn the_details_share_one_value_column() {
    let gui = gui_source();
    assert!(
        gui.contains("const DETAIL_LABEL_WIDTH: f32 = 190.0;"),
        "one label width is chosen for all groups"
    );
    assert!(
        gui.contains("min_col_width(DETAIL_LABEL_WIDTH)"),
        "every Details group uses the same value column"
    );
}

#[test]
fn the_shared_rows_are_body_size_with_caption_help() {
    let gui = gui_source();
    let row = between(&gui, "fn settings_row", "fn settings_value_row");
    assert!(
        row.contains(".size(text.body)"),
        "the title and the value are the body token"
    );
    assert!(
        row.contains(".size(text.caption)"),
        "the explanation is the caption token"
    );
}

#[test]
fn the_daemon_derives_the_session_rows_from_the_drop_events() {
    let daemon = daemon_source();
    assert!(
        daemon.contains("session_drop_count"),
        "the snapshot counts the session drops from the same events as the tile"
    );
    assert!(
        daemon.contains("session_drops"),
        "the snapshot carries the drop count to the rows"
    );
    assert!(
        daemon.contains("session.connected_ms / 1_000"),
        "the card's time is the session's connected time"
    );
}

#[test]
fn the_about_version_comes_from_the_crate_version() {
    let gui = gui_source();
    assert!(
        gui.contains(r#"env!("CARGO_PKG_VERSION")"#),
        "the About modal reads the crate version"
    );
}
