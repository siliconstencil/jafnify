//! What the Linux window shows for each state: one pure function from the app
//! state to texts and controls, the twin of the phone's render(). The GUI only
//! draws the result.

use jafnify_core::snapshot::{
    classify_path, CodeView, Freshness, Health, PeerPath, RetryView, Snapshot,
};
use jafnify_core::ConnectionState;
use jafnify_linux::ui_text::{format_timestamp_or_none, link_counts_line, LINK_LOST};
use jafnify_linux::view::{
    details_line, device_line, forgotten_line, peer_line, view, Primary, ViewInput, POLKIT_NOTE,
};

const NAME: &str = "Phone - Icy Parrot";

fn paired(state: ConnectionState) -> ViewInput {
    ViewInput {
        paired_name: Some(NAME.to_string()),
        ..ViewInput::new(state)
    }
}

#[test]
fn unpaired_idle_asks_for_the_code_and_explains_the_app() {
    let v = view(&ViewInput::new(ConnectionState::Idle));
    assert_eq!(v.title, "Jafnify");
    assert!(v.intro.as_deref().unwrap().contains("encrypted tunnel"));
    assert!(v.intro.as_deref().unwrap().contains("hotspot"));
    assert_eq!(v.status, "Not sharing");
    assert!(v.show_code_entry);
    assert!(v.code_hint.as_deref().unwrap().contains("Share internet"));
    assert_eq!(v.primary, Primary::Connect);
    assert!(!v.primary_enabled, "no code yet");
    assert!(
        !v.polkit_note,
        "no privileged call is imminent by state alone"
    );
    assert!(!v.show_pair_row);
    assert!(v.last.is_none());
}

#[test]
fn the_connect_button_needs_all_four_digits() {
    let mut i = ViewInput::new(ConnectionState::Idle);
    i.code_digits = 3;
    assert!(!view(&i).primary_enabled);
    i.code_digits = 4;
    assert!(view(&i).primary_enabled);
}

#[test]
fn paired_idle_shows_the_name_last_connected_and_a_connect_button() {
    let v = view(&ViewInput {
        last_connected_ago: Some(5 * 60 + 30),
        ..paired(ConnectionState::Idle)
    });
    assert_eq!(v.title, NAME);
    assert!(v.intro.is_none());
    assert_eq!(v.status, "Not sharing");
    assert_eq!(
        v.last.as_deref(),
        Some("Last connected: Phone - Icy Parrot, 5 min ago")
    );
    assert!(!v.show_code_entry);
    assert_eq!(v.primary, Primary::Connect);
    assert!(v.primary_enabled);
    assert!(v.show_pair_row);
    assert!(!v.polkit_note);
}

#[test]
fn paired_idle_without_a_time_still_names_the_device() {
    let v = view(&paired(ConnectionState::Idle));
    assert_eq!(
        v.last.as_deref(),
        Some("Last connected: Phone - Icy Parrot")
    );
}

#[test]
fn pairing_a_new_device_asks_for_a_code_under_the_app_name() {
    let v = view(&ViewInput {
        pairing_new: true,
        ..paired(ConnectionState::Idle)
    });
    assert_eq!(v.title, "Jafnify");
    assert!(v.show_code_entry);
    assert!(v.show_cancel_entry);
    assert!(!v.show_pair_row);
    assert!(v.last.is_none());
    assert_eq!(v.primary, Primary::Connect);
}

#[test]
fn searching_offers_cancel_without_guessing_the_password_note() {
    let v = view(&paired(ConnectionState::Searching));
    assert_eq!(v.status, "Searching for the other device");
    assert_eq!(v.primary, Primary::Cancel);
    assert!(!v.polkit_note);
    assert!(!v.show_code_entry);
    assert!(!v.warning);
}

#[test]
fn searching_warns_about_the_password_when_the_attempt_publishes_it() {
    let v = view(&ViewInput {
        polkit_note: true,
        ..paired(ConnectionState::Searching)
    });
    assert_eq!(v.status, "Searching for the other device");
    assert_eq!(v.primary, Primary::Cancel);
    assert!(v.polkit_note);
    assert!(!v.show_code_entry);
    assert!(!v.warning);
}

#[test]
fn a_long_search_says_what_to_check_and_that_it_keeps_searching() {
    let v = view(&ViewInput {
        no_peer: true,
        ..paired(ConnectionState::Searching)
    });
    assert!(v.status.starts_with("No device found"), "{}", v.status);
    assert!(
        v.status.contains("same hotspot or Wi-Fi network"),
        "{}",
        v.status
    );
    assert!(v.status.contains("still searching"), "{}", v.status);
}

