//! Discovery probe: everything up to tunnel-up, nothing after.
//!
//! Runs the 0x01 discovery announce/hear and the PAKE key exchange
//! (0x05/0x06) with a pairing code, on the given interfaces, and reports what
//! it heard from whom. It builds no WireGuard config, starts no tunnel and
//! never binds the WireGuard port, so it can run while another VPN app (or
//! wg0) is up.
//!
//! Two devices run it with the same code at the same time. Both announce 0x01
//! and their PAKE Hello from the start, so each report says on its own what
//! that device hears: a sender nobody hears and a receiver behind a firewall
//! look different. No key and no code ever appears in the report.
//!
//! The probe never sends a frame that the code alone verifies: the presence
//! tag is public and the PAKE frames are keyed by the derived key (P0-1).

use std::net::{Ipv4Addr, SocketAddr};
use std::time::{Duration, Instant};

use rand::RngCore;

use crate::discovery::{Discovery, DiscoveryConfig, DiscoveryError};
use crate::echo::EchoWatch;
use crate::keyexchange::KeyPair;
use crate::pairing::{DiscoveryToken, PairingCode};
use crate::pake::{PakeError, PakeLink};
use crate::state::Role;
use crate::wire::Path;
use crate::ANNOUNCE_INTERVAL;

/// Frames handled per kind per loop turn, so a flood cannot starve the loop.
const MAX_FRAMES_PER_TURN: usize = 32;
const POLL: Duration = Duration::from_millis(50);
/// The most distinct sources the report keeps. A hostile LAN peer can spoof
/// many addresses during a user-started probe; past this the oldest is
/// dropped, so the newest N are the ones reported.
pub const MAX_SIGHTINGS: usize = 64;

/// One local interface, for labelling where a frame came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalIface {
    pub name: String,
    pub ip: Ipv4Addr,
    pub prefix: u8,
}

/// `name=ip/prefix` entries separated by commas ("swlan0=10.20.89.13/24"):
/// how the Android app hands its interfaces to native code. Junk is skipped.
pub fn parse_local_ifaces(list: &str) -> Vec<LocalIface> {
    list.split(',')
        .filter_map(|entry| {
            let (name, cidr) = entry.trim().split_once('=')?;
            let (ip, prefix) = cidr.split_once('/')?;
            let prefix: u8 = prefix.parse().ok().filter(|p| *p <= 32)?;
            Some(LocalIface {
                name: name.to_string(),
                ip: ip.parse().ok()?,
                prefix,
            })
        })
        .collect()
}

pub(crate) fn network(ip: Ipv4Addr, prefix: u8) -> u32 {
    let mask = u32::MAX.checked_shl(32 - u32::from(prefix)).unwrap_or(0);
    u32::from(ip) & mask
}

/// The interface a source address is on: the most specific local network
/// that holds it. (The probe only knows addresses, not the ingress interface.)
pub fn local_iface_for(ifaces: &[LocalIface], src: Ipv4Addr) -> Option<&str> {
    ifaces
        .iter()
        .filter(|i| network(i.ip, i.prefix) == network(src, i.prefix))
        .max_by_key(|i| i.prefix)
        .map(|i| i.name.as_str())
}

#[derive(Debug, Clone)]
pub struct ProbeConfig {
    pub code: PairingCode,
    /// The interfaces discovery runs on (empty = the kernel's default route,
    /// and then no echo can be judged).
    pub ifaces: Vec<LocalIface>,
    /// How long to listen at most.
    pub window: Duration,
    /// How long to keep announcing after the key exchange completed, so the
    /// other device can complete too.
    pub linger: Duration,
    /// Gateways that also get our frames by unicast (see `DiscoveryConfig`).
    pub unicast: Vec<Ipv4Addr>,
    /// The role the probe takes in the PAKE exchange: the laptop probes as
    /// the joiner, the phone as the sharer.
    pub role: Role,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SeenKind {
    /// 0x01 discovery token.
    Discovery,
    /// 0x06 PAKE Confirm: the peer's key exchange.
    KeyExchange,
}

impl SeenKind {
    /// The frame type byte as the reports print it.
    pub fn label(self) -> &'static str {
        match self {
            SeenKind::Discovery => "0x01",
            SeenKind::KeyExchange => "0x06",
        }
    }
}

