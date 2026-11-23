//! What may leave the device is decided in one place, so the phone, the
//! laptop and every future port redact the same way.

use jafnify_core::redact::{
    classify_addr, redact_export, redact_export_with, AddrScope, Redactor, CODE, DEVICE, HOME,
    HOST, MAC, PUBLIC_IP, REDACTED, SSID,
};

const B64_KEY: &str = "yAnz5TF+lXXJte14tji3zlMNq+hd2rYUIgJBgB3fBmk=";
const HEX_KEY: &str = "e8f4c2b19a7d3e6f0c5b8a2d9e4f1c7a3b6d0e2f5c8a1b4d7e0f3a6c9b2d5e8f1";
const UAPI_DUMP: &str = "private_key=e8f4c2b19a7d3e6f0c5b8a2d9e4f1c7a3b6d0e2f5c8a1b4d7e0f3a6c9b2d5e8f1\nlisten_port=51820\npublic_key=yAnz5TF+lXXJte14tji3zlMNq+hd2rYUIgJBgB3fBmk=\n";

#[test]
fn a_public_address_becomes_a_placeholder() {
    assert_eq!(classify_addr("203.0.113.5"), Some(AddrScope::Public));
    assert_eq!(classify_addr("8.8.8.8"), Some(AddrScope::Public));
    assert_eq!(classify_addr("2001:db8::1"), Some(AddrScope::Public));
}

#[test]
fn a_lan_address_stays() {
    assert_eq!(classify_addr("192.168.1.20"), Some(AddrScope::Private));
    assert_eq!(classify_addr("10.0.0.2"), Some(AddrScope::Private));
    assert_eq!(classify_addr("172.16.0.1"), Some(AddrScope::Private));
    assert_eq!(classify_addr("fd00::1"), Some(AddrScope::Private));
}

#[test]
fn a_public_address_in_a_record_value_becomes_a_placeholder() {
    let out = redact_export("ts=2026-10-31T12:34:56.789Z kind=STATS peer=203.0.113.5:9797");
    assert!(out.contains(&format!("peer={PUBLIC_IP}:9797")), "{out}");
}

#[test]
fn a_private_address_stays_byte_for_byte() {
    let text = "ts=2026-10-31T12:34:56.789Z kind=STATS peer=192.168.1.20:9797 local=10.0.0.2";
    let out = redact_export(text);
    assert!(out.contains("peer=192.168.1.20:9797"), "{out}");
    assert!(out.contains("local=10.0.0.2"), "{out}");
}

#[test]
fn the_loopback_the_link_local_and_the_unspecified_address_stay() {
    for addr in [
        "127.0.0.1",
        "169.254.10.1",
        "0.0.0.0",
        "100.64.0.1",
        "224.0.0.1",
        "::1",
        "fe80::1",
    ] {
        let text = format!("ts=2026-10-31T12:34:56.789Z kind=STATS peer={addr}:9797");
        let out = redact_export(&text);
        assert!(out.contains(addr), "{addr} was changed: {out}");
    }
}

#[test]
fn a_public_ipv6_address_becomes_a_placeholder() {
    let out = redact_export("ts=2026-10-31T12:34:56.789Z kind=STATS peer=2001:db8::1:9797");
    assert!(out.contains(&format!("peer={PUBLIC_IP}:9797")), "{out}");
}

#[test]
fn an_ipv4_mapped_ipv6_address_is_public_when_its_ipv4_is() {
    assert_eq!(classify_addr("::ffff:203.0.113.5"), Some(AddrScope::Public));
    assert_eq!(
        classify_addr("::ffff:192.168.1.20"),
        Some(AddrScope::Private)
    );
}

#[test]
fn a_base64_key_becomes_a_placeholder() {
    let out = redact_export(&format!(
        "ts=2026-10-31T12:34:56.789Z kind=EVENT key={B64_KEY}"
    ));
    assert!(!out.contains(B64_KEY), "{out}");
    assert!(out.contains(REDACTED), "{out}");
}

#[test]
fn a_hex_key_becomes_a_placeholder() {
    let out = redact_export(&format!(
        "ts=2026-10-31T12:34:56.789Z kind=EVENT key={HEX_KEY}"
    ));
    assert!(!out.contains(HEX_KEY), "{out}");
    assert!(out.contains(REDACTED), "{out}");
}

