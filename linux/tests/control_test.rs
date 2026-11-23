//! The local control socket between the tray daemon and the window process.

use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt;
use std::time::{Duration, Instant};

use jafnify_linux::control::{
    bind_socket, connect_at, request_at, InstanceLock, Request, Response, Snapshot, WindowPresence,
};

#[test]
fn requests_and_responses_round_trip_over_json() {
    let req = Request::Pair {
        code: "4321".to_string(),
    };
    let text = serde_json::to_string(&req).unwrap();
    assert_eq!(serde_json::from_str::<Request>(&text).unwrap(), req);

    let resp = Response::State(Box::new(Snapshot {
        state: "connected".to_string(),
        paired_name: Some("Phone - Test".to_string()),
        ..Snapshot::default()
    }));
    let text = serde_json::to_string(&resp).unwrap();
    assert_eq!(serde_json::from_str::<Response>(&text).unwrap(), resp);
}

#[test]
fn the_status_request_round_trips() {
    let req = Request::Status;
    let text = serde_json::to_string(&req).unwrap();
    assert_eq!(text, r#"{"cmd":"status"}"#);
    assert_eq!(serde_json::from_str::<Request>(&text).unwrap(), req);
}

#[test]
fn the_best_spot_commands_and_the_answer_round_trip() {
    for req in [
        Request::BestSpotStart,
        Request::BestSpotMark,
        Request::BestSpotStop,
    ] {
        let text = serde_json::to_string(&req).unwrap();
        assert_eq!(serde_json::from_str::<Request>(&text).unwrap(), req);
    }

    let resp = Response::BestSpot {
        running: true,
        active: true,
        lines: vec!["Now: 5G (advanced), -97 dBm, signal 3/4".to_string()],
    };
    let text = serde_json::to_string(&resp).unwrap();
    assert_eq!(serde_json::from_str::<Response>(&text).unwrap(), resp);
}

#[test]
fn an_older_snapshot_without_the_best_spot_fields_still_loads() {
    let mut value = serde_json::to_value(Snapshot::default()).unwrap();
    let object = value.as_object_mut().unwrap();
    object.remove("best_spot_running");
    object.remove("best_spot_active");
    object.remove("best_spot_lines");
    let old: Snapshot = serde_json::from_value(value).unwrap();
    assert!(!old.best_spot_running);
    assert!(!old.best_spot_active);
    assert!(old.best_spot_lines.is_empty());
}

/// The Stop button of the best-spot block stays available while the session
/// shows its marks, also when the two minutes of measuring are over.
#[test]
fn the_best_spot_session_shows_its_marks_after_the_two_minutes() {
    let over = Snapshot {
        best_spot_running: false,
        best_spot_active: true,
        ..Snapshot::default()
    };
    assert!(
        over.best_spot_showing(),
        "the session still shows its marks and its verdict"
    );

    let none = Snapshot::default();
    assert!(
        !none.best_spot_showing(),
        "no session shows nothing, so Stop has nothing to clear"
    );
}

#[test]
fn the_window_can_persist_its_expansion_state() {
    let req = Request::SetViewState {
        details_expanded: true,
        advanced_expanded: false,
    };
    let text = serde_json::to_string(&req).unwrap();
    assert_eq!(serde_json::from_str::<Request>(&text).unwrap(), req);

    let snapshot = Snapshot {
        details_expanded: true,
        advanced_expanded: true,
        ..Snapshot::default()
    };
    let text = serde_json::to_string(&snapshot).unwrap();
    assert_eq!(serde_json::from_str::<Snapshot>(&text).unwrap(), snapshot);
}

#[test]
fn an_older_snapshot_without_expansion_fields_still_loads() {
    let mut value = serde_json::to_value(Snapshot::default()).unwrap();
    let object = value.as_object_mut().unwrap();
    object.remove("details_expanded");
    object.remove("advanced_expanded");
    let old: Snapshot = serde_json::from_value(value).unwrap();
    assert!(!old.details_expanded);
    assert!(!old.advanced_expanded);
}

#[test]
fn an_older_snapshot_without_link_lost_still_loads_false() {
    let mut value = serde_json::to_value(Snapshot::default()).unwrap();
    value.as_object_mut().unwrap().remove("link_lost");
    let old: Snapshot = serde_json::from_value(value).unwrap();
    assert!(!old.link_lost);
}

#[test]
fn the_health_verdict_and_detail_round_trip() {
    let snapshot = Snapshot {
        health_line: Some("Direct over hotspot".to_string()),
        health_detail: Some("Last handshake 5 s ago".to_string()),
        code_expired: true,
        ..Snapshot::default()
    };
    let text = serde_json::to_string(&snapshot).unwrap();
    assert_eq!(serde_json::from_str::<Snapshot>(&text).unwrap(), snapshot);
}

#[test]
fn an_older_snapshot_without_health_still_loads() {
    let mut value = serde_json::to_value(Snapshot::default()).unwrap();
    let object = value.as_object_mut().unwrap();
    object.remove("health_line");
    object.remove("health_detail");
    object.remove("code_expired");
    let old: Snapshot = serde_json::from_value(value).unwrap();
    assert_eq!(old.health_line, None);
    assert_eq!(old.health_detail, None);
    assert!(!old.code_expired);
}

#[test]
fn the_window_can_persist_its_geometry() {
    let window = jafnify_linux::settings::WindowGeometry {
        width: 700,
        height: 500,
        x: Some(-40),
        y: Some(30),
    };
    let req = Request::SetWindowGeometry { window };
    let text = serde_json::to_string(&req).unwrap();
    assert_eq!(serde_json::from_str::<Request>(&text).unwrap(), req);

    let snapshot = Snapshot {
        window,
        ..Snapshot::default()
    };
    let text = serde_json::to_string(&snapshot).unwrap();
    assert_eq!(serde_json::from_str::<Snapshot>(&text).unwrap(), snapshot);
}

#[test]
fn the_link_lost_flag_round_trips() {
    let snapshot = Snapshot {
        link_lost: true,
        ..Snapshot::default()
    };
    let text = serde_json::to_string(&snapshot).unwrap();
    assert_eq!(serde_json::from_str::<Snapshot>(&text).unwrap(), snapshot);
}

#[test]
fn an_older_snapshot_without_theme_still_loads_system() {
    let mut value = serde_json::to_value(Snapshot::default()).unwrap();
    value.as_object_mut().unwrap().remove("theme");
    let old: Snapshot = serde_json::from_value(value).unwrap();
    assert_eq!(old.theme, "system");
}

#[test]
fn set_settings_round_trips_the_theme() {
    let req = Request::SetSettings {
        role: "joiner".to_string(),
        autostart: false,
        kill_switch: false,
        tray_only_when_closed: true,
        theme: Some("dark".to_string()),
        flags: BTreeMap::new(),
        nickname: None,
    };
    let text = serde_json::to_string(&req).unwrap();
    assert_eq!(serde_json::from_str::<Request>(&text).unwrap(), req);
}

#[test]
fn set_theme_round_trips_and_leaves_settings_alone() {
    let req = Request::SetTheme {
        theme: "light".to_string(),
    };
    let text = serde_json::to_string(&req).unwrap();
    assert_eq!(serde_json::from_str::<Request>(&text).unwrap(), req);
    assert!(text.contains("\"cmd\":\"set_theme\""), "{text}");
    assert!(!text.contains("autostart"), "{text}");
    assert!(!text.contains("kill_switch"), "{text}");
}

#[test]
fn an_older_set_settings_without_theme_means_keep_the_saved_theme() {
    let old: Request = serde_json::from_str(
        r#"{"cmd":"set_settings","role":"joiner","autostart":false,"kill_switch":false,"tray_only_when_closed":false}"#,
    )
    .unwrap();
    match old {
        Request::SetSettings { theme, .. } => assert_eq!(theme, None),
        other => panic!("expected SetSettings, got {other:?}"),
    }
}

#[test]
fn an_explicit_null_set_settings_theme_also_means_keep_the_saved_theme() {
    let old: Request = serde_json::from_str(
        r#"{"cmd":"set_settings","role":"joiner","autostart":false,"kill_switch":false,"tray_only_when_closed":false,"theme":null}"#,
    )
    .unwrap();
    match old {
        Request::SetSettings { theme, .. } => assert_eq!(theme, None),
        other => panic!("expected SetSettings, got {other:?}"),
    }
}

#[test]
fn a_state_snapshot_never_carries_a_private_key() {
    let snapshot = Snapshot {
        state: "connected".to_string(),
        status_text: "Connected".to_string(),
        paired_name: Some("Phone - Test".to_string()),
        paired: true,
        pair_id: Some("00112233445566778899aabbccddeeff".to_string()),
        peer_key_prefix: Some("abcd1234".to_string()),
        ..Snapshot::default()
    };
    let text = serde_json::to_string(&snapshot).unwrap();
    assert!(!text.contains("private_key"), "{text}");
    assert!(!text.contains("private"), "{text}");
}

#[test]
fn a_request_and_response_travel_over_a_real_socket() {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixListener;

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("jafnify/control.sock");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let listener = UnixListener::bind(&path).unwrap();
    let server = std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let mut line = String::new();
        BufReader::new(stream.try_clone().unwrap())
            .read_line(&mut line)
            .unwrap();
        let request: Request = serde_json::from_str(&line).unwrap();
        assert_eq!(request, Request::GetState);
        let answer = serde_json::to_string(&Response::Ok {
            note: Some("ok".to_string()),
        })
        .unwrap();
        let mut stream = stream;
        writeln!(stream, "{answer}").unwrap();
    });
    let response = request_at(&Request::GetState, &path).unwrap();
    assert_eq!(
        response,
        Response::Ok {
            note: Some("ok".to_string())
        }
    );
    server.join().unwrap();
}

