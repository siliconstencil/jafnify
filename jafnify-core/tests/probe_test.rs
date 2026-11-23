//! The discovery probe: everything
//! up to tunnel-up (0x01 discovery AND the 0x02 key exchange), nothing after.
//! No WireGuard config, no tunnel, and the WireGuard port 9797 is never bound,
//! so it can run while the legacy WgServer and wg0 stay up. Two devices run it
//! with the same code; each reports separately what it hears, which tells an
//! unreachable sender from a blocked receiver.

use std::net::Ipv4Addr;
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use jafnify_core::pairing::PairingCode;
use jafnify_core::probe::{
    local_iface_for, parse_local_ifaces, run_probe_at, LocalIface, ProbeConfig, ProbeReport,
    SeenKind,
};
use jafnify_core::state::Role;

static MULTICAST: Mutex<()> = Mutex::new(());

fn serial() -> MutexGuard<'static, ()> {
    MULTICAST.lock().unwrap_or_else(|e| e.into_inner())
}

fn loopback_iface() -> LocalIface {
    LocalIface {
        name: "lo".to_string(),
        ip: Ipv4Addr::LOCALHOST,
        prefix: 8,
    }
}

/// The laptop side of a probe pair: the joiner.
fn cfg(code: &str, window: Duration) -> ProbeConfig {
    probe_cfg(code, window, Role::Joiner)
}

/// The phone side of a probe pair: the sharer. SPAKE2 needs opposite roles.
fn sharer_cfg(code: &str, window: Duration) -> ProbeConfig {
    probe_cfg(code, window, Role::Sharer)
}

fn probe_cfg(code: &str, window: Duration, role: Role) -> ProbeConfig {
    ProbeConfig {
        code: PairingCode::parse(code).unwrap(),
        role,
        ifaces: vec![loopback_iface()],
        window,
        linger: Duration::from_millis(1500),
        unicast: Vec::new(),
    }
}

/// A loopback multicast port that is not the product's 9799, so a running
/// Jafnify on this host cannot take the test's unicast frames.
fn test_addr(port: u16) -> std::net::SocketAddr {
    format!("239.255.42.99:{port}").parse().unwrap()
}

#[test]
fn two_probes_with_the_same_code_complete_the_key_exchange() {
    let _g = serial();
    let a = std::thread::spawn(|| {
        run_probe_at(
            &cfg("4321", Duration::from_secs(8)),
            test_addr(49920),
            |_| {},
        )
    });
    let b = std::thread::spawn(|| {
        run_probe_at(
            &sharer_cfg("4321", Duration::from_secs(8)),
            test_addr(49920),
            |_| {},
        )
    });
    let (a, b) = (a.join().unwrap().unwrap(), b.join().unwrap().unwrap());
    for r in [&a, &b] {
        assert!(r.discovery_done(), "no 0x01 from the other probe");
        assert!(r.key_exchange_done(), "no 0x06 from the other probe");
        assert_eq!(r.echo, Some(true), "our own frames never came back");
        let kinds: Vec<_> = r.sightings.iter().map(|s| (s.kind, s.verified)).collect();
        assert!(kinds.contains(&(SeenKind::Discovery, true)));
        assert!(kinds.contains(&(SeenKind::KeyExchange, true)));
        assert!(r.sightings.iter().all(|s| s.from == Ipv4Addr::LOCALHOST));
    }
}

#[test]
fn it_ends_soon_after_the_key_exchange_not_at_the_end_of_the_window() {
    let _g = serial();
    let t0 = Instant::now();
    let a = std::thread::spawn(|| {
        run_probe_at(
            &cfg("4321", Duration::from_secs(30)),
            test_addr(49921),
            |_| {},
        )
    });
    let b = std::thread::spawn(|| {
        run_probe_at(
            &sharer_cfg("4321", Duration::from_secs(30)),
            test_addr(49921),
            |_| {},
        )
    });
    a.join().unwrap().unwrap();
    b.join().unwrap().unwrap();
    assert!(t0.elapsed() < Duration::from_secs(10), "{:?}", t0.elapsed());
}

#[test]
fn a_lone_probe_hears_only_itself_and_does_not_complete() {
    let _g = serial();
    let r = run_probe_at(
        &cfg("4321", Duration::from_secs(3)),
        test_addr(49922),
        |_| {},
    )
    .unwrap();
    assert!(!r.discovery_done());
    assert!(!r.key_exchange_done());
    assert!(
        r.sightings.is_empty(),
        "our own frames are not a peer: {:?}",
        r.sightings
    );
    assert_eq!(r.echo, Some(true));
}