#[test]
fn a_key_named_field_is_redacted_whatever_the_encoding() {
    for field in ["private_key", "preshared_key", "public_key", "psk", "key"] {
        let out = redact_export(&format!("{field}={B64_KEY}"));
        assert!(!out.contains(B64_KEY), "{field}: {out}");
        assert!(out.contains(REDACTED), "{field}: {out}");
    }
}

#[test]
fn a_wireguard_uapi_dump_loses_its_keys() {
    let out = redact_export(UAPI_DUMP);
    assert!(!out.contains(HEX_KEY), "{out}");
    assert!(!out.contains(B64_KEY), "{out}");
    assert!(out.contains(REDACTED), "{out}");
    assert!(out.contains("listen_port=51820"), "{out}");
}

#[test]
fn a_peer_prefix_in_a_peer_field_becomes_a_peer_label() {
    let out = redact_export("ts=2026-10-31T12:34:56.789Z kind=EVENT peer_key_prefix=AbCdEfGhIjKl");
    assert!(!out.contains("AbCdEfGhIjKl"), "{out}");
    assert!(out.contains("peer_key_prefix=peer-1"), "{out}");
}

#[test]
fn the_same_prefix_in_another_field_stays() {
    let out = redact_export("ts=2026-10-31T12:34:56.789Z kind=EVENT note=AbCdEfGhIjKl");
    assert!(out.contains("note=AbCdEfGhIjKl"), "{out}");
}

#[test]
fn a_session_id_is_not_a_peer_prefix() {
    let out = redact_export("ts=2026-10-31T12:34:56.789Z kind=EVENT id=19f2a3b4c5d6-01");
    assert!(out.contains("id=19f2a3b4c5d6-01"), "{out}");
}

#[test]
fn a_prose_line_keeps_its_punctuation() {
    let out = redact_export("2026-10-31 12:34:56 reconnected (peer 203.0.113.5:9797)");
    assert!(out.contains(&format!("(peer {PUBLIC_IP}:9797)")), "{out}");
}

#[test]
fn every_record_shape_is_handled() {
    let shapes = [
        "ts=2026-10-31T12:34:56.789Z kind=STATS peer=203.0.113.5:9797",
        "STATS peer=203.0.113.5:9797",
        "EVENT ts=1761912345678 event=peer_endpoint peer=203.0.113.5:9797",
    ];
    for shape in shapes {
        let out = redact_export(shape);
        assert!(!out.contains("203.0.113.5"), "{shape} -> {out}");
    }
}

#[test]
fn a_field_nobody_thought_of_is_still_safe() {
    let out = redact_export("ts=2026-10-31T12:34:56.789Z kind=STATS new_field=203.0.113.5");
    assert!(!out.contains("203.0.113.5"), "{out}");
}

#[test]
fn the_counters_the_timestamps_and_the_interface_names_stay() {
    let text = "ts=2026-10-31T12:34:56.789Z kind=STATS control_rx_packets=7 wg_handshake_ms=-1 egress_iface=wlan0 state=Connected";
    let out = redact_export(text);
    assert!(out.contains("control_rx_packets=7"), "{out}");
    assert!(out.contains("wg_handshake_ms=-1"), "{out}");
    assert!(out.contains("egress_iface=wlan0"), "{out}");
    assert!(out.contains("state=Connected"), "{out}");
    assert!(out.contains("ts=2026-10-31T12:34:56.789Z"), "{out}");
}

#[test]
fn redacting_twice_changes_nothing() {
    let text = format!(
        "ts=2026-10-31T12:34:56.789Z kind=STATS peer=203.0.113.5:9797 key={B64_KEY} mac=aa:bb:cc:dd:ee:ff"
    );
    let once = redact_export(&text);
    let twice = redact_export(&once);
    assert_eq!(once, twice);
}

#[test]
fn a_wireless_name_becomes_a_placeholder() {
    let out = redact_export("ts=2026-10-31T12:34:56.789Z kind=EVENT ssid=MyHomeNetwork");
    assert!(!out.contains("MyHomeNetwork"), "{out}");
    assert!(out.contains(&format!("ssid={SSID}")), "{out}");
}