#[test]
fn connecting_to_a_missing_socket_fails_quickly() {
    let dir = tempfile::tempdir().unwrap();
    let start = Instant::now();
    let err = connect_at(&dir.path().join("missing.sock")).unwrap_err();
    assert!(start.elapsed() < Duration::from_secs(2), "{err}");
}

#[test]
fn the_control_socket_is_0600_in_a_0700_directory() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("jafnify/control.sock");
    let _listener = bind_socket(&path).unwrap();
    let socket_mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    let dir_mode = std::fs::metadata(path.parent().unwrap())
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(socket_mode, 0o600);
    assert_eq!(dir_mode, 0o700);
}

#[test]
fn the_lock_allows_only_one_instance() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tray.lock");
    let first = InstanceLock::acquire(&path).unwrap().unwrap();
    assert!(
        InstanceLock::acquire(&path).unwrap().is_none(),
        "a second daemon must not start"
    );
    drop(first);
    assert!(
        InstanceLock::acquire(&path).unwrap().is_some(),
        "the lock is released with the process"
    );
}

#[test]
fn the_second_instance_says_it_is_running_and_opens_the_one_window() {
    // The lock seam: the loser of the lock takes this path. It prints the
    // core's line and tries to open the existing window; a GUI that is not
    // installed is harmless and the exit code stays 0.
    let mut said = String::new();
    let opened = std::cell::Cell::new(false);
    jafnify_linux::daemon::second_instance(
        |line| said.push_str(line),
        || {
            opened.set(true);
            Ok(())
        },
    );
    assert_eq!(said, "Jafnify is already running in the tray.");
    assert!(opened.get());
    // A missing GUI must not panic or change the line.
    jafnify_linux::daemon::second_instance(
        |line| assert_eq!(line, "Jafnify is already running in the tray."),
        || Err(std::io::Error::from(std::io::ErrorKind::NotFound)),
    );
}

