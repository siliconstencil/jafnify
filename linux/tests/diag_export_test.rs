//! The Linux diagnostics export: the connection log, the journal tail and the
//! application log tail, redacted once.
//!
//! Nothing here touches the operator's state directory: the tails are read
//! from temp files and the bundle is built from the pieces the daemon has.

use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use jafnify_core::redact::{PUBLIC_IP, MAC};
use jafnify_linux::connection_log::{self, ConnectionLog};
use jafnify_linux::control::{Request, Response};
use jafnify_linux::export;
use jafnify_linux::tray::TrayCommand;

/// 2023-11-14 22:13:20 UTC.
const T: i64 = 1_700_000_000_000;

/// A publicly routable peer endpoint.
const PUBLIC_ENDPOINT: &str = "203.0.113.5:9797";
/// Another publicly routable address, for the journal.
const PUBLIC_ADDR: &str = "198.51.100.7";
/// A LAN address: it stays.
const LAN_ADDR: &str = "192.168.1.20";
/// The peer's display name.
const PEER_NAME: &str = "Phone - Blue Falcon";
/// A 64-character hex key (the WireGuard UAPI encoding).
const HEX_KEY: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
/// A 44-character base64 key.
const B64_KEY: &str = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";

fn line(text: &str) -> String {
    text.to_string()
}

#[test]
fn the_export_replaces_a_public_address_and_a_key_and_keeps_nothing_of_the_original() {
    let connection_log = vec![line(&format!(
        "2023-11-14 22:13:20 UTC reconnected at 2023-11-14 22:13:20 UTC after 0:01:05 (peer {PUBLIC_ENDPOINT})"
    ))];
    let journal = vec![line(&format!(
        "ts=2023-11-14T22:13:20.000Z kind=STATS peer={PUBLIC_ADDR}:9797 local={LAN_ADDR}"
    ))];
    let app_log = vec![line(&format!(
        "2023-11-14T22:13:20Z INFO uapi private_key={HEX_KEY} preshared_key={B64_KEY}"
    ))];

    let text = export::bundle(&connection_log, &journal, &app_log, &[], &[], "");

    assert!(text.contains("<public-ip>"), "{text}");
    assert!(text.contains("[redacted]"), "{text}");
    assert!(!text.contains(PUBLIC_ENDPOINT), "{text}");
    assert!(!text.contains(PUBLIC_ADDR), "{text}");
    assert!(!text.contains(HEX_KEY), "{text}");
    assert!(!text.contains(B64_KEY), "{text}");
}

#[test]
fn a_private_address_survives_the_export_unchanged() {
    let journal = vec![line(&format!(
        "ts=2023-11-14T22:13:20.000Z kind=STATS peer={LAN_ADDR}:9797 local=10.9.9.2"
    ))];

    let text = export::bundle(&[], &journal, &[], &[], &[], "");

    assert!(text.contains(LAN_ADDR), "{text}");
    assert!(text.contains("10.9.9.2"), "{text}");
    assert!(!text.contains("<public-ip>"), "{text}");
}

#[test]
fn the_peer_name_gets_a_label_and_the_endpoint_is_redacted_by_value() {
    let connection_log = vec![line(&format!(
        "2023-11-14 22:13:20 UTC reconnected (peer {PUBLIC_ENDPOINT})"
    ))];
    let journal = vec![line(&format!(
        "ts=2023-11-14T22:13:20.000Z kind=EVENT peer={PEER_NAME}"
    ))];

    // An endpoint is never an alias (see `daemon.rs`): the peer identity is
    // the human name, the key prefix and the pair id.
    let text = export::bundle(
        &connection_log,
        &journal,
        &[],
        &[],
        &[PEER_NAME, "AbCdEfGhIjKl", "pair-9f2a"],
        "",
    );

    assert!(text.contains("peer-1"), "{text}");
    assert!(!text.contains(PEER_NAME), "{text}");
    // The endpoint still reaches the address rule: it is a placeholder, never
    // a peer label, so an address-shaped alias cannot override it.
    assert!(text.contains(&format!("peer {PUBLIC_IP}")), "{text}");
    assert!(!text.contains(PUBLIC_ENDPOINT), "{text}");
    assert!(!text.contains("peer peer-1"), "{text}");
}

