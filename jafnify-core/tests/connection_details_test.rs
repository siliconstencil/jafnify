//! The connection details both apps show, in the core's own words.

use jafnify_core::connection_details::{ConnectionDetails, DeviceSide, DnsMode};
use jafnify_core::snapshot::PeerPath;

fn details() -> ConnectionDetails {
    ConnectionDetails {
        path: Some(
            PeerPath::DirectHotspot
                .text()
                .unwrap_or_default()
                .to_string(),
        ),
        verdict: Some("Update Jafnify on the other device".to_string()),
        dns: DnsMode::ThroughTunnel,
        endpoint: Some("10.1.0.2:9797".to_string()),
    }
}

#[test]
fn the_phone_shows_the_laptop_address_and_the_laptop_the_phone_address() {
    let phone = details().rows(DeviceSide::Phone);
    assert_eq!(phone[3].0, "Laptop address");
    assert_eq!(phone[3].1, "10.1.0.2:9797");

    let laptop = details().rows(DeviceSide::Laptop);
    assert_eq!(laptop[3].0, "Phone address");
    assert_eq!(laptop[3].1, "10.1.0.2:9797");
}

#[test]
fn the_rows_come_in_a_fixed_order() {
    let rows = details().rows(DeviceSide::Laptop);
    let labels: Vec<&str> = rows.iter().map(|(label, _)| label.as_str()).collect();
    assert_eq!(
        labels,
        vec!["Path", "MTU", "DNS", "Phone address", "Link health"]
    );
    assert_eq!(rows[0].1, "Direct over hotspot");
    assert_eq!(rows[1].1, "1280");
    assert_eq!(rows[2].1, "through the tunnel");
}

#[test]
fn an_unknown_path_or_endpoint_says_unknown() {
    let rows = ConnectionDetails {
        path: None,
        verdict: None,
        dns: DnsMode::SystemResolver,
        endpoint: None,
    }
    .rows(DeviceSide::Laptop);
    assert_eq!(rows[0], ("Path".to_string(), "unknown".to_string()));
    assert_eq!(
        rows[2],
        (
            "DNS".to_string(),
            "system resolver (not changed)".to_string()
        )
    );
    assert_eq!(
        rows[3],
        ("Phone address".to_string(), "unknown".to_string())
    );
    assert_eq!(rows.len(), 4, "no link health row without a verdict");
}

#[test]
fn a_verdict_that_says_the_same_as_the_path_is_not_shown_twice() {
    let rows = ConnectionDetails {
        path: Some("Direct over hotspot".to_string()),
        verdict: Some("Direct over hotspot".to_string()),
        dns: DnsMode::ThroughTunnel,
        endpoint: None,
    }
    .rows(DeviceSide::Phone);
    assert_eq!(rows.len(), 4);
    assert!(!rows.iter().any(|(label, _)| label == "Link health"));
}

#[test]
fn the_lines_form_is_label_tab_value() {
    let lines = details().lines(DeviceSide::Phone);
    let first = lines.lines().next().unwrap_or_default();
    assert_eq!(first, "Path\tDirect over hotspot");
    assert_eq!(lines.lines().count(), 5);
}

#[test]
fn the_text_form_is_one_readable_line_per_row() {
    let lines = details().text_lines(DeviceSide::Phone);
    let rows: Vec<&str> = lines.lines().collect();
    assert_eq!(rows[0], "Path: Direct over hotspot");
    assert_eq!(rows[1], "MTU: 1280");
    assert_eq!(rows[2], "DNS: through the tunnel");
    assert_eq!(rows[3], "Laptop address: 10.1.0.2:9797");
    assert_eq!(rows[4], "Link health: Update Jafnify on the other device");
    assert_eq!(rows.len(), 5);
}