#[test]
fn a_probe_with_another_code_sees_the_frames_but_does_not_complete() {
    let _g = serial();
    let a = std::thread::spawn(|| {
        run_probe_at(
            &cfg("4321", Duration::from_secs(3)),
            test_addr(49923),
            |_| {},
        )
    });
    let b = std::thread::spawn(|| {
        run_probe_at(
            &sharer_cfg("1234", Duration::from_secs(3)),
            test_addr(49923),
            |_| {},
        )
    });
    let (a, b) = (a.join().unwrap().unwrap(), b.join().unwrap().unwrap());
    for r in [&a, &b] {
        // The v2 presence tag is public: both probes hear each other.
        assert!(r.discovery_done());
        // The PAKE exchange is code-keyed: a wrong code never completes it,
        // and a frame that does not verify is not reported as a key exchange.
        assert!(!r.key_exchange_done());
    }
}

#[test]
fn every_first_sighting_is_reported_as_it_happens() {
    let _g = serial();
    let seen = Mutex::new(Vec::new());
    let a = std::thread::spawn(|| {
        run_probe_at(
            &cfg("4321", Duration::from_secs(8)),
            test_addr(49924),
            |_| {},
        )
    });
    let r = run_probe_at(
        &sharer_cfg("4321", Duration::from_secs(8)),
        test_addr(49924),
        |s| seen.lock().unwrap().push((s.kind, s.from, s.verified)),
    )
    .unwrap();
    a.join().unwrap().unwrap();
    let seen = seen.into_inner().unwrap();
    assert_eq!(
        seen.len(),
        r.sightings.len(),
        "once per (kind, source, verified)"
    );
    assert!(seen.contains(&(SeenKind::KeyExchange, Ipv4Addr::LOCALHOST, true)));
}

// ---- the report ---------------------------------------------------------------

#[test]
fn the_report_starts_with_the_interfaces_it_ran_on() {
    // On a phone this is the one question the report must answer first:
    // was the hotspot interface in the list?
    let _g = serial();
    let r = run_probe_at(
        &cfg("4321", Duration::from_secs(1)),
        test_addr(49925),
        |_| {},
    )
    .unwrap();
    assert_eq!(r.lines()[0], "probe: interfaces: lo=127.0.0.1/8");
}

#[test]
fn the_report_lines_name_the_source_and_the_interface() {
    let _g = serial();
    let a = std::thread::spawn(|| {
        run_probe_at(
            &cfg("4321", Duration::from_secs(8)),
            test_addr(49926),
            |_| {},
        )
    });
    let r = run_probe_at(
        &sharer_cfg("4321", Duration::from_secs(8)),
        test_addr(49926),
        |_| {},
    )
    .unwrap();
    a.join().unwrap().unwrap();
    let text = r.lines().join("\n");
    assert!(
        text.contains("probe: seen 0x01 from 127.0.0.1 via lo (multicast) x"),
        "{text}"
    );
    assert!(
        text.contains("probe: seen 0x06 from 127.0.0.1 via lo (multicast) x"),
        "{text}"
    );
    assert!(
        text.contains("probe: discovery: ok (peer 127.0.0.1, multicast)"),
        "{text}"
    );
    assert!(
        text.contains("probe: key exchange: ok (peer 127.0.0.1, multicast)"),
        "{text}"
    );
    assert!(text.contains("probe: own echo: yes"), "{text}");
}

#[test]
fn a_failed_probe_says_what_did_not_happen() {
    let _g = serial();
    let r = run_probe_at(
        &cfg("4321", Duration::from_secs(2)),
        test_addr(49927),
        |_| {},
    )
    .unwrap();
    let text = r.lines().join("\n");
    assert!(text.contains("probe: discovery: no peer heard"), "{text}");
    assert!(
        text.contains("probe: key exchange: not completed"),
        "{text}"
    );
}

#[test]
fn the_report_never_contains_a_key_or_the_code() {
    let _g = serial();
    let a = std::thread::spawn(|| {
        run_probe_at(
            &cfg("4321", Duration::from_secs(8)),
            test_addr(49928),
            |_| {},
        )
    });
    let r = run_probe_at(
        &sharer_cfg("4321", Duration::from_secs(8)),
        test_addr(49928),
        |_| {},
    )
    .unwrap();
    a.join().unwrap().unwrap();
    let text = r.lines().join("\n");
    assert!(!text.contains("4321"));
    assert!(!regex_like_key(&text), "{text}");
}