#[test]
fn a_known_search_waits_for_the_phone_instead_of_saying_no_device() {
    let v = view(&ViewInput {
        waiting_for_phone: true,
        no_peer: true,
        ..paired(ConnectionState::Searching)
    });
    assert_eq!(v.status, "Waiting for phone...");
    assert!(!v.warning);
}

#[test]
fn a_paired_boot_without_a_network_waits_for_the_phone() {
    let v = view(&ViewInput {
        error: Some("No LAN interface".to_string()),
        ..paired(ConnectionState::Idle)
    });
    assert_eq!(v.status, "Waiting for phone...");
    assert!(!v.warning);
}

#[test]
fn pairing_and_connecting_have_their_own_words() {
    assert_eq!(view(&paired(ConnectionState::Pairing)).status, "Pairing");
    assert_eq!(
        view(&paired(ConnectionState::Connecting)).status,
        "Connecting"
    );
    assert_eq!(
        view(&paired(ConnectionState::Connecting)).primary,
        Primary::Cancel
    );
}

#[test]
fn connected_shows_the_device_the_clock_and_the_data() {
    let v = view(&ViewInput {
        connected_secs: Some(12 * 60 + 34),
        traffic: Some((1_258_291, 340 * 1024)),
        ..paired(ConnectionState::Connected)
    });
    assert_eq!(v.status, "Connected - Phone - Icy Parrot");
    assert_eq!(v.elapsed.as_deref(), Some("Connected for 12:34"));
    assert_eq!(v.traffic.as_deref(), Some("Sent 1.2 MB   Received 340 KB"));
    assert_eq!(v.primary, Primary::Disconnect);
    assert!(
        !v.polkit_note,
        "the tunnel is up, nothing more to authorise"
    );
    assert!(v.show_pair_row);
    assert!(v.last.is_none());
}

#[test]
fn connected_without_a_name_or_counters_degrades_quietly() {
    let v = view(&ViewInput::new(ConnectionState::Connected));
    assert_eq!(v.status, "Connected");
    assert!(v.elapsed.is_none());
    assert!(v.traffic.is_none());
}

#[test]
fn a_connected_link_loss_is_the_exact_waiting_sentence_with_a_warning() {
    let v = view(&ViewInput {
        link_lost: true,
        ..paired(ConnectionState::Connected)
    });
    assert_eq!(v.status, LINK_LOST);
    assert_eq!(LINK_LOST, "Wi-Fi link lost - waiting for the hotspot");
    assert!(v.warning);
    assert_eq!(v.primary, Primary::Disconnect);

    // A healthy Connected line is unchanged.
    let normal = view(&paired(ConnectionState::Connected));
    assert_eq!(normal.status, "Connected - Phone - Icy Parrot");
    assert!(!normal.warning);
}

#[test]
fn a_link_loss_does_not_rewrite_a_non_connected_status() {
    let v = view(&ViewInput {
        link_lost: true,
        retry_secs_left: Some(7),
        ..paired(ConnectionState::Retry)
    });
    assert_eq!(v.status, "Connection lost - retrying in 7 s");
}

#[test]
fn an_expired_code_says_so_instead_of_waiting_for_the_phone() {
    let v = view(&ViewInput {
        waiting_for_phone: true,
        code_expired: true,
        ..paired(ConnectionState::Searching)
    });
    assert_eq!(v.status, "Code expired");
    // An expired code is an error the user must act on: it carries the
    // warning mark, never colour alone.
    assert!(v.warning);
    assert_eq!(v.primary, Primary::Cancel);

    for state in [
        ConnectionState::Searching,
        ConnectionState::Pairing,
        ConnectionState::Connecting,
    ] {
        let v = view(&ViewInput {
            code_expired: true,
            ..paired(state)
        });
        assert_eq!(v.status, "Code expired", "{state:?}");
    }
}

#[test]
fn a_completed_pairing_is_not_told_that_its_code_expired() {
    let v = view(&ViewInput {
        code_expired: true,
        ..paired(ConnectionState::Connected)
    });
    assert_eq!(v.status, "Connected - Phone - Icy Parrot");
    assert!(!v.warning);
}

#[test]
fn the_health_verdict_is_shown_under_the_status_when_present() {
    let verdict = Health {
        handshake: jafnify_core::snapshot::Freshness::Age(12),
        path: jafnify_core::snapshot::PeerPath::DirectHotspot,
        old_peer: false,
    }
    .verdict();
    let v = view(&ViewInput {
        health: verdict.clone(),
        ..paired(ConnectionState::Connected)
    });
    assert_eq!(v.health.as_deref(), Some("Direct over hotspot"));
    assert_eq!(v.health, verdict);
    let plain = view(&paired(ConnectionState::Connected));
    assert!(plain.health.is_none());
}

