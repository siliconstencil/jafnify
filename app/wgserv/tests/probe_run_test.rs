//! The phone-side discovery probe:
//! the same probe as `jafnify probe`, started on a background thread so the
//! app can show peers as they are seen. It never binds the WireGuard port.

use std::net::{Ipv4Addr, UdpSocket};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use jafnify_core::pairing::PairingCode;
use jafnify_core::probe::{parse_local_ifaces, run_probe, LocalIface, ProbeConfig};
use jafnify_core::state::Role;
use wgserv::probe::{parse_iface_arg, ProbeRun};

static MULTICAST: Mutex<()> = Mutex::new(());

fn serial() -> MutexGuard<'static, ()> {
    MULTICAST.lock().unwrap_or_else(|e| e.into_inner())
}

const LO: &str = "lo=127.0.0.1/8";

fn wait_done(run: &ProbeRun) -> String {
    let end = Instant::now() + Duration::from_secs(15);
    while run.running() {
        assert!(
            Instant::now() < end,
            "probe never finished: {}",
            run.status()
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    run.status()
}

fn laptop(code: &str) -> std::thread::JoinHandle<()> {
    let code = PairingCode::parse(code).unwrap();
    std::thread::spawn(move || {
        run_probe(
            &ProbeConfig {
                code,
                role: Role::Joiner,
                ifaces: vec![LocalIface {
                    name: "lo".into(),
                    ip: Ipv4Addr::LOCALHOST,
                    prefix: 8,
                }],
                window: Duration::from_secs(10),
                linger: Duration::from_millis(1500),
                unicast: Vec::new(),
            },
            |_| {},
        )
        .unwrap();
    })
}

#[test]
fn start_returns_a_code_and_status_shows_the_peer_and_the_result() {
    let _g = serial();
    let run = ProbeRun::new();
    let code = run.start(parse_local_ifaces(LO), 10).unwrap();
    assert!(PairingCode::parse(&code).is_some(), "{code}");
    assert!(run.running());
    let peer = laptop(&code);
    let status = wait_done(&run);
    peer.join().unwrap();
    assert!(
        status.contains("probe: seen 0x06 from 127.0.0.1 via lo"),
        "{status}"
    );
    assert!(status.contains("probe: key exchange: ok"), "{status}");
    assert!(status.ends_with("probe: result: ok"), "{status}");
}

#[test]
fn a_caller_chosen_code_is_used() {
    // The host binary and the lab run both sides with one agreed code.
    let _g = serial();
    let run = ProbeRun::new();
    run.start_with_code("4321", parse_local_ifaces(LO), 10)
        .unwrap();
    let peer = laptop("4321");
    let status = wait_done(&run);
    peer.join().unwrap();
    assert!(status.ends_with("probe: result: ok"), "{status}");
    assert_eq!(
        run.start_with_code("12ab", parse_local_ifaces(LO), 1),
        Err("Invalid code".to_string())
    );
}

#[test]
fn a_probe_nobody_answers_ends_failed() {
    let _g = serial();
    let run = ProbeRun::new();
    run.start(parse_local_ifaces(LO), 2).unwrap();
    let status = wait_done(&run);
    assert!(status.ends_with("probe: result: failed"), "{status}");
}

#[test]
fn it_runs_while_something_else_holds_the_wireguard_port() {
    let _g = serial();
    // The legacy WgServer stand-in.
    let held = match UdpSocket::bind("0.0.0.0:9797") {
        Ok(s) => s,
        Err(e) => panic!("this test needs UDP 9797 free: {e}"),
    };
    let run = ProbeRun::new();
    let code = run.start(parse_local_ifaces(LO), 10).unwrap();
    let peer = laptop(&code);
    let status = wait_done(&run);
    peer.join().unwrap();
    drop(held);
    assert!(status.ends_with("probe: result: ok"), "{status}");
}

#[test]
fn a_second_start_while_running_is_refused() {
    let _g = serial();
    let run = ProbeRun::new();
    run.start(parse_local_ifaces(LO), 2).unwrap();
    assert_eq!(
        run.start(parse_local_ifaces(LO), 2),
        Err("Already running".to_string())
    );
    wait_done(&run);
    // Finished: it may start again.
    run.start(parse_local_ifaces(LO), 1).unwrap();
    wait_done(&run);
}

#[test]
fn no_interface_is_an_error_not_a_silent_default_route() {
    let run = ProbeRun::new();
    assert!(run
        .start(Vec::new(), 2)
        .unwrap_err()
        .contains("no interface"));
    assert!(!run.running());
}

#[test]
fn status_before_any_start_is_empty() {
    assert_eq!(ProbeRun::new().status(), "");
}

#[test]
fn the_host_flag_takes_an_address_with_an_optional_prefix() {
    let a = parse_iface_arg("192.168.77.1/24").unwrap();
    assert_eq!((a.ip, a.prefix), ("192.168.77.1".parse().unwrap(), 24));
    let b = parse_iface_arg("10.77.0.1").unwrap();
    assert_eq!(
        (b.ip, b.prefix, b.name.as_str()),
        ("10.77.0.1".parse().unwrap(), 24, "10.77.0.1")
    );
    assert!(parse_iface_arg("nope").is_none());
    assert!(parse_iface_arg("10.0.0.1/40").is_none());
}