/// Any run of 40+ base64 characters would be key-shaped.
fn regex_like_key(s: &str) -> bool {
    s.split(|c: char| !(c.is_ascii_alphanumeric() || c == '+' || c == '/' || c == '='))
        .any(|w| w.len() >= 40)
}

// ---- which interface a source is on --------------------------------------------

#[test]
fn local_ifaces_parse_from_name_ip_prefix() {
    assert_eq!(
        parse_local_ifaces(
            "swlan0=10.20.89.13/24, wlan0=192.168.1.5/16,junk,x=1.2.3.4,y=1.2.3.4/99"
        ),
        vec![
            LocalIface {
                name: "swlan0".into(),
                ip: "10.20.89.13".parse().unwrap(),
                prefix: 24
            },
            LocalIface {
                name: "wlan0".into(),
                ip: "192.168.1.5".parse().unwrap(),
                prefix: 16
            },
        ]
    );
    assert!(parse_local_ifaces("").is_empty());
}

#[test]
fn a_source_belongs_to_the_most_specific_interface_whose_network_holds_it() {
    let ifaces =
        parse_local_ifaces("wlan0=10.20.0.5/16,swlan0=10.20.89.13/24,rmnet0=100.64.14.9/30");
    assert_eq!(
        local_iface_for(&ifaces, "10.20.89.70".parse().unwrap()),
        Some("swlan0")
    );
    assert_eq!(
        local_iface_for(&ifaces, "10.20.7.7".parse().unwrap()),
        Some("wlan0")
    );
    assert_eq!(local_iface_for(&ifaces, "8.8.8.8".parse().unwrap()), None);
}

// ---- start order loses the phone's PAKE Confirm ----------------------------

/// Both probe roles against each other on loopback, with either side started
/// first by 0 to 1500 ms. Before the late Confirm resend
/// (`PakeLink::resend_confirm_after_accept`) every non-zero delay lost the
/// key exchange on the side that started second: its first Confirm arrived
/// before the first side had a peer and was dropped, and once it had
/// accepted the other Confirm it ignored the repeated Hellos. The namespace
/// lab hit this in 3 of 5 runs (tools/lab/e2e-hotspot.sh).
#[test]
fn both_probes_complete_whichever_side_starts_first() {
    let _g = serial();
    let orders = [
        ("phone first", true, 0u64),
        ("phone first", true, 100),
        ("phone first", true, 500),
        ("phone first", true, 1500),
        ("laptop first", false, 0),
        ("laptop first", false, 100),
        ("laptop first", false, 500),
        ("laptop first", false, 1500),
    ];
    let mut failures = Vec::new();
    for (i, (label, phone_first, delay_ms)) in orders.iter().enumerate() {
        let addr = format!("239.255.42.99:{}", 49900 + i as u16)
            .parse()
            .unwrap();
        let (phone, laptop) = pair_at(addr, *phone_first, Duration::from_millis(*delay_ms));
        let phone_ok = phone.key_exchange_done();
        let laptop_ok = laptop.key_exchange_done();
        println!(
            "start order {label} +{delay_ms}ms: phone={} laptop={}",
            if phone_ok { "ok" } else { "FAIL" },
            if laptop_ok { "ok" } else { "FAIL" }
        );
        if !phone_ok || !laptop_ok {
            failures.push(format!("{label} +{delay_ms}ms"));
        }
    }
    assert!(
        failures.is_empty(),
        "orders that lost the key exchange: {failures:?}"
    );
}

fn pair_at(
    addr: std::net::SocketAddr,
    phone_first: bool,
    delay: Duration,
) -> (ProbeReport, ProbeReport) {
    let phone_cfg = probe_cfg("4321", Duration::from_secs(5), Role::Sharer);
    let laptop_cfg = probe_cfg("4321", Duration::from_secs(5), Role::Joiner);
    let (phone, laptop) = if phone_first {
        let phone = std::thread::spawn(move || run_probe_at(&phone_cfg, addr, |_| {}));
        std::thread::sleep(delay);
        let laptop = std::thread::spawn(move || run_probe_at(&laptop_cfg, addr, |_| {}));
        (phone, laptop)
    } else {
        let laptop = std::thread::spawn(move || run_probe_at(&laptop_cfg, addr, |_| {}));
        std::thread::sleep(delay);
        let phone = std::thread::spawn(move || run_probe_at(&phone_cfg, addr, |_| {}));
        (phone, laptop)
    };
    (
        phone.join().unwrap().unwrap(),
        laptop.join().unwrap().unwrap(),
    )
}