#[test]
fn the_displayed_lines_are_the_core_lines() {
    // Connected, with the core's documented peer suffix.
    let connected = ViewInput {
        paired_name: Some(NAME.to_string()),
        ..ViewInput::new(ConnectionState::Connected)
    };
    let core = Snapshot {
        state: ConnectionState::Connected,
        ..Snapshot::default()
    };
    assert_eq!(
        view(&connected).status,
        core.status_line_with_peer(Some(NAME))
    );

    // Retry with a real reason and countdown.
    let reason = "Wi-Fi link lost".to_string();
    let retry = ViewInput {
        paired_name: Some(NAME.to_string()),
        retry_secs_left: Some(7),
        retry_reason: Some(reason.clone()),
        ..ViewInput::new(ConnectionState::Retry)
    };
    let core = Snapshot {
        state: ConnectionState::Retry,
        retry: Some(RetryView {
            remaining_secs: 7,
            reason: Some(reason),
        }),
        ..Snapshot::default()
    };
    assert_eq!(view(&retry).status, core.status_line());

    // An expired code is the core's own line.
    let expired = ViewInput {
        code_expired: true,
        ..paired(ConnectionState::Searching)
    };
    let core = CodeView {
        digits: String::new(),
        expires_in_secs: None,
        expired: true,
    };
    assert_eq!(view(&expired).status, core.expiry_line());
}

#[test]
fn the_health_verdicts_are_the_core_lines() {
    for (handshake, path) in [
        (Freshness::Never, classify_path(Freshness::Never, None)),
        (
            Freshness::Age(181),
            classify_path(Freshness::Age(181), None),
        ),
    ] {
        let verdict = Health {
            handshake,
            path,
            old_peer: false,
        }
        .verdict();
        let v = view(&ViewInput {
            health: verdict.clone(),
            ..paired(ConnectionState::Connected)
        });
        assert_eq!(v.health, verdict);
    }
    assert_eq!(
        Health {
            handshake: Freshness::Never,
            path: PeerPath::WaitingForHandshake,
            old_peer: false,
        }
        .verdict()
        .as_deref(),
        Some("Waiting for the first handshake")
    );
    assert_eq!(
        Health {
            handshake: Freshness::Age(181),
            path: PeerPath::Unreachable,
            old_peer: false,
        }
        .verdict()
        .as_deref(),
        Some("Not reachable")
    );
}

#[test]
fn a_lost_connection_counts_down_and_says_the_internet_is_back() {
    let v = view(&ViewInput {
        retry_secs_left: Some(7),
        ..paired(ConnectionState::Retry)
    });
    assert_eq!(v.status, "Connection lost - retrying in 7 s");
    assert_eq!(
        v.extra.as_deref(),
        Some("Your normal internet connection was restored.")
    );
    assert_eq!(v.primary, Primary::Cancel);
    assert!(!v.polkit_note);
    let v = view(&paired(ConnectionState::Retry));
    assert_eq!(v.status, "Connection lost - retrying");
}

#[test]
fn losing_the_network_while_retrying_says_what_to_do() {
    let v = view(&ViewInput {
        retry_reason: Some("no LAN interface found - connect to a network".to_string()),
        ..paired(ConnectionState::Retry)
    });
    assert_eq!(v.status, "No network - turn on the hotspot or join Wi-Fi");
    assert!(v.warning);
}

#[test]
fn a_refused_password_prompt_stops_and_offers_try_again() {
    let v = view(&ViewInput {
        denied: true,
        ..paired(ConnectionState::Idle)
    });
    assert_eq!(v.status, "Permission was denied - try again");
    assert!(v.warning);
    assert_eq!(v.primary, Primary::TryAgain);
    assert!(v.primary_enabled);
    assert!(!v.polkit_note);
    assert!(!v.show_code_entry);
}

#[test]
fn a_denied_teardown_keeps_the_reason_visible() {
    let v = view(&ViewInput {
        error: Some("could not remove the tunnel: permission denied".to_string()),
        ..paired(ConnectionState::Idle)
    });
    assert_eq!(v.status, "could not remove the tunnel: permission denied");
    assert!(v.warning);
}