#[test]
fn a_hardware_address_becomes_a_placeholder() {
    let out = redact_export("ts=2026-10-31T12:34:56.789Z kind=EVENT mac=aa:bb:cc:dd:ee:ff");
    assert!(!out.contains("aa:bb:cc:dd:ee:ff"), "{out}");
    assert!(out.contains(&format!("mac={MAC}")), "{out}");
}

#[test]
fn the_home_directory_becomes_a_tilde() {
    let out = redact_export("2026-10-31 12:34:56 wrote /home/user/.config/jafnify/state.toml");
    assert_eq!(
        out,
        format!("2026-10-31 12:34:56 wrote {HOME}/.config/jafnify/state.toml")
    );
}

#[test]
fn the_app_data_directory_becomes_a_tilde() {
    let out = redact_export("2026-10-31 12:34:56 read /data/user/0/com.siliconstencil.jafnify/diag");
    assert_eq!(
        out,
        format!("2026-10-31 12:34:56 read {HOME}/com.siliconstencil.jafnify/diag")
    );
}

#[test]
fn a_home_directory_followed_by_a_public_address_redacts_both() {
    assert_eq!(redact_export("/home/user/203.0.113.5"), "~/<public-ip>");
}

#[test]
fn a_home_directory_followed_by_an_ordinary_path_is_unchanged() {
    assert_eq!(
        redact_export("/home/user/.config/jafnify/state.toml"),
        "~/.config/jafnify/state.toml"
    );
}

#[test]
fn the_app_data_directory_followed_by_a_public_address_redacts_both() {
    assert_eq!(
        redact_export("/data/user/0/com.x/files/203.0.113.5"),
        "~/com.x/files/<public-ip>"
    );
}

#[test]
fn both_home_shapes_are_idempotent() {
    for text in [
        "/home/user/203.0.113.5",
        "/data/user/0/com.x/files/203.0.113.5",
    ] {
        let once = redact_export(text);
        assert_eq!(once, redact_export(&once), "{text}");
    }
}

#[test]
fn aliases_are_labelled_by_registration_not_first_appearance() {
    let peers: &[&[&str]] = &[&["AbCdEfGhIjKl"], &["ZyXwVuTsRqPo"]];
    assert_eq!(
        redact_export_with("ZyXwVuTsRqPo AbCdEfGhIjKl", peers, ""),
        "peer-2 peer-1"
    );
    assert_eq!(
        redact_export_with("AbCdEfGhIjKl ZyXwVuTsRqPo", peers, ""),
        "peer-1 peer-2"
    );
}

#[test]
fn the_same_alias_list_gives_the_same_label_in_two_calls() {
    let peers: &[&[&str]] = &[
        &["Alice's Phone", "AbCdEfGhIjKl"],
        &["Bob's Phone", "ZyXwVuTsRqPo"],
    ];
    let first = redact_export_with("AbCdEfGhIjKl", peers, "");
    let second = redact_export_with("AbCdEfGhIjKl", peers, "");
    assert_eq!(first, "peer-1");
    assert_eq!(second, "peer-1");
}

#[test]
fn the_machine_name_becomes_a_placeholder() {
    let mut r = Redactor::new();
    r.set_hostname("example-laptop");
    let out = r.redact("2026-10-31 12:34:56 host example-laptop reported a problem");
    assert!(!out.contains("example-laptop"), "{out}");
    assert!(out.contains(&format!("host {HOST}")), "{out}");
}

#[test]
fn the_pairing_code_becomes_a_placeholder() {
    let out = redact_export("2026-10-31 12:34:56 pairing code 4173 issued");
    assert!(!out.contains("4173"), "{out}");
    assert!(out.contains(&format!("code {CODE}")), "{out}");
}

#[test]
fn a_pairing_code_field_becomes_a_placeholder() {
    let out = redact_export("ts=2026-10-31T12:34:56.789Z kind=EVENT code=4173");
    assert!(!out.contains("4173"), "{out}");
}

