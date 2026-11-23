//! Flag values ride in the existing `SetSettings` request and `Snapshot`.
//! A 0.1.1 tray and a 0.2 daemon must parse each other: a missing `flags`
//! field reads as empty, and the other side's unknown field is ignored.

use std::collections::BTreeMap;

use jafnify_core::phone_stats::{NetworkKind, PhoneStats};
use jafnify_linux::control::{Request, Snapshot};

/// The 0.1.1 `SetSettings` shape, frozen as a literal message.
const OLD_SET_SETTINGS: &str = r#"{"cmd":"set_settings","role":"joiner","autostart":false,"kill_switch":true,"tray_only_when_closed":false}"#;

/// The 0.1.1 reader had no `flags` field and no `deny_unknown_fields`, so a
/// 0.2 message must still parse into this shape.
#[derive(Debug, serde::Deserialize)]
struct SetSettingsV011 {
    role: String,
    autostart: bool,
    kill_switch: bool,
    tray_only_when_closed: bool,
    #[serde(default)]
    theme: Option<String>,
}

/// The 0.1.1 `Snapshot` shape (frozen field list). Values are present in a
/// 0.2 message; only the new `flags` key is extra. The fields mirror the old
/// wire shape even when this test reads only a few of them.
#[allow(dead_code)]
#[derive(Debug, serde::Deserialize)]
struct SnapshotV011 {
    state: String,
    status_text: String,
    paired_name: Option<String>,
    paired: bool,
    denied: bool,
    retry_reason: Option<String>,
    no_peer: bool,
    connected_secs: Option<u64>,
    traffic: Option<(u64, u64)>,
    last_connected_ago: Option<u64>,
    retry_secs_left: Option<u64>,
    health_line: Option<String>,
    health_detail: Option<String>,
    foreign_tunnel: bool,
    code_expired: bool,
    firewall: Option<String>,
    firewall_iface: Option<String>,
    dns_override: Option<String>,
    last_error: Option<String>,
    role: String,
    autostart: bool,
    kill_switch: bool,
    tray_only_when_closed: bool,
    details_expanded: bool,
    advanced_expanded: bool,
    terms_ok: bool,
    ufw_active: bool,
    pair_id: Option<String>,
    peer_key_prefix: Option<String>,
    current_session: jafnify_linux::session::SessionStats,
    session_history: Vec<jafnify_linux::session::SessionRecord>,
    connection_log: Vec<String>,
    polkit_note: bool,
    link_lost: bool,
    no_network: bool,
    theme: String,
    window: jafnify_linux::settings::WindowGeometry,
}

fn request_with_flags() -> Request {
    Request::SetSettings {
        role: "sharer".to_string(),
        autostart: true,
        kill_switch: false,
        tray_only_when_closed: true,
        theme: Some("dark".to_string()),
        flags: BTreeMap::from([("usage_statistics".to_string(), true)]),
        nickname: None,
    }
}

#[test]
fn a_0_1_1_set_settings_without_flags_still_parses() {
    let old: Request = serde_json::from_str(OLD_SET_SETTINGS).unwrap();
    match old {
        Request::SetSettings {
            role,
            autostart,
            kill_switch,
            tray_only_when_closed,
            theme,
            flags,
            nickname,
        } => {
            assert_eq!(role, "joiner");
            assert!(!autostart);
            assert!(kill_switch);
            assert!(!tray_only_when_closed);
            assert_eq!(theme, None);
            assert!(flags.is_empty(), "a missing flags field means no change");
            assert!(nickname.is_none(), "an old request carries no name");
        }
        other => panic!("expected SetSettings, got {other:?}"),
    }
}

#[test]
fn a_0_1_1_snapshot_without_flags_still_parses() {
    let mut value = serde_json::to_value(Snapshot::default()).unwrap();
    value.as_object_mut().unwrap().remove("flags");
    let old: Snapshot = serde_json::from_value(value).unwrap();
    assert!(
        old.flags.is_empty(),
        "a missing flags field means every row reads its default"
    );
}