#[test]
fn a_private_endpoint_survives_a_bundle_that_has_a_named_peer() {
    // The alias list the daemon now registers: the human name, the key prefix
    // and the pair id. An address is never an alias, so a private endpoint
    // reaches the export unchanged even when the peer is named.
    let private_endpoint = "10.20.30.40:9797";
    let connection_log = vec![line(&format!(
        "2023-11-14 22:13:20 UTC reconnected (peer {private_endpoint})"
    ))];
    let journal = vec![line(&format!(
        "ts=2023-11-14T22:13:20.000Z kind=EVENT peer={PEER_NAME}"
    ))];
    let aliases = [PEER_NAME, "AbCdEfGhIjKl", "pair-9f2a"];

    let text = export::bundle(&connection_log, &journal, &[], &[], &aliases, "");

    assert!(text.contains(private_endpoint), "{text}");
    assert!(text.contains("peer-1"), "{text}");
    assert!(!text.contains(PEER_NAME), "{text}");
}

#[test]
fn the_sections_come_in_the_order_of_the_plan() {
    let connection_log = vec![line("2023-11-14 22:13:20 UTC state Connected -> Retry")];
    let journal = vec![line("ts=2023-11-14T22:13:20.000Z kind=EVENT event=state")];
    let app_log = vec![line("2023-11-14T22:13:20Z INFO connected")];

    let text = export::bundle(&connection_log, &journal, &app_log, &[], &[], "");

    assert!(text.starts_with("Jafnify diagnostics\n"), "{text}");
    assert!(
        text.contains("Redacted: public IP addresses, SSIDs and peer identifiers are replaced with placeholders."),
        "{text}"
    );
    assert!(
        text.contains("Private and LAN addresses are kept. Keys are never included."),
        "{text}"
    );

    let connection = text
        .find("--- connection log (1 lines) ---")
        .expect("header");
    let journal_at = text.find("--- journal (last 1 lines) ---").expect("header");
    let app_log_at = text
        .find("--- application log (last 1 lines) ---")
        .expect("header");
    assert!(connection < journal_at, "{text}");
    assert!(journal_at < app_log_at, "{text}");
}

#[test]
fn an_export_with_every_identifier_keeps_none_of_them() {
    // One fixture per identifier the checklist names: private key, preshared
    // key, pairing code, public IP, SSID, MAC/BSSID, peer id, pair id, peer
    // name and the machine name. None of them may survive the export.
    const PSK_B64: &str = "BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB=";
    const SSID_NAME: &str = "Blue Falcon 5G";
    const MAC_ADDR: &str = "aa:bb:cc:dd:ee:ff";
    const BSSID_ADDR: &str = "11:22:33:44:55:66";
    const PEER_ID: &str = "abcdef0123456789";
    const PAIR_ID: &str = "pair-9f2a";
    const HOST_NAME: &str = "example-laptop";
    const PAIRING_CODE: &str = "4321";

    let connection_log = vec![
        line(&format!(
            "2023-11-14 22:13:20 UTC reconnected (peer {PUBLIC_ENDPOINT})"
        )),
        line(&format!("2023-11-14 22:13:20 UTC ssid={SSID_NAME}")),
        line(&format!(
            "2023-11-14 22:13:20 UTC bssid={BSSID_ADDR} mac={MAC_ADDR}"
        )),
    ];
    let journal = vec![
        line(&format!(
            "ts=2023-11-14T22:13:20.000Z kind=UAPI private_key={HEX_KEY} preshared_key={PSK_B64} public_key={B64_KEY}"
        )),
        line(&format!(
            "ts=2023-11-14T22:13:20.000Z kind=PAIR code={PAIRING_CODE} peer_id={PEER_ID} pair_id={PAIR_ID}"
        )),
    ];
    let app_log = vec![
        line(&format!(
            "2023-11-14T22:13:20Z INFO listening on {HOST_NAME} interface wlan0"
        )),
        line(&format!(
            "2023-11-14T22:13:20Z INFO paired with \"{PEER_NAME}\""
        )),
    ];
    let link = vec![line(&format!(
        "JFD1 1 drop at=1700000000000 ssid={SSID_NAME} bssid={BSSID_ADDR}"
    ))];

    let text = export::bundle(
        &connection_log,
        &journal,
        &app_log,
        &link,
        &[PEER_NAME, PEER_ID, PAIR_ID],
        HOST_NAME,
    );

    for identifier in [
        PUBLIC_ENDPOINT,
        HEX_KEY,
        B64_KEY,
        PSK_B64,
        SSID_NAME,
        MAC_ADDR,
        BSSID_ADDR,
        PEER_ID,
        PAIR_ID,
        PEER_NAME,
        HOST_NAME,
        PAIRING_CODE,
    ] {
        assert!(
            !text.contains(identifier),
            "identifier {identifier:?} survived the export: {text}"
        );
    }
    for placeholder in [
        "<public-ip>",
        "[redacted]",
        "<ssid>",
        "<mac>",
        "<code>",
        "peer-1",
        "<host>",
    ] {
        assert!(
            text.contains(placeholder),
            "placeholder {placeholder} is missing: {text}"
        );
    }
}