#[test]
fn one_peer_gets_one_label_whatever_it_is_called() {
    let mut r = Redactor::new();
    r.note_peer(&["Alice's Phone", "AbCdEfGhIjKl", "pair-9f2a"]);
    let out =
        r.redact("2026-10-31 12:34:56 peer AbCdEfGhIjKl (Alice's Phone) pair-9f2a reconnected");
    assert!(out.contains("peer-1"), "{out}");
    assert!(!out.contains("AbCdEfGhIjKl"), "{out}");
    assert!(!out.contains("Alice's Phone"), "{out}");
    assert!(!out.contains("pair-9f2a"), "{out}");
}

#[test]
fn two_peers_get_two_labels() {
    let mut r = Redactor::new();
    r.note_peer(&["Alice's Phone", "AbCdEfGhIjKl"]);
    r.note_peer(&["Bob's Laptop", "ZyXwVuTsRqPo"]);
    let out = r.redact("2026-10-31 12:34:56 AbCdEfGhIjKl and ZyXwVuTsRqPo paired");
    assert!(out.contains("peer-1"), "{out}");
    assert!(out.contains("peer-2"), "{out}");
    assert!(!out.contains("AbCdEfGhIjKl"), "{out}");
    assert!(!out.contains("ZyXwVuTsRqPo"), "{out}");
}

#[test]
fn a_fixture_with_every_secret_shape_keeps_none_of_them() {
    let fixture = format!(
        "ts=2026-10-31T12:34:56.789Z kind=STATS peer=203.0.113.5:9797 local=192.168.1.20 egress_src=203.0.113.9\n\
         ts=2026-10-31T12:34:57.789Z kind=EVENT key={B64_KEY}\n\
         ts=2026-10-31T12:34:58.789Z kind=EVENT private_key={HEX_KEY}\n\
         {UAPI_DUMP}\
         2026-10-31 12:34:59 mac=aa:bb:cc:dd:ee:ff host=example-laptop\n\
         2026-10-31 12:35:00 /home/user/.config/jafnify/state.toml /data/user/0/com.siliconstencil.jafnify/diag\n\
         2026-10-31 12:35:01 pairing code 4173 peer=203.0.113.5:9797 peer_name=Alice's Phone\n"
    );
    let out = redact_export(&fixture);
    for secret in [
        B64_KEY,
        HEX_KEY,
        "203.0.113.5",
        "203.0.113.9",
        "aa:bb:cc:dd:ee:ff",
        "example-laptop",
        "/home/user",
        "/data/user/0",
        "4173",
        "Alice's Phone",
    ] {
        assert!(!out.contains(secret), "{secret} survived:\n{out}");
    }
    // The LAN address stays, because the decision keeps LAN addresses.
    assert!(out.contains("192.168.1.20"), "{out}");
}

#[test]
fn a_fixture_with_every_secret_shape_is_stable_under_a_second_pass() {
    let fixture = format!(
        "ts=2026-10-31T12:34:56.789Z kind=STATS peer=203.0.113.5:9797 key={B64_KEY}\n\
         {UAPI_DUMP}\
         2026-10-31 12:35:00 /home/user/x mac=aa:bb:cc:dd:ee:ff pairing code 4173\n"
    );
    let once = redact_export(&fixture);
    assert_eq!(once, redact_export(&once));
}

#[test]
fn an_alias_that_is_a_key_prefix_does_not_mangle_the_key() {
    // 44 characters: the registered 12-character prefix followed by more
    // base64 and the padding. A substring replacement would leave `peer-1`
    // plus 31 visible key characters.
    let key = format!("AbCdEfGhIjKl{}=", "A".repeat(31));
    assert_eq!(key.len(), 44);
    let peers: &[&[&str]] = &[&["AbCdEfGhIjKl"]];
    let out = redact_export_with(&format!("peer {key}"), peers, "");
    assert_eq!(out, "peer [redacted]");
    assert!(!out.contains("AbCdEfGhIjKl"), "{out}");
}

#[test]
fn a_standalone_key_prefix_is_still_a_peer_label() {
    let peers: &[&[&str]] = &[&["AbCdEfGhIjKl"]];
    assert_eq!(
        redact_export_with("peer AbCdEfGhIjKl", peers, ""),
        "peer peer-1"
    );
}

#[test]
fn an_unaliased_peer_name_with_spaces_leaves_no_word_behind() {
    let out = redact_export("ts=2026-10-31T12:34:56.789Z kind=EVENT peer_name=Alice's Phone");
    assert!(!out.contains("Alice"), "{out}");
    assert!(!out.contains("Phone"), "{out}");
    assert_eq!(
        out,
        "ts=2026-10-31T12:34:56.789Z kind=EVENT peer_name=peer-1"
    );
}