/// Frames from one source, of one kind. Our own frames coming back are not a
/// sighting (they are the echo).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sighting {
    pub kind: SeenKind,
    pub from: Ipv4Addr,
    /// Authenticated under our code. `false`: another device is there, but
    /// with another code.
    pub verified: bool,
    /// Whether the frame came by multicast or by unicast to us.
    pub via: Path,
    pub count: u32,
}

#[derive(Debug, Clone)]
pub struct ProbeReport {
    pub sightings: Vec<Sighting>,
    /// Our own announcements came back (None: no own address to judge by).
    pub echo: Option<bool>,
    /// The last send error, if announcing failed at some point.
    pub send_error: Option<String>,
    ifaces: Vec<LocalIface>,
}

/// The label the reports use for how a frame arrived.
fn path_label(path: Path) -> &'static str {
    match path {
        Path::Multicast => "multicast",
        Path::Unicast => "unicast",
    }
}

impl ProbeReport {
    fn first_verified(&self, kind: SeenKind) -> Option<&Sighting> {
        self.sightings.iter().find(|s| s.kind == kind && s.verified)
    }

    /// A verified frame from another device arrived by unicast (not multicast):
    /// discovery works without the group reaching us.
    pub fn peer_heard_by_unicast(&self) -> bool {
        self.sightings
            .iter()
            .any(|s| s.verified && s.via == Path::Unicast)
    }

    /// A verified 0x01 from another device arrived.
    pub fn discovery_done(&self) -> bool {
        self.first_verified(SeenKind::Discovery).is_some()
    }

    /// A verified 0x06 from another device arrived: the key exchange works in
    /// this direction. (The probe stops here: no tunnel.)
    pub fn key_exchange_done(&self) -> bool {
        self.first_verified(SeenKind::KeyExchange).is_some()
    }

    /// The report, one line each, all starting "probe: ".
    pub fn lines(&self) -> Vec<String> {
        let mut out = vec![format!(
            "probe: interfaces: {}",
            if self.ifaces.is_empty() {
                "kernel default".to_string()
            } else {
                self.ifaces
                    .iter()
                    .map(|i| format!("{}={}/{}", i.name, i.ip, i.prefix))
                    .collect::<Vec<_>>()
                    .join(", ")
            }
        )];
        for s in &self.sightings {
            out.push(format!(
                "probe: seen {} from {} via {} ({}) x{}{}",
                s.kind.label(),
                s.from,
                local_iface_for(&self.ifaces, s.from).unwrap_or("?"),
                path_label(s.via),
                s.count,
                if s.verified { "" } else { " (other code)" }
            ));
        }
        out.push(match self.first_verified(SeenKind::Discovery) {
            Some(s) => format!(
                "probe: discovery: ok (peer {}, {})",
                s.from,
                path_label(s.via)
            ),
            None => "probe: discovery: no peer heard".to_string(),
        });
        out.push(match self.first_verified(SeenKind::KeyExchange) {
            Some(s) => format!(
                "probe: key exchange: ok (peer {}, {})",
                s.from,
                path_label(s.via)
            ),
            None => "probe: key exchange: not completed".to_string(),
        });
        out.push(match self.echo {
            Some(true) => "probe: own echo: yes".to_string(),
            Some(false) => {
                "probe: own echo: no (our own announcements did not come back)".to_string()
            }
            None => "probe: own echo: unknown".to_string(),
        });
        if let Some(e) = &self.send_error {
            out.push(format!("probe: send error: {e}"));
        }
        out
    }
}

/// Add one sighting: a known one counts up, a new one is pushed and, past
/// [`MAX_SIGHTINGS`], the oldest is dropped so the report keeps the newest.
/// Returns true when this is a first sighting (the caller prints it once).
fn note_sighting(sightings: &mut Vec<Sighting>, sighting: Sighting) -> bool {
    if let Some(existing) = sightings.iter_mut().find(|s| {
        s.kind == sighting.kind
            && s.from == sighting.from
            && s.verified == sighting.verified
            && s.via == sighting.via
    }) {
        existing.count = existing.count.saturating_add(1);
        return false;
    }
    if sightings.len() >= MAX_SIGHTINGS {
        sightings.remove(0);
    }
    sightings.push(sighting);
    true
}

