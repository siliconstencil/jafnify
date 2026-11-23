//! Jafnify core: platform-agnostic pairing, discovery, config, state, and
//! connection orchestration. Consumed by the Android and Linux frontends.

pub mod battery;
pub mod best_spot;
pub mod bottleneck;
pub mod budget;
pub mod config;
pub mod connclock;
pub mod connection_details;
pub mod connection_log;
pub mod device_panel;
pub mod diag;
pub mod discovery;
pub mod drop_history;
pub mod drop_store;
pub mod echo;
pub mod flags;
pub mod good_hours;
pub mod good_hours_store;
pub mod good_time;
pub mod handshake;
pub mod home_layout;
pub mod icon;
pub mod keyexchange;
pub mod known;
pub mod line_file;
pub mod link;
pub mod link_test;
pub mod names;
pub mod network_report;
pub mod nickname;
pub mod notification;
pub mod orchestrator;
pub mod pairing;
pub mod pake;
pub mod phone_exit;
pub mod phone_heat;
pub mod phone_network_warning;
pub mod phone_stats;
pub mod platform;
pub mod private_fs;
pub mod probe;
pub mod redact;
pub mod route;
pub mod scope;
pub mod session;
pub mod share_notice;
pub mod signal_history;
pub mod snapshot;
pub mod state;
pub mod terms;
pub mod tile_scopes;
pub mod tiles;
pub mod timing;
pub mod tokens;
pub mod ui;
pub mod usage;
pub mod wifi_leg;
pub mod wifi_range;
pub mod wire;

// Convenience re-exports consumed by the Linux and Android frontends.
pub use config::{ConfigGenerator, WireGuardConfig};
pub use discovery::{Discovery, DiscoveryConfig};
// Only the receiver is re-exported: the legacy 0x04 announcement type has no
// builder any more, so no frontend can put a code-verifiable frame on the wire.
pub use handshake::Handshake;
pub use keyexchange::KeyPair;
pub use orchestrator::{ConnectionState, Event, Orchestrator};

use std::time::Duration;

/// WireGuard MTU used in every generated config.
pub const MTU: u16 = 1280;
/// Full-tunnel route for every generated config.
pub const ALLOWED_IPS: &str = "0.0.0.0/0";
/// Example network used by generated configs (sharer side).
pub const EXAMPLE_NETWORK: &str = "10.9.9.0/24";
/// Sharer's tunnel IP.
pub const SHARER_IP: &str = "10.9.9.1";
/// Joiner's tunnel IP.
pub const JOINER_IP: &str = "10.9.9.2";
/// Address where the sharer's gateway answers DNS (and ping). Joiner configs
/// point `DNS` at it; the Android/host gateway listens there (port 53).
pub const GATEWAY_DNS_IP: &str = "10.233.0.1";
/// Pairing code lifetime.
pub const CODE_TTL: Duration = Duration::from_secs(600);
/// Fixed reconnect interval (no backoff).
pub const RETRY_INTERVAL: Duration = Duration::from_secs(10);
/// A side in Connecting re-announces its 0x02 frame this often until it has
/// seen WireGuard data from the peer (a single announcement can be lost).
pub const ANNOUNCE_INTERVAL: Duration = Duration::from_secs(1);
/// Connecting gives up (-> Retry -> fresh search) if no WireGuard data from
/// the peer arrived within this long.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
/// A tunnel whose rx counter has not moved for this long is dead. Must stay
/// above the 25 s persistent keepalive.
pub const RX_STALL_TIMEOUT: Duration = Duration::from_secs(35);
/// UDP multicast address used for LAN discovery.
pub const DISCOVERY_ADDR: &str = "239.255.42.99:9799";
/// The WireGuard port a sharer tries first (the legacy WgServer port). If it
/// is taken, the sharer uses another port and announces it: the joiner reads
/// the port from the MAC-covered handshake, so it never has to be fixed.
pub const LISTEN_PORT: u16 = 9797;
/// Shown to a user whose other device still runs the version before ports
/// (frame 0x02 / known version 1). Both devices must be updated to pair.
pub const OLD_PEER_MESSAGE: &str =
    "The other device runs an older Jafnify - update it, then try again";
/// Short old-peer reason shown on the main status card of both apps.
pub const OLD_PEER_CARD_REASON: &str = "Update Jafnify on the other device";
/// Shown by the joiner when the code it typed has outlived its lifetime. The
/// phone owns the code, so only the phone can issue a new one.
pub const CODE_EXPIRED_MESSAGE: &str = "Code expired - get a new code on the phone";