#[test]
fn the_tails_are_capped() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("diag.log");
    let mut body = String::new();
    for i in 1..=50 {
        body.push_str(&format!("line {i}\n"));
    }
    std::fs::write(&path, body).unwrap();

    let tail = export::tail_lines(&path, 10);
    assert_eq!(tail.len(), 10, "{tail:?}");
    assert_eq!(tail.first().map(String::as_str), Some("line 41"));
    assert_eq!(tail.last().map(String::as_str), Some("line 50"));

    // The caps of the export are fixed and small.
    const { assert!(export::JOURNAL_TAIL_LINES > 0 && export::JOURNAL_TAIL_LINES <= 1_000) };
    const { assert!(export::APP_LOG_TAIL_LINES > 0 && export::APP_LOG_TAIL_LINES <= 1_000) };
}

#[test]
fn the_link_evidence_survives_the_export_and_the_redactor() {
    // A fixture link.tsv with more lines than the cap: the export keeps the
    // last 60, oldest first, and never an address.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("link.tsv");
    let mut body = String::new();
    for i in 0..80u64 {
        body.push_str(&format!(
            "JFL1 1 at={} sig=-60 avg=-62 rate=8667 retry={} fail=0 bloss=0\n",
            1_700_000_000_000i64 + i as i64 * 1_000,
            i
        ));
    }
    std::fs::write(&path, body).unwrap();

    let link = export::link_log_tail_in(dir.path());
    assert_eq!(link.len(), export::LINK_LOG_TAIL_LINES);
    assert!(link[0].contains("at=1700000020000"), "{link:?}");
    assert!(link.iter().all(|line| !line.contains(':')), "{link:?}");

    // A drop line keeps every evidence key through the redactor, and a token
    // that looks like a MAC address is still replaced.
    let drop = line("JFD1 1 drop at=1700000000000 reason=no-network net=wifi sig=3 temp=400 batt=50 rtt=20 thr=0 wifi_ms=40 loss=2 n=20 range_dbm=-81 rate_min=65 retry_d=320 fail_d=0 bloss_d=7 ploss=3 prtt=41");
    let dirty = line("JFL1 1 at=1700000000000 sig=-60 peer=aa:bb:cc:dd:ee:ff");
    let mut section = link.clone();
    section.push(drop);
    section.push(dirty);

    let text = export::bundle(&[], &[], &[], &section, &[], "");

    assert!(
        text.contains("--- link evidence (last 62 lines) ---"),
        "{text}"
    );
    for key in [
        "range_dbm=-81",
        "rate_min=65",
        "retry_d=320",
        "fail_d=0",
        "bloss_d=7",
        "ploss=3",
        "prtt=41",
    ] {
        assert!(
            text.contains(key),
            "the drop evidence key {key} survives: {text}"
        );
    }
    assert!(!text.contains("aa:bb:cc:dd:ee:ff"), "{text}");
    assert!(text.contains(MAC), "the MAC-like token is redacted: {text}");
}

#[test]
fn a_missing_link_file_gives_no_section_and_no_panic() {
    let dir = tempfile::tempdir().unwrap();
    assert!(export::link_log_tail_in(dir.path()).is_empty());
}

