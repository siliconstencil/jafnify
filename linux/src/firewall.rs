//! Host firewall handling: say so when the firewall eats discovery, and offer
//! one narrow rule instead of an endless "Searching".
//!
//! Inbound flows a joiner needs that conntrack does not cover: the 0x01, 0x02
//! and 0x03 frames, unsolicited. They are sent BOTH to the multicast group and
//! by unicast to the learned peer (`KnownPeer::learn` makes `Endpoint::send`
//! do both), and the PAKE confirms are unicast too. A rule that only opens the
//! group therefore drops the unicast copies on a ufw host, silently turning
//! the product's redundancy against lossy hotspot multicast off. The rule must
//! cover UDP 9799 arriving on the LAN interface whatever the destination.
//! WireGuard replies answer the laptop's own packets, so conntrack accepts
//! them and no inbound 9797 rule is needed. Scoping by interface (not the
//! current subnet) keeps the rule valid when a phone hotspot moves to a
//! different subnet.
//!
//! Detection is behavioural (see `jafnify_core::echo`): multicast loopback
//! re-enters the input path, so a firewall that drops the group also drops our
//! own announcements. `ufw.conf` is world-readable and only names the firewall;
//! plain nftables cannot be read without root, so it is detected by behaviour
//! alone.
//!
//! W2a moved the pure wording and rule text into
//! `jafnify_core::ui::firewall`; this module keeps the Linux detection.

use std::time::{Duration, Instant};

use jafnify_core::discovery::DiscoveryConfig;
use jafnify_core::echo::EchoWatch;

pub(crate) use jafnify_core::ui::firewall::group_and_port;
pub use jafnify_core::ui::firewall::{
    blocked_help, lan_subnet, nft_rule, rule_lines, ufw_allow_args, FirewallKind, BLOCKED_STATUS,
};

/// Silence this long while announcing (once per second) is a block.
pub const ECHO_GRACE: Duration = Duration::from_secs(3);
/// ufw's world-readable switch.
pub const UFW_CONF: &str = "/etc/ufw/ufw.conf";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Too early to say.
    Unknown,
    /// Our own announcements came back, or the peer's frames got through on
    /// some path: nothing local blocks discovery.
    Open,
    /// The firewall is dropping our frames.
    Blocked(FirewallKind),
}

/// The decision, as a pure function. `heard`: our own announcement came back.
/// `elapsed`: how long we have been announcing.
pub fn verdict(heard: bool, elapsed: Duration, ufw_active: bool) -> Verdict {
    if heard {
        Verdict::Open
    } else if elapsed < ECHO_GRACE {
        Verdict::Unknown
    } else if ufw_active {
        Verdict::Blocked(FirewallKind::Ufw)
    } else {
        Verdict::Blocked(FirewallKind::Other)
    }
}

/// `ENABLED=yes` in ufw.conf text (comments ignored, case-insensitive).
pub fn ufw_enabled(conf: &str) -> bool {
    conf.lines()
        .map(str::trim)
        .filter(|l| !l.starts_with('#'))
        .filter_map(|l| l.split_once('='))
        .any(|(k, v)| k.trim() == "ENABLED" && v.trim().eq_ignore_ascii_case("yes"))
}

/// Whether ufw is enabled on this host (unreadable file = not enabled).
pub fn ufw_enabled_on_host() -> bool {
    std::fs::read_to_string(UFW_CONF).is_ok_and(|c| ufw_enabled(&c))
}

/// A running check: listens for our own announcements and decides.
pub struct FirewallCheck {
    watch: EchoWatch,
    started: Instant,
    ufw: bool,
}

impl FirewallCheck {
    /// None if the listening socket cannot be opened (then there is no verdict).
    pub fn start(cfg: &DiscoveryConfig, now: Instant, ufw: bool) -> Option<Self> {
        let watch = EchoWatch::bind(cfg, cfg.interfaces()).ok()?;
        Some(Self {
            watch,
            started: now,
            ufw,
        })
    }

    /// Blocked only when nothing arrived on either path: our own announcement
    /// did not come back AND no frame from the peer got through (unicast
    /// discovery reaches us without the group).
    pub fn poll(&mut self, now: Instant) -> Verdict {
        let heard = self.watch.poll();
        if self.watch.peer_heard() {
            return Verdict::Open;
        }
        verdict(heard, now.saturating_duration_since(self.started), self.ufw)
    }
}