#[test]
fn window_presence_expires_and_can_be_closed_explicitly() {
    let now = Instant::now();
    let mut presence = WindowPresence::new(Duration::from_secs(2));
    assert!(!presence.is_open(now));
    presence.seen(now);
    assert!(presence.is_open(now));
    assert!(presence.is_open(now + Duration::from_secs(1)));
    assert!(!presence.is_open(now + Duration::from_secs(3)));
    presence.seen(now + Duration::from_secs(3));
    assert!(presence.is_open(now + Duration::from_secs(3)));
    presence.closed();
    assert!(!presence.is_open(now + Duration::from_secs(3)));
}

#[test]
fn the_get_state_json_carries_the_passive_wifi_leg() {
    let snapshot = Snapshot {
        wifi_ms: Some(42),
        wifi_loss_pct: Some(3),
        wifi_n: Some(30),
        ..Snapshot::default()
    };
    let value = serde_json::to_value(&snapshot).unwrap();
    assert_eq!(value["wifi_ms"], serde_json::json!(42));
    assert_eq!(value["wifi_loss_pct"], serde_json::json!(3));
    assert_eq!(value["wifi_n"], serde_json::json!(30));

    // An older window that does not know the new fields still loads.
    let mut value = serde_json::to_value(Snapshot::default()).unwrap();
    let object = value.as_object_mut().unwrap();
    object.remove("wifi_ms");
    object.remove("wifi_loss_pct");
    object.remove("wifi_n");
    let old: Snapshot = serde_json::from_value(value).unwrap();
    assert_eq!(old.wifi_ms, None);
    assert_eq!(old.wifi_loss_pct, None);
    assert_eq!(old.wifi_n, None);
}

#[test]
fn the_get_state_json_carries_the_heat_and_the_good_time() {
    let snapshot = Snapshot {
        phone_hot_temp_c_x10: Some(431),
        phone_good_time: true,
        ..Snapshot::default()
    };
    let value = serde_json::to_value(&snapshot).unwrap();
    assert_eq!(value["phone_hot_temp_c_x10"], serde_json::json!(431));
    assert_eq!(value["phone_good_time"], serde_json::json!(true));

    // An older window that does not know the new fields still loads.
    let mut value = serde_json::to_value(Snapshot::default()).unwrap();
    let object = value.as_object_mut().unwrap();
    object.remove("phone_hot_temp_c_x10");
    object.remove("phone_good_time");
    let old: Snapshot = serde_json::from_value(value).unwrap();
    assert!(old.phone_hot_temp_c_x10.is_none());
    assert!(!old.phone_good_time);
}
