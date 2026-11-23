use jafnify_core::config::{ConfigError, ConfigGenerator};

// Test fixtures: placeholder EXAMPLE keys (all-zero/all-B base64), never real WireGuard keys.
const PRIV: &str = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";
const PUB: &str = "BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB=";

// The laptop drops NEW inbound connections on its tunnel interface, installed
// with the tunnel and removed with it.
#[test]
fn the_joiner_config_drops_new_inbound_connections_on_the_tunnel() {
    let cfg = ConfigGenerator::joiner_config(PRIV, PUB, "192.168.43.1:9797");
    let text = ConfigGenerator.to_wg_quick_text(&cfg);
    // The guard is the first command of PostUp; the hooks share one line.
    // Without systemd-resolved the guard is the only command, so the line ends
    // there (the IPv6 guard moved to the root helper).
    let up = "PostUp = iptables -I INPUT 1 -i %i -m conntrack --ctstate NEW -j DROP";
    let down = "PostDown = iptables -D INPUT -i %i -m conntrack --ctstate NEW -j DROP || true";
    let peer_at = text.find("\n[Peer]").expect("no [Peer] section");
    let up_at = text
        .find(up)
        .unwrap_or_else(|| panic!("no PostUp in:\n{text}"));
    assert!(up_at < peer_at, "PostUp must be in [Interface]:\n{text}");
    assert!(text.contains(down), "no PostDown in:\n{text}");
}

#[test]
fn the_joiner_config_round_trips_with_its_hooks() {
    let cfg = ConfigGenerator::joiner_config(PRIV, PUB, "192.168.43.1:9797");
    let text = ConfigGenerator.to_wg_quick_text(&cfg);
    assert_eq!(ConfigGenerator::parse(&text).unwrap(), cfg);
}

#[test]
fn the_sharer_config_has_no_hooks() {
    let cfg = ConfigGenerator::sharer_config(PRIV, PUB);
    let text = ConfigGenerator.to_wg_quick_text(&cfg);
    assert!(
        !text.contains("PostUp") && !text.contains("PostDown"),
        "{text}"
    );
}

#[test]
fn sharer_config_has_spec_invariants() {
    let cfg = ConfigGenerator::sharer_config(PRIV, PUB);
    assert_eq!(cfg.interface.address, "10.9.9.1");
    assert_eq!(cfg.interface.mtu, Some(1280));
    assert_eq!(cfg.peer.allowed_ips, "0.0.0.0/0");
    assert_eq!(cfg.peer.endpoint, None);
}

#[test]
fn joiner_config_has_endpoint() {
    let cfg = ConfigGenerator::joiner_config(PRIV, PUB, "192.168.43.1:9797");
    assert_eq!(cfg.interface.address, "10.9.9.2");
    assert_eq!(cfg.peer.endpoint.as_deref(), Some("192.168.43.1:9797"));
}

#[test]
fn joiner_config_text_has_the_gateway_dns_line() {
    // Without it the PC keeps its old resolver while all traffic is routed
    // into the tunnel; the sharer's router answers DNS at 10.233.0.1.
    let cfg = ConfigGenerator::joiner_config(PRIV, PUB, "192.168.43.1:9797");
    assert_eq!(cfg.interface.dns.as_deref(), Some("10.233.0.1"));
    let text = ConfigGenerator.to_wg_quick_text(&cfg);
    assert!(
        text.contains("\nDNS = 10.233.0.1\n"),
        "no DNS line in:\n{text}"
    );
    let parsed = ConfigGenerator::parse(&text).unwrap();
    assert_eq!(parsed.interface.dns.as_deref(), Some("10.233.0.1"));
}

#[test]
fn sharer_config_text_has_no_dns_line() {
    let cfg = ConfigGenerator::sharer_config(PRIV, PUB);
    assert_eq!(cfg.interface.dns, None);
    assert!(!ConfigGenerator.to_wg_quick_text(&cfg).contains("DNS"));
}

#[test]
fn round_trip_parse_generate_is_identity() {
    let cfg = ConfigGenerator::joiner_config(PRIV, PUB, "192.168.43.1:9797");
    let text = ConfigGenerator.to_wg_quick_text(&cfg);
    let parsed = ConfigGenerator::parse(&text).unwrap();
    assert_eq!(parsed, cfg);
}

#[test]
fn unknown_field_rejected() {
    let text = "[Interface]\naddress = 10.9.9.2\nprivate_key = x\nbogus = 1\n\n[Peer]\npublic_key = y\nallowed_ips = 0.0.0.0/0\n";
    match ConfigGenerator::parse(text) {
        Err(ConfigError::Parse(_)) => {}
        other => panic!("expected Parse error, got {other:?}"),
    }
}

#[test]
fn keepalive_emitted_in_wg_quick_text() {
    // Phone-hotspot NAT drops idle tunnels; 25s keepalive is required
    // and must survive the wg-quick text round trip.
    let cfg = ConfigGenerator::sharer_config(PRIV, PUB);
    assert_eq!(cfg.peer.persistent_keepalive, Some(25));
    let text = ConfigGenerator.to_wg_quick_text(&cfg);
    assert!(
        text.contains("PersistentKeepalive = 25"),
        "expected keepalive in text, got:\n{text}"
    );
}

#[test]
fn parse_rejects_missing_required_field() {
    let text = "[Interface]\nmtu = 1280\n\n[Peer]\nallowed_ips = 0.0.0.0/0\n";
    match ConfigGenerator::parse(text) {
        Err(ConfigError::InvalidField(_)) => {}
        other => panic!("expected InvalidField error, got {other:?}"),
    }
}

#[test]
fn wg_quick_text_uses_wg_key_names() {
    // `wg setconf` rejects snake_case keys ("Line unrecognized"); the text
    // must use the exact wg-quick names.
    let cfg = ConfigGenerator::joiner_config(PRIV, PUB, "192.168.43.1:9797");
    let text = ConfigGenerator.to_wg_quick_text(&cfg);
    for key in [
        "Address = ",
        "PrivateKey = ",
        "MTU = ",
        "PublicKey = ",
        "Endpoint = ",
        "PersistentKeepalive = ",
        "AllowedIPs = ",
    ] {
        assert!(text.contains(key), "missing `{key}` in:\n{text}");
    }
    for line in text.lines().filter(|l| l.contains('=')) {
        let key = line.split('=').next().unwrap();
        assert!(!key.contains('_'), "snake_case key `{key}` in:\n{text}");
    }
}

#[test]
fn parse_accepts_wg_quick_key_names() {
    let text = "[Interface]\nAddress = 10.9.9.2\nPrivateKey = x\nMTU = 1280\nDNS = 1.1.1.1\n\n[Peer]\nPublicKey = y\nEndpoint = 192.168.43.1:9797\nPersistentKeepalive = 25\nAllowedIPs = 0.0.0.0/0\n";
    let cfg = ConfigGenerator::parse(text).expect("wg-quick key names must parse");
    assert_eq!(cfg.interface.private_key, "x");
    assert_eq!(cfg.interface.mtu, Some(1280));
    assert_eq!(cfg.interface.dns.as_deref(), Some("1.1.1.1"));
    assert_eq!(cfg.peer.allowed_ips, "0.0.0.0/0");
    assert_eq!(cfg.peer.persistent_keepalive, Some(25));
}