#[test]
fn a_missing_file_gives_no_section_and_no_panic() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("missing.log");

    assert!(export::tail_lines(&missing, 10).is_empty());
    assert!(export::tail_lines(&missing, 0).is_empty());

    let connection_log = vec![line("2023-11-14 22:13:20 UTC state Connected -> Retry")];
    let text = export::bundle(&connection_log, &[], &[], &[], &[], "");
    assert!(text.contains("--- connection log (1 lines) ---"), "{text}");
    assert!(!text.contains("--- journal"), "{text}");
    assert!(!text.contains("--- application log"), "{text}");
}

#[test]
fn a_directory_is_not_a_file_and_does_not_panic() {
    let dir = tempfile::tempdir().unwrap();
    assert!(export::tail_lines(dir.path(), 10).is_empty());
}

#[test]
fn the_tail_keeps_the_last_line_without_a_newline() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("jafnify.log");
    std::fs::write(&path, "first\nsecond").unwrap();

    let tail = export::tail_lines(&path, 5);
    assert_eq!(tail, vec!["first".to_string(), "second".to_string()]);
}

#[test]
fn the_journal_paths_are_the_rotated_sibling_first() {
    let paths = export::journal_paths_for(Path::new("/tmp/jafnify/diag.log"));
    assert_eq!(
        paths,
        vec![
            Path::new("/tmp/jafnify/diag.1.log").to_path_buf(),
            Path::new("/tmp/jafnify/diag.log").to_path_buf(),
        ]
    );
}

#[test]
fn the_copy_text_of_the_connection_log_is_redacted() {
    let mut log = ConnectionLog::new();
    log.push(
        "reconnect",
        &format!("reconnected (peer {PUBLIC_ENDPOINT})"),
        T,
    );

    let copied = log.copy_text(connection_log::MAX_LINES);

    assert!(!copied.contains(PUBLIC_ENDPOINT), "{copied}");
    assert!(copied.contains("<public-ip>"), "{copied}");
}

#[test]
fn the_saved_log_is_redacted_and_private() {
    let dir = tempfile::tempdir().unwrap();
    let path = connection_log::save_log_at(
        dir.path(),
        &format!("peer {PUBLIC_ENDPOINT} key {HEX_KEY}"),
        T,
    )
    .unwrap();

    let content = std::fs::read_to_string(&path).unwrap();
    assert!(!content.contains(PUBLIC_ENDPOINT), "{content}");
    assert!(!content.contains(HEX_KEY), "{content}");
    assert!(content.contains("<public-ip>"), "{content}");
    assert!(content.contains("[redacted]"), "{content}");
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
}

#[test]
fn the_tray_copy_entry_asks_for_the_d3_diagnostics() {
    assert_eq!(
        TrayCommand::CopyDiagnostics.request(),
        Some(Request::Diagnostics),
        "the tray entry uses the daemon's redacted diagnostics text"
    );
}

#[test]
fn the_usage_store_is_never_part_of_the_export() {
    // The export only has three optional sections; there is no usage section
    // and no path to the usage store. A usage file can sit next to the logs
    // and still never be read by this builder.
    let text = export::bundle(&[], &[], &[], &[], &[], "");
    assert!(!text.contains("JFU1"), "{text}");
    assert!(!text.contains("Shared today"), "{text}");
    assert!(!text.contains("usage_statistics"), "{text}");

    let usage = Path::new("/home/u/.config/jafnify/usage.tsv");
    assert!(
        export::journal_paths_for(Path::new("/tmp/jafnify/diag.log"))
            .iter()
            .all(|path| path != usage)
    );
    assert_ne!(export::app_log_path(), usage);
}

#[test]
fn the_diagnostics_request_and_response_round_trip() {
    let request = Request::Diagnostics;
    let text = serde_json::to_string(&request).unwrap();
    assert!(text.contains("\"cmd\":\"diagnostics\""), "{text}");
    assert_eq!(serde_json::from_str::<Request>(&text).unwrap(), request);

    let response = Response::Diagnostics {
        text: "Jafnify diagnostics\n".to_string(),
    };
    let text = serde_json::to_string(&response).unwrap();
    assert!(text.contains("\"result\":\"diagnostics\""), "{text}");
    assert_eq!(serde_json::from_str::<Response>(&text).unwrap(), response);
}
