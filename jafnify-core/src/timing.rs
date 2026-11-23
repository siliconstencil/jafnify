//! Shared timing policy for routing around a dead tunnel.
//!
//! Platform-independent on purpose: the Linux joiner and a future Windows
//! client must agree on when the system's own routing wins again.
//!
//! By design, with the kill switch off, the machine's normal networking is the
//! default and the tunnel only carries traffic while it is alive. Routing
//! follows measured liveness, not the connection state.

use std::time::Duration;

/// The tunnel has been silent this long: send a liveness probe through it
/// (an ICMP echo to the gateway, answered by the peer itself), and again at
/// this interval while it stays silent.
pub const LIVENESS_PROBE_IDLE: Duration = Duration::from_secs(2);

/// No data from the peer for this long despite probes: the tunnel is dead and
/// the system's own routing wins. Three unanswered probes plus a margin.
pub const LIVENESS_DEAD_AFTER: Duration = Duration::from_secs(7);

/// The same when the system forbids unprivileged ping and the probe is a
/// plain UDP packet: the peer then answers only with WireGuard's passive
/// keepalive, which may take up to 10 s.
pub const LIVENESS_DEAD_AFTER_NO_PING: Duration = Duration::from_secs(15);

/// The uplink came back (Wi-Fi reconnected): a probe goes out at once and the
/// peer, if it is on this network, answers in milliseconds. No answer within
/// this time means another network, and the system's routing wins at once.
/// A Wi-Fi blip back to the same hotspot rides through untouched.
pub const LINK_BACK_ANSWER: Duration = Duration::from_secs(2);

/// Minimum time between two routing changes, so a lossy hotspot cannot flip
/// the routing on every gap.
pub const MIN_ROUTE_TOGGLE: Duration = Duration::from_secs(3);

/// A known-peer reconnect after a fallback. When the routing goes back to the
/// tunnel, WireGuard may still hold a session younger than its rekey time and
/// keeps sending with it; it starts a fresh handshake only after its no-reply
/// timer (keepalive 10 s + rekey 5 s + jitter, about 15 s), which races the
/// plain CONNECT_TIMEOUT. Wait long enough for that plus one retry (the
/// 15 s timeout tore the tunnel down every time).
pub const RETURN_CONNECT_TIMEOUT: Duration = Duration::from_secs(35);