#[test]
fn a_start_error_is_one_sentence_and_the_rest_goes_to_details() {
    let v = view(&ViewInput {
        error: Some("no LAN interface found - connect to a network".to_string()),
        ..paired(ConnectionState::Idle)
    });
    assert_eq!(v.status, "Waiting for phone...");
    assert!(!v.warning);
    let v = view(&ViewInput {
        error: Some("no LAN interface found - connect to a network".to_string()),
        ..ViewInput::new(ConnectionState::Idle)
    });
    assert_eq!(v.status, "No network - turn on the hotspot or join Wi-Fi");
    assert!(v.warning);
    let v = view(&ViewInput {
        error: Some("settings error: boom".to_string()),
        ..paired(ConnectionState::Idle)
    });
    assert_eq!(v.status, "Could not connect - see Details");
    assert!(v.warning);
}

#[test]
fn labels_match_the_buttons() {
    assert_eq!(Primary::Connect.label(), "Connect");
    assert_eq!(Primary::Cancel.label(), "Cancel");
    assert_eq!(Primary::Disconnect.label(), "Disconnect");
    assert_eq!(Primary::TryAgain.label(), "Try again");
}

// ---- CLI lines -------------------------------------------------------------

#[test]
fn cli_device_line_names_the_paired_device_or_says_there_is_none() {
    assert_eq!(device_line(Some(NAME)), "Device: Phone - Icy Parrot");
    assert_eq!(device_line(None), "Device: none (not paired)");
}

#[test]
fn cli_forget_line_names_what_was_forgotten() {
    assert_eq!(
        forgotten_line(Some(NAME)),
        "Device forgotten: Phone - Icy Parrot"
    );
    assert_eq!(forgotten_line(None), "No paired device");
}

#[test]
fn cli_peer_line_is_name_then_pair_id() {
    assert_eq!(peer_line(NAME, "0123abcd"), "Phone - Icy Parrot  0123abcd");
}

#[test]
fn cli_details_line_is_indented_under_the_status() {
    assert_eq!(
        details_line("no data from peer for 35 s"),
        "  details: no data from peer for 35 s"
    );
}

#[test]
fn the_polkit_note_is_the_sentence_the_user_was_promised() {
    assert_eq!(POLKIT_NOTE, "Your password is needed to set up the tunnel.");
}

#[test]
fn the_polkit_note_is_exactly_the_published_input() {
    let mut input = paired(ConnectionState::Idle);
    assert!(!view(&input).polkit_note);
    input.polkit_note = true;
    assert!(view(&input).polkit_note, "the flag, not the state, decides");

    let mut connected = ViewInput::new(ConnectionState::Connected);
    assert!(!view(&connected).polkit_note);
    connected.polkit_note = true;
    assert!(view(&connected).polkit_note);

    let mut retry = ViewInput::new(ConnectionState::Retry);
    assert!(!view(&retry).polkit_note);
    retry.polkit_note = true;
    assert!(view(&retry).polkit_note);

    let mut denied = paired(ConnectionState::Idle);
    denied.denied = true;
    assert!(!view(&denied).polkit_note);
    denied.polkit_note = true;
    assert!(view(&denied).polkit_note);
}

#[test]
fn details_and_history_share_the_link_counts_line() {
    assert_eq!(link_counts_line(2, 1), "Link losses: 2   Link restores: 1");
    assert_eq!(link_counts_line(0, 0), "Link losses: 0   Link restores: 0");
}

#[test]
fn zero_timestamps_render_none_and_real_ones_use_the_local_zone() {
    assert_eq!(format_timestamp_or_none(0), "none");
    assert_eq!(format_timestamp_or_none(-1), "none");
    assert_eq!(
        format_timestamp_or_none(1_700_000_000_000),
        jafnify_linux::applog::format_local_millis(1_700_000_000_000)
    );
}

#[test]
fn the_kill_switch_says_the_internet_is_blocked_while_the_tunnel_is_down() {
    let mut input = ViewInput::new(ConnectionState::Retry);
    assert!(view(&input).kill_switch_note.is_none());
    input.kill_switch = true;
    assert_eq!(
        view(&input).kill_switch_note.as_deref(),
        Some("Kill switch on: the internet is blocked while the tunnel is down.")
    );
}

#[test]
fn a_search_waiting_for_the_network_says_what_to_do() {
    // After a loss with no network the search waits quietly instead of
    // cycling through Retry; the line keeps the Retry wording.
    let v = view(&ViewInput {
        waiting_for_phone: true,
        no_network: true,
        ..paired(ConnectionState::Searching)
    });
    assert_eq!(v.status, "No network - turn on the hotspot or join Wi-Fi");
    assert!(v.warning);
}