/// Runs the probe. `on_seen` is called the first time each (kind, source,
/// verified) is seen, for live output. Returns when the window is over, or
/// `linger` after the key exchange completed.
pub fn run_probe(
    cfg: &ProbeConfig,
    on_seen: impl FnMut(&Sighting),
) -> Result<ProbeReport, DiscoveryError> {
    let addr = crate::DISCOVERY_ADDR
        .parse()
        .expect("DISCOVERY_ADDR is a valid socket address");
    run_probe_at(cfg, addr, on_seen)
}

/// `run_probe` on an explicit discovery address, so a test does not have to
/// share the product's 9799 socket with a running app on the host.
pub fn run_probe_at(
    cfg: &ProbeConfig,
    addr: SocketAddr,
    mut on_seen: impl FnMut(&Sighting),
) -> Result<ProbeReport, DiscoveryError> {
    let dcfg = DiscoveryConfig {
        addr,
        ifaces: cfg.ifaces.iter().map(|i| i.ip).collect(),
        unicast: cfg.unicast.clone(),
        ..Default::default()
    };
    let discovery = Discovery::bind(&dcfg)?;
    let own = dcfg.interfaces();
    let mut echo = (!own.is_empty())
        .then(|| EchoWatch::bind(&dcfg, own).ok())
        .flatten();

    let keypair = KeyPair::generate();
    // The probe binds no gateway: it announces no port.
    let mut pake = PakeLink::open(&dcfg, cfg.role, &cfg.code, &keypair, 0).map_err(map_pake)?;
    let mut nonce = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut nonce);
    let token = DiscoveryToken::derive(&nonce);

    let mut report = ProbeReport {
        sightings: Vec::new(),
        echo: echo.as_ref().map(|_| false),
        send_error: None,
        ifaces: cfg.ifaces.clone(),
    };
    let mut note = |report: &mut ProbeReport, kind, from: Ipv4Addr, verified, via: Path| {
        let sighting = Sighting {
            kind,
            from,
            verified,
            via,
            count: 1,
        };
        if note_sighting(&mut report.sightings, sighting.clone()) {
            on_seen(&sighting);
        }
    };

    let start = Instant::now();
    let mut last_announce: Option<Instant> = None;
    let mut done_at: Option<Instant> = None;
    loop {
        let now = Instant::now();
        if now.duration_since(start) >= cfg.window
            || done_at.is_some_and(|d| now.duration_since(d) >= cfg.linger)
        {
            break;
        }
        if last_announce.is_none_or(|t| now.duration_since(t) >= ANNOUNCE_INTERVAL) {
            // An interface that is down must not end the probe: the report
            // shows the silence (and the error).
            if let Err(e) = discovery.announce(&token) {
                report.send_error = Some(e.to_string());
            }
            last_announce = Some(now);
        }
        // The PAKE Hello goes out with the same cadence and the Confirm comes
        // back from the peer that has the same code.
        let poll = pake.poll(now).map_err(map_pake)?;
        if let Some(peer) = poll.peer {
            if let std::net::IpAddr::V4(ip) = peer.endpoint.ip() {
                note(&mut report, SeenKind::KeyExchange, ip, true, peer.path);
                if done_at.is_none() {
                    done_at = Some(now);
                }
            }
        }
        for _ in 0..MAX_FRAMES_PER_TURN {
            let Some((from, via, tok)) = discovery.try_receive_via(Duration::ZERO)? else {
                break;
            };
            if tok.nonce != nonce {
                if let std::net::IpAddr::V4(ip) = from.ip() {
                    let verified = tok.verify();
                    if verified {
                        discovery.learn(from);
                    }
                    note(&mut report, SeenKind::Discovery, ip, verified, via);
                }
            }
        }
        if let Some(e) = echo.as_mut() {
            report.echo = Some(e.poll());
        }
        std::thread::sleep(POLL);
    }
    Ok(report)
}

