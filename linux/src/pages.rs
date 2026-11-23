//! Pure display helpers for the pages of the Linux window. The page bodies
//! stay in the window; these split the daemon's one-line texts into rows so
//! every page can use the settings row language, and keep the words of one
//! fact the same on every page.

use jafnify_core::session::SessionRecord;
use jafnify_core::tiles::{ID_AVERAGE, ID_DNS, ID_DROPS, ID_PHONE, ID_SIGNAL, ID_TODAY, ID_WIFI};
use jafnify_core::ui::ui_text::{format_elapsed, format_timestamp_or_none};
use jafnify_core::usage;

/// One drop row: the clock, how long it was down and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DropRow {
    pub time: String,
    pub duration: String,
    pub cause: String,
}

/// Split the daemon's drop line into its three parts. A line that does not
/// have the shape keeps its whole text as the cause, so nothing is lost.
pub fn drop_parts(detail: &str) -> DropRow {
    let parts: Vec<&str> = detail.split("  ").collect();
    if parts.len() < 2 {
        return DropRow {
            time: String::new(),
            duration: String::new(),
            cause: detail.to_string(),
        };
    }
    let time = parts[0].to_string();
    let duration = parts[1].strip_prefix("down ").unwrap_or(parts[1]).to_string();
    let cause = parts[2..].join("  ");
    DropRow {
        time,
        duration,
        cause,
    }
}

/// The summary of the drop history as a row: the title, the count and the
/// longest outage. A summary in an unknown shape becomes the title alone.
pub fn drop_summary_row(summary: &str) -> (String, String, String) {
    let rest = summary
        .strip_prefix("Drops in the last 24 h: ")
        .unwrap_or("");
    let (count, longest) = match rest.split_once(" (") {
        Some((count, longest)) => (count.trim(), longest.trim_end_matches(')')),
        None => (rest.trim(), ""),
    };
    if count.is_empty() {
        return (summary.to_string(), String::new(), String::new());
    }
    let value = if count == "1" {
        "1 drop".to_string()
    } else {
        format!("{count} drops")
    };
    (String::from("Last 24 hours"), value, longest.to_string())
}

/// The phone measurement lines split into label/value rows and the lines that
/// are notes without a value. A trailing full stop is dropped: the rows are
/// labels, not sentences.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PhoneBlock {
    pub rows: Vec<(String, String)>,
    pub notes: Vec<String>,
}

pub fn phone_block(lines: &[String]) -> PhoneBlock {
    let mut block = PhoneBlock::default();
    for line in lines {
        // The bottleneck verdict is a sentence, not a measurement, even when
        // it holds a colon ("Not enough data: run the link test.").
        if line.starts_with("Not enough data") {
            block.notes.push(line.trim_end_matches('.').to_string());
            continue;
        }
        match line.split_once(": ") {
            Some((label, value)) => block
                .rows
                .push((label.to_string(), value.trim_end_matches('.').to_string())),
            None => block.notes.push(line.trim_end_matches('.').to_string()),
        }
    }
    block
}

/// The home caption of a tile. The settings page names a slot the same way
/// the home names the tile, so a combo never says TODAY while the tile it
/// controls says SESSION.
pub fn home_slot_caption(id: &str, scope_word: &str) -> String {
    match id {
        ID_DNS => "DNS ANSWER".to_string(),
        ID_SIGNAL => "SIGNAL".to_string(),
        ID_PHONE => "PHONE".to_string(),
        ID_WIFI => "WI-FI LEG".to_string(),
        "range" => "RANGE".to_string(),
        ID_TODAY => scope_word.to_string(),
        ID_DROPS => format!("DROPS {scope_word}"),
        ID_AVERAGE => "AVERAGE".to_string(),
        other => other.to_ascii_uppercase(),
    }
}

/// One finished session as a headline: start, duration, data and how it
/// ended. The internal id is not a fact the user needs.
pub fn history_headline(record: &SessionRecord) -> String {
    let total = record.down_bytes.saturating_add(record.up_bytes);
    let ended = match record.end_reason.as_str() {
        "" => "Ended",
        "stopped" => "Stopped",
        "phone session restarted" => "Phone restarted",
        other => other,
    };
    format!(
        "{}  {}  {}  {}",
        format_timestamp_or_none(record.started_at),
        format_elapsed(record.connected_ms / 1_000),
        usage::format_bytes(total),
        ended
    )
}

/// A usage line in the page's words. The core keeps its own wording for the
/// phone and the CLI; the laptop's Statistics page says "Moved" like the
/// AVERAGE tile and hides the line that says nothing.
pub fn usage_display_line(line: &str) -> Option<String> {
    if line == "Network: Unknown" {
        return None;
    }
    if let Some(rest) = line.strip_prefix("Shared ") {
        if let Some((window, amount)) = rest.split_once(": ") {
            return Some(format!("Moved {amount} {window}"));
        }
    }
    Some(line.to_string())
}

