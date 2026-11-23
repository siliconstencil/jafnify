//! Host-runnable sharer: the same sharer/gateway loop the Android app runs
//! (discovery + handshake + libwgslirpy gateway on UDP 9797), without JNI or
//! any Android API, so a sharer and a Linux joiner can be tested in two
//! network namespaces.
//!
//!     jafnify-sharer-host --code 4321 [--iface-ip A.B.C.D] [--state-dir DIR]
//!     jafnify-sharer-host --state-dir DIR [--iface-ip A.B.C.D]
//!
//! --iface-ip pins discovery/handshake multicast to the interface owning that
//! address; repeat it to use several interfaces at once (what the phone does
//! with every non-cellular interface). Without it the kernel's default
//! multicast route is used.
//! --state-dir keeps the paired device in DIR/state.toml (0600): a code
//! pairing is stored after its first received data. With a stored peer and no
//! --code the known track runs (no code, 0x03 Hello/Proof); --code always
//! pairs anew and replaces the stored peer.
//! State changes are logged to stderr; stop with Ctrl-C.
//!
//!     jafnify-sharer-host --probe --code 4321 --iface-ip A.B.C.D[/P]... [--seconds N]
//!
//! --probe runs the discovery probe instead: 0x01 discovery and the 0x02 key
//! exchange on those interfaces, a report on stdout, exit code 0 only if the
//! key exchange completed. No tunnel, no WireGuard config, UDP 9797 never bound.

use std::net::Ipv4Addr;
use std::path::PathBuf;
use std::time::Duration;

use jafnify_core::discovery::DiscoveryConfig;
use jafnify_core::pairing::PairingCode;
use jafnify_core::probe::LocalIface;
use wgserv::joiner::Joiner;
use wgserv::probe::{parse_iface_arg, ProbeRun};

const USAGE: &str = "usage: jafnify-sharer-host (--code NNNN | --state-dir DIR) [--code NNNN] [--state-dir DIR] [--iface-ip A.B.C.D[/P]]... [--stats-fake-line LINE] [--stats-fake-exit updated|crash|user|anr|low-memory|killed|other]\n       jafnify-sharer-host --probe --code NNNN --iface-ip A.B.C.D[/P]... [--seconds N]";

#[derive(Debug)]
struct Args {
    code: Option<String>,
    iface_ips: Vec<Ipv4Addr>,
    /// The same interfaces with a prefix, for the probe's labels.
    ifaces: Vec<LocalIface>,
    state_dir: Option<PathBuf>,
    /// Run the discovery probe instead of the sharer.
    probe: bool,
    seconds: u64,
    /// Lab/test only: a fixed summary line the stats responder serves, so the
    /// channel can be proven before the real phone feed exists.
    stats_fake_line: Option<String>,
    /// Lab/test only: the exit word the phone reports for its previous
    /// process. An app update passes `updated`; a crash passes `crash`; an
    /// old app passes nothing at all.
    stats_fake_exit: Option<jafnify_core::phone_exit::ExitReason>,
}