#[test]
fn a_quoted_peer_name_keeps_the_rest_of_the_line() {
    let out = redact_export("peer_name=\"Alice's Phone\" peer=203.0.113.5:9797");
    assert!(!out.contains("Alice"), "{out}");
    assert!(!out.contains("Phone"), "{out}");
    assert!(out.contains("peer_name=peer-1"), "{out}");
    assert!(out.contains(&format!("peer={PUBLIC_IP}:9797")), "{out}");
}

#[test]
fn a_wireless_name_with_spaces_is_consumed_whole() {
    let out = redact_export("ts=1 kind=EVENT ssid=My Home Network");
    assert!(!out.contains("My"), "{out}");
    assert!(!out.contains("Home"), "{out}");
    assert_eq!(out, format!("ts=1 kind=EVENT ssid={SSID}"));
}

#[test]
fn a_device_name_with_spaces_is_consumed_whole() {
    let out = redact_export("ts=1 kind=EVENT device_name=Pixel 8 Pro");
    assert!(!out.contains("Pixel"), "{out}");
    assert!(!out.contains("Pro"), "{out}");
    assert_eq!(out, format!("ts=1 kind=EVENT device_name={DEVICE}"));
}

#[test]
fn a_long_hex_run_and_a_hostname_are_redacted_in_one_pass() {
    let mut r = Redactor::new();
    r.set_hostname("example-laptop");
    let mut line = String::from("example-laptop ");
    line.push_str(&"ab:".repeat(70_000));
    line.push_str(" peer=203.0.113.5:9797");
    assert!(line.len() > 200_000);

    let out = r.redact(&line);

    assert!(out.contains(HOST), "{out}");
    assert!(!out.contains("example-laptop"), "{out}");
    assert!(out.contains(&format!("peer={PUBLIC_IP}:9797")), "{out}");
    assert!(!out.contains("203.0.113.5"), "{out}");
}

#[test]
fn a_very_long_line_does_not_break_the_redactor() {
    let long = "x".repeat(200_000);
    let out = redact_export(&long);
    assert_eq!(out.len(), long.len());
}

#[test]
fn a_colon_separated_hex_key_becomes_a_placeholder() {
    let key = &HEX_KEY[..64];
    let split: Vec<&str> = key
        .as_bytes()
        .chunks(2)
        .map(|c| std::str::from_utf8(c).unwrap())
        .collect();
    let text = format!("key={}", split.join(":"));
    let out = redact_export(&text);
    assert_eq!(out, "key=[redacted]");
    assert_eq!(out, redact_export(&out));
}

#[test]
fn a_space_separated_hex_key_becomes_a_placeholder() {
    let key = &HEX_KEY[..64];
    let split: Vec<&str> = key
        .as_bytes()
        .chunks(8)
        .map(|c| std::str::from_utf8(c).unwrap())
        .collect();
    let text = format!("key={}", split.join(" "));
    let out = redact_export(&text);
    assert_eq!(out, "key=[redacted]");
    assert_eq!(out, redact_export(&out));
}

#[test]
fn a_colon_separated_base64_key_becomes_a_placeholder() {
    let body = B64_KEY.trim_end_matches('=');
    let split: Vec<&str> = body
        .as_bytes()
        .chunks(4)
        .map(|c| std::str::from_utf8(c).unwrap())
        .collect();
    let text = format!("key={}=", split.join(":"));
    let out = redact_export(&text);
    assert_eq!(out, "key=[redacted]");
    assert_eq!(out, redact_export(&out));
}

#[test]
fn the_single_export_entry_point_redacts_the_host_and_the_peers() {
    let peers: &[&[&str]] = &[&["AbCdEfGhIjKl"]];
    let out = redact_export_with("host=my-laptop peer=AbCdEfGhIjKl", peers, "my-laptop");
    assert!(out.contains(HOST), "{out}");
    assert!(out.contains("peer-1"), "{out}");
    assert!(!out.contains("my-laptop"), "{out}");
    assert!(!out.contains("AbCdEfGhIjKl"), "{out}");
}