/// The PAKE errors as discovery errors: the probe reports one error type.
fn map_pake(e: PakeError) -> DiscoveryError {
    match e {
        PakeError::Io(e) => DiscoveryError::Io(e),
        PakeError::Discovery(e) => e,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::{Accept, Endpoint};
    use hmac::{Hmac, Mac};
    use sha2::Sha256;

    /// A port other than the product's 9799 keeps a running app out.
    const TEST_PORT: u16 = 49806;

    fn sighting(from: Ipv4Addr) -> Sighting {
        Sighting {
            kind: SeenKind::Discovery,
            from,
            verified: false,
            via: Path::Multicast,
            count: 1,
        }
    }

    #[test]
    fn the_sightings_keep_the_newest_and_never_grow_past_the_cap() {
        let mut sightings = Vec::new();
        for host in 1..=(MAX_SIGHTINGS as u32 + 5) {
            let from = Ipv4Addr::new(192, 168, 1, host as u8);
            assert!(note_sighting(&mut sightings, sighting(from)));
        }
        assert_eq!(sightings.len(), MAX_SIGHTINGS);
        assert_eq!(
            sightings.first().map(|s| s.from),
            Some(Ipv4Addr::new(192, 168, 1, 6)),
            "the oldest five are dropped"
        );
        assert_eq!(
            sightings.last().map(|s| s.from),
            Some(Ipv4Addr::new(192, 168, 1, MAX_SIGHTINGS as u8 + 5))
        );
    }

    #[test]
    fn a_repeat_sighting_counts_up_and_is_not_a_first_sighting() {
        let mut sightings = Vec::new();
        let from = Ipv4Addr::new(192, 168, 1, 2);
        assert!(note_sighting(&mut sightings, sighting(from)));
        assert!(!note_sighting(&mut sightings, sighting(from)));
        assert_eq!(sightings.len(), 1);
        assert_eq!(sightings[0].count, 2);
    }

    fn cfg() -> DiscoveryConfig {
        DiscoveryConfig {
            iface: Some(Ipv4Addr::LOCALHOST),
            addr: format!("239.255.42.99:{TEST_PORT}").parse().unwrap(),
            ..Default::default()
        }
    }

    /// The legacy 0x04 MAC: HMAC-SHA256(code, public_key || port).
    fn legacy_04_verifies(code: &PairingCode, frame: &[u8]) -> bool {
        if frame.len() != 67 || frame[0] != 0x04 {
            return false;
        }
        let mut mac = Hmac::<Sha256>::new_from_slice(code.as_str().as_bytes())
            .expect("HMAC accepts any key length");
        mac.update(&frame[1..35]);
        mac.verify_slice(&frame[35..]).is_ok()
    }

    /// The legacy 0x01 MAC: HMAC-SHA256(code, nonce).
    fn legacy_01_verifies(code: &PairingCode, frame: &[u8]) -> bool {
        if frame.len() != 49 || frame[0] != 0x01 {
            return false;
        }
        let mut mac = Hmac::<Sha256>::new_from_slice(code.as_str().as_bytes())
            .expect("HMAC accepts any key length");
        mac.update(&frame[1..17]);
        mac.verify_slice(&frame[17..]).is_ok()
    }

    /// The probe must never put a frame on the wire that the pairing code
    /// alone verifies: a captured frame would allow an offline brute force of
    /// the 9,000 codes (P0-1).
    #[test]
    fn the_probe_never_sends_a_frame_the_code_alone_verifies() {
        let dcfg = cfg();
        let rx = Endpoint::open(&dcfg, Accept::Any).expect("receiver");
        let code = PairingCode::parse("1234").expect("code");
        let pcfg = ProbeConfig {
            code: code.clone(),
            ifaces: vec![LocalIface {
                name: "lo".to_string(),
                ip: Ipv4Addr::LOCALHOST,
                prefix: 8,
            }],
            window: Duration::from_millis(800),
            linger: Duration::from_millis(100),
            unicast: Vec::new(),
            role: Role::Joiner,
        };
        run_probe_at(&pcfg, dcfg.addr, |_| {}).expect("probe ran");

        let mut frames = Vec::new();
        while let Ok(Some(d)) = rx.recv(Duration::ZERO) {
            frames.push(d.frame);
        }
        assert!(!frames.is_empty(), "the probe sent nothing");
        for f in &frames {
            assert!(
                !legacy_04_verifies(&code, f),
                "the probe sent a 0x04 frame the code alone verifies"
            );
            assert!(
                !legacy_01_verifies(&code, f),
                "the probe sent a 0x01 frame the code alone verifies"
            );
        }
    }
}