fn parse_args(mut args: impl Iterator<Item = String>) -> Result<Args, String> {
    let mut code = None;
    let mut iface_ips = Vec::new();
    let mut ifaces = Vec::new();
    let mut state_dir = None;
    let mut probe = false;
    let mut seconds = 30;
    let mut stats_fake_line = None;
    let mut stats_fake_exit = None;
    while let Some(flag) = args.next() {
        let mut value = || args.next().ok_or_else(|| format!("{flag} needs a value"));
        match flag.as_str() {
            "--code" => code = Some(value()?),
            "--iface-ip" => {
                let v = value()?;
                let iface = parse_iface_arg(&v).ok_or_else(|| format!("bad --iface-ip: {v}"))?;
                iface_ips.push(iface.ip);
                ifaces.push(iface);
            }
            "--state-dir" => state_dir = Some(PathBuf::from(value()?)),
            "--stats-fake-line" => stats_fake_line = Some(value()?),
            "--stats-fake-exit" => {
                let v = value()?;
                let reason = jafnify_core::phone_exit::ExitReason::parse(&v)
                    .ok_or_else(|| format!("bad --stats-fake-exit: {v}"))?;
                stats_fake_exit = Some(reason);
            }
            "--probe" => probe = true,
            "--seconds" => {
                let v = value()?;
                seconds = v.parse().map_err(|_| format!("bad --seconds: {v}"))?;
            }
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    if probe && (code.is_none() || ifaces.is_empty()) {
        return Err("--probe needs --code and at least one --iface-ip".into());
    }
    // Without a state dir nothing can be remembered, so a code is the only way in.
    if code.is_none() && state_dir.is_none() {
        return Err("--code is required (or --state-dir DIR to use the stored peer)".into());
    }
    if let Some(code) = &code {
        if PairingCode::parse(code).is_none() {
            return Err(format!("bad --code {code}: need 4 digits, first non-zero"));
        }
    }
    Ok(Args {
        code,
        iface_ips,
        ifaces,
        state_dir,
        probe,
        seconds,
        stats_fake_line,
        stats_fake_exit,
    })
}

#[derive(Debug, PartialEq, Eq)]
enum Mode {
    /// A code pairs a new device (replacing any stored one).
    Pair(String),
    /// The stored peer reconnects with no code.
    Known,
}

fn choose_mode(code: Option<&str>, paired: bool) -> Result<Mode, String> {
    match (code, paired) {
        (Some(code), _) => Ok(Mode::Pair(code.to_string())),
        (None, true) => Ok(Mode::Known),
        (None, false) => Err("no paired device - pass --code to pair".into()),
    }
}

fn main() {
    let args = match parse_args(std::env::args().skip(1)) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("error: {e}\n{USAGE}");
            std::process::exit(2);
        }
    };
    // This binary is a host harness; the Android build compiles it too but
    // never ships it, and the Android dependency set has no `fmt` feature.
    #[cfg(not(target_os = "android"))]
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .init();

    if args.probe {
        // Parse guarantees a code and an interface. Nothing here binds UDP
        // 9797 or builds a tunnel.
        let code = args.code.as_deref().unwrap_or_default();
        let run = ProbeRun::new();
        if let Err(e) = run.start_with_code(code, args.ifaces.clone(), args.seconds) {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
        while run.running() {
            std::thread::sleep(Duration::from_millis(100));
        }
        let status = run.status();
        println!("{status}");
        std::process::exit(if status.ends_with("probe: result: ok") {
            0
        } else {
            1
        });
    }

    // Lab/test only: fill the gateway's stats provider with a fixed line so a
    // namespace test can prove the channel before Android can feed it.
    if let Some(line) = &args.stats_fake_line {
        wgserv::joiner::phone_stats_provider().set_line(Some(line.clone()));
    }
    // The Android side reports this when sharing starts; the host lets the
    // lab pass the real words: an update from a new app, a crash, or none at
    // all from an old app.
    if let Some(reason) = args.stats_fake_exit {
        wgserv::joiner::phone_stats_provider().set_exit(reason);
    }
    let cfg = DiscoveryConfig {
        ifaces: args.iface_ips.clone(),
        ..Default::default()
    };
    let joiner = Joiner::new();
    if let Some(dir) = &args.state_dir {
        joiner.set_state_path(dir.join("state.toml"));
    }
    let mode = match choose_mode(args.code.as_deref(), joiner.is_paired()) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    };
    let started = match &mode {
        Mode::Pair(code) => joiner.sharer_start_with(code.clone(), cfg),
        Mode::Known => joiner.sharer_start_known(cfg),
    };
    if let Err(e) = started {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
    eprintln!(
        "sharer: {}, multicast iface {}",
        match &mode {
            Mode::Pair(code) => format!("pairing with code {code}"),
            Mode::Known => "known peer (no code)".to_string(),
        },
        if args.iface_ips.is_empty() {
            "kernel default".to_string()
        } else {
            args.iface_ips
                .iter()
                .map(|i| i.to_string())
                .collect::<Vec<_>>()
                .join(",")
        }
    );

    // Status is "state\npeer id\npeer name\nrx\ntx". The peer id is skipped
    // (key-shaped); the short peer name identifies the peer. The clock line
    // is for the labs: the state the phone displays and its connected time.
    let mut last = String::new();
    let mut last_clock = String::new();
    let mut last_clock_at: u64 = 0;
    loop {
        let status = joiner.status();
        let mut lines = status.lines();
        let state = lines.next().unwrap_or("").to_string();
        let line = format!("{} {}", state, lines.nth(1).unwrap_or(""));
        if line != last {
            eprintln!("sharer: state: {}", line.trim());
            last = line;
        }
        let at_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let clock = format!(
            "at_ms={} connected_ms={} neutral={}",
            at_ms,
            joiner.connected_ms().unwrap_or(0),
            u8::from(joiner.reconnecting())
        );
        // Once a second, not every poll: the labs read the last line.
        if clock != last_clock && at_ms >= last_clock_at + 1_000 {
            eprintln!("sharer: clock: {clock}");
            last_clock = clock;
            last_clock_at = at_ms;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(v: &[&str]) -> impl Iterator<Item = String> {
        v.iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>()
            .into_iter()
    }

    #[test]
    fn parses_code_only() {
        let a = parse_args(args(&["--code", "4321"])).unwrap();
        assert_eq!(a.code.as_deref(), Some("4321"));
        assert!(a.iface_ips.is_empty());
        assert_eq!(a.state_dir, None);
    }

    // The phone lists every non-cellular interface; the lab does the same
    // with the hotspot NOT first, so a last-one-wins parser fails it.
    #[test]
    fn repeated_iface_ip_keeps_every_address_in_order() {
        let a = parse_args(args(&[
            "--code",
            "1234",
            "--iface-ip",
            "192.168.77.1",
            "--iface-ip",
            "10.77.0.1",
        ]))
        .unwrap();
        assert_eq!(
            a.iface_ips,
            vec![
                "192.168.77.1".parse::<Ipv4Addr>().unwrap(),
                "10.77.0.1".parse().unwrap()
            ]
        );
    }

    // The probe: same code on both sides, interfaces by address.
    #[test]
    fn probe_needs_a_code_and_an_interface_and_has_a_default_window() {
        let a = parse_args(args(&[
            "--probe",
            "--code",
            "4321",
            "--iface-ip",
            "10.1.0.2/16",
        ]))
        .unwrap();
        assert!(a.probe);
        assert_eq!(a.seconds, 30);
        assert_eq!(a.ifaces[0].prefix, 16);
        let a = parse_args(args(&[
            "--probe",
            "--code",
            "4321",
            "--iface-ip",
            "10.1.0.2",
            "--seconds",
            "5",
        ]))
        .unwrap();
        assert_eq!((a.seconds, a.ifaces[0].prefix), (5, 24));
        // A state dir does not replace the code: the key exchange needs one.
        assert!(parse_args(args(&[
            "--probe",
            "--state-dir",
            "/tmp/x",
            "--iface-ip",
            "10.1.0.2"
        ]))
        .is_err());
        assert!(parse_args(args(&["--probe", "--code", "4321"])).is_err());
        assert!(parse_args(args(&["--code", "4321", "--seconds", "x"])).is_err());
    }

    #[test]
    fn state_dir_makes_the_code_optional() {
        let a = parse_args(args(&["--state-dir", "/tmp/x/sh"])).unwrap();
        assert_eq!(a.code, None);
        assert_eq!(a.state_dir, Some(std::path::PathBuf::from("/tmp/x/sh")));
        let a = parse_args(args(&["--state-dir", "/tmp/x/sh", "--code", "1234"])).unwrap();
        assert_eq!(a.code.as_deref(), Some("1234"));
        // Without a state dir nothing can be remembered: a code is needed.
        assert!(parse_args(args(&["--iface-ip", "10.1.0.2"])).is_err());
    }

    #[test]
    fn code_pairs_and_no_code_uses_the_stored_peer() {
        assert_eq!(
            choose_mode(Some("4321"), true),
            Ok(Mode::Pair("4321".into()))
        );
        assert_eq!(
            choose_mode(Some("4321"), false),
            Ok(Mode::Pair("4321".into()))
        );
        assert_eq!(choose_mode(None, true), Ok(Mode::Known));
        assert!(choose_mode(None, false)
            .unwrap_err()
            .contains("no paired device"));
    }

    #[test]
    fn parses_code_and_iface_ip() {
        let a = parse_args(args(&["--iface-ip", "10.1.0.2", "--code", "1234"])).unwrap();
        assert_eq!(a.code.as_deref(), Some("1234"));
        assert_eq!(a.iface_ips, vec!["10.1.0.2".parse::<Ipv4Addr>().unwrap()]);
    }

    #[test]
    fn stats_fake_line_is_optional_and_needs_a_value() {
        let a = parse_args(args(&[
            "--code",
            "4321",
            "--stats-fake-line",
            "JFS1 1 summary v=1 net=wifi",
        ]))
        .unwrap();
        assert_eq!(
            a.stats_fake_line.as_deref(),
            Some("JFS1 1 summary v=1 net=wifi")
        );
        assert!(parse_args(args(&["--code", "4321", "--stats-fake-line"])).is_err());
    }

    #[test]
    fn stats_fake_exit_takes_a_known_word_only() {
        let a = parse_args(args(&["--code", "4321", "--stats-fake-exit", "updated"])).unwrap();
        assert_eq!(
            a.stats_fake_exit,
            Some(jafnify_core::phone_exit::ExitReason::Updated)
        );
        let a = parse_args(args(&["--code", "4321", "--stats-fake-exit", "crash"])).unwrap();
        assert_eq!(
            a.stats_fake_exit,
            Some(jafnify_core::phone_exit::ExitReason::Crash)
        );
        assert!(parse_args(args(&["--code", "4321", "--stats-fake-exit"])).is_err());
        assert!(parse_args(args(&["--code", "4321", "--stats-fake-exit", "teleported"])).is_err());
        let none = parse_args(args(&["--code", "4321"])).unwrap();
        assert_eq!(none.stats_fake_exit, None, "an old app sends no word");
    }

    #[test]
    fn rejects_missing_or_bad_input() {
        assert!(parse_args(args(&[])).is_err());
        assert!(parse_args(args(&["--code", "12ab"])).is_err());
        assert!(parse_args(args(&["--code", "0123"])).is_err());
        assert!(parse_args(args(&["--code", "1234", "--iface-ip", "nope"])).is_err());
        assert!(parse_args(args(&["--code"])).is_err());
        assert!(parse_args(args(&["--code", "1234", "--bogus"])).is_err());
    }
}