/// The good-hours lines without the learning line: the strip above already
/// says how many days it has learnt, and the page says it once.
pub fn good_hours_detail_lines(lines: &[String]) -> Vec<String> {
    lines
        .iter()
        .filter(|line| !line.starts_with("Learning the busy hours:"))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_drop_line_becomes_a_time_a_duration_and_a_cause() {
        let row = drop_parts(
            "23:10  down 46 s  5G (advanced), signal 3/4, 32.7 C - likely out of range",
        );
        assert_eq!(row.time, "23:10");
        assert_eq!(row.duration, "46 s");
        assert_eq!(
            row.cause,
            "5G (advanced), signal 3/4, 32.7 C - likely out of range"
        );
        let open = drop_parts("23:10  not back yet  5G - waiting");
        assert_eq!(open.duration, "not back yet");
    }

    #[test]
    fn the_drop_summary_is_a_row() {
        assert_eq!(
            drop_summary_row("Drops in the last 24 h: 6 (longest 46 s)"),
            (
                "Last 24 hours".to_string(),
                "6 drops".to_string(),
                "longest 46 s".to_string()
            )
        );
        assert_eq!(
            drop_summary_row("Drops in the last 24 h: 1 (longest 0 s)").1,
            "1 drop"
        );
    }

    #[test]
    fn phone_lines_are_rows_and_notes_without_a_trailing_full_stop() {
        let block = phone_block(&[
            "Phone battery: 70%.".to_string(),
            "Phone temperature: 37.4 C.".to_string(),
            "No clear bottleneck".to_string(),
            "Good time for big downloads".to_string(),
        ]);
        assert_eq!(
            block.rows,
            vec![
                ("Phone battery".to_string(), "70%".to_string()),
                ("Phone temperature".to_string(), "37.4 C".to_string())
            ]
        );
        assert_eq!(
            block.notes,
            vec!["No clear bottleneck", "Good time for big downloads"]
        );
    }

    #[test]
    fn a_slot_is_named_like_its_home_tile() {
        assert_eq!(home_slot_caption("dns", "TODAY"), "DNS ANSWER");
        assert_eq!(home_slot_caption("wifi", "TODAY"), "WI-FI LEG");
    }

    #[test]
    fn the_bottleneck_verdict_stays_one_note() {
        let block = phone_block(&[
            "Network: 5G (advanced)".to_string(),
            "Not enough data: run the link test.".to_string(),
        ]);
        assert_eq!(block.rows, vec![("Network".to_string(), "5G (advanced)".to_string())]);
        assert_eq!(block.notes, vec!["Not enough data: run the link test".to_string()]);
        assert_eq!(home_slot_caption("today", "SESSION"), "SESSION");
        assert_eq!(home_slot_caption("drops", "SESSION"), "DROPS SESSION");
        assert_eq!(home_slot_caption("avg", "TODAY"), "AVERAGE");
    }

    #[test]
    fn a_history_headline_has_no_internal_id() {
        let record = SessionRecord {
            id: "1a1f5d24d30-0".to_string(),
            started_at: 1_700_000_000_000,
            ended_at: 1_700_001_000_000,
            connected_ms: 1_000_000,
            down_bytes: 2_500_000_000,
            up_bytes: 3_400_000_000,
            end_reason: "stopped".to_string(),
            ..SessionRecord::default()
        };
        let headline = history_headline(&record);
        assert!(!headline.contains("1a1f5d24d30"), "{headline}");
        assert!(headline.contains("5.9 GB"), "{headline}");
        assert!(headline.ends_with("Stopped"), "{headline}");
        assert!(headline.contains("16:") || headline.contains("17:"), "{headline}");
    }

    #[test]
    fn the_usage_lines_say_moved_and_hide_the_unknown_network() {
        assert_eq!(
            usage_display_line("Shared today: 7.0 GB"),
            Some("Moved 7.0 GB today".to_string())
        );
        assert_eq!(
            usage_display_line("Shared this week: 92.5 GB"),
            Some("Moved 92.5 GB this week".to_string())
        );
        assert_eq!(usage_display_line("Network: Unknown"), None);
        assert_eq!(
            usage_display_line("Average: 233.6 KB/s down, 355.9 KB/s up"),
            Some("Average: 233.6 KB/s down, 355.9 KB/s up".to_string())
        );
    }

    #[test]
    fn the_learning_line_is_not_repeated_in_the_detail_lines() {
        let lines = good_hours_detail_lines(&[
            "Learning the busy hours: 1 of 3 days so far".to_string(),
            "Usually quick: 02:00-07:00".to_string(),
            "Places change the network too".to_string(),
        ]);
        assert_eq!(
            lines,
            vec![
                "Usually quick: 02:00-07:00",
                "Places change the network too"
            ]
        );
    }
}