#[test]
fn a_0_2_set_settings_still_parses_in_the_0_1_1_shape() {
    let text = serde_json::to_string(&request_with_flags()).unwrap();
    assert!(text.contains("\"flags\""), "{text}");
    let old: SetSettingsV011 = serde_json::from_str(&text).unwrap();
    assert_eq!(old.role, "sharer");
    assert!(old.autostart);
    assert!(!old.kill_switch);
    assert!(old.tray_only_when_closed);
    assert_eq!(old.theme.as_deref(), Some("dark"));
}

#[test]
fn a_0_2_snapshot_still_parses_in_the_0_1_1_shape() {
    let snapshot = Snapshot {
        state: "connected".to_string(),
        status_text: "Connected".to_string(),
        paired: true,
        flags: BTreeMap::from([("usage_statistics".to_string(), true)]),
        ..Snapshot::default()
    };
    let text = serde_json::to_string(&snapshot).unwrap();
    assert!(text.contains("\"flags\""), "{text}");
    let old: SnapshotV011 = serde_json::from_str(&text).unwrap();
    assert_eq!(old.state, "connected");
    assert!(old.paired);
}

/// The 0.1.1 parser shape: an unknown `cmd` is an error, not a panic.
#[derive(Debug, serde::Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
enum RequestV011 {
    GetState,
}

#[test]
fn the_0_1_1_parser_rejects_the_set_flag_literal() {
    let text = r#"{"cmd":"set_flag","id":"x","value":true}"#;
    assert!(
        serde_json::from_str::<RequestV011>(text).is_err(),
        "the 0.1.1 daemon answers bad request, it does not panic"
    );
}

#[test]
fn set_flag_round_trips_on_the_wire() {
    let req = Request::SetFlag {
        id: "x".to_string(),
        value: true,
    };
    let text = serde_json::to_string(&req).unwrap();
    assert_eq!(text, r#"{"cmd":"set_flag","id":"x","value":true}"#);
    assert_eq!(serde_json::from_str::<Request>(&text).unwrap(), req);
}

#[test]
fn a_0_1_1_snapshot_without_phone_stats_still_parses() {
    let mut value = serde_json::to_value(Snapshot::default()).unwrap();
    let map = value.as_object_mut().unwrap();
    for key in [
        "flags",
        "phone_stats",
        "phone_stats_age_secs",
        "phone_stats_unavailable",
    ] {
        map.remove(key);
    }
    let old: Snapshot = serde_json::from_value(value).unwrap();
    assert!(old.phone_stats.is_none());
    assert_eq!(old.phone_stats_age_secs, None);
    assert!(!old.phone_stats_unavailable);
}

#[test]
fn a_snapshot_with_phone_stats_round_trips_and_an_old_shape_ignores_it() {
    let stats = PhoneStats {
        net: NetworkKind::Mobile5g,
        signal: Some(3),
        ..Default::default()
    };
    let snapshot = Snapshot {
        phone_stats: Some(stats.clone()),
        phone_stats_age_secs: Some(2),
        phone_stats_unavailable: true,
        ..Snapshot::default()
    };
    let text = serde_json::to_string(&snapshot).unwrap();
    assert!(text.contains("phone_stats"));
    let back: Snapshot = serde_json::from_str(&text).unwrap();
    assert_eq!(back.phone_stats, Some(stats));
    assert_eq!(back.phone_stats_age_secs, Some(2));
    assert!(back.phone_stats_unavailable);
    let old: SnapshotV011 = serde_json::from_str(&text).unwrap();
    assert_eq!(old.state, "");
}

#[test]
fn flags_round_trip_through_the_current_set_settings() {
    let req = request_with_flags();
    let text = serde_json::to_string(&req).unwrap();
    assert_eq!(serde_json::from_str::<Request>(&text).unwrap(), req);
}
