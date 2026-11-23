//! What the window (and the CLI status) shows for each state: one pure
//! function from the app state to texts and controls, the twin of the phone's
//! MainActivity.render(). The GUI only draws the result. The sentences are the
//! phone's wherever the same thing happens there.
//!
//! The platform supplies its facts through [`ViewSource`]; the wording and the
//! control decisions stay here.

use std::time::{SystemTime, UNIX_EPOCH};

use crate::snapshot::{CodeView, Health, RetryView, Snapshot};
use crate::ui::firewall::{blocked_help, FirewallKind, BLOCKED_STATUS};
use crate::ui::ui_text::{
    format_ago, format_bytes, format_elapsed, problem, retry_reason, Problem, FOREIGN_TUNNEL,
    LINK_LOST, TEARDOWN_DENIED,
};
use crate::ConnectionState;

pub const POLKIT_NOTE: &str = "Your password is needed to set up the tunnel.";
pub const KILL_SWITCH_NOTE: &str =
    "Kill switch on: the internet is blocked while the tunnel is down.";
const NO_NETWORK: &str = "No network - turn on the hotspot or join Wi-Fi";
const NO_PEER: &str = "No device found - make sure both devices are on the same hotspot or Wi-Fi network (still searching)";
const WAITING_FOR_PHONE: &str = "Waiting for phone...";

/// The one big button.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Primary {
    Connect,
    Cancel,
    Disconnect,
    TryAgain,
}

impl Primary {
    pub fn label(self) -> &'static str {
        match self {
            Primary::Connect => "Connect",
            Primary::Cancel => "Cancel",
            Primary::Disconnect => "Disconnect",
            Primary::TryAgain => "Try again",
        }
    }
}

/// Everything `view` needs, as plain values.
#[derive(Debug, Clone)]
pub struct ViewInput {
    pub state: ConnectionState,
    /// Display name of the paired device ("Phone - Icy Parrot").
    pub paired_name: Option<String>,
    /// A password prompt was refused (`App::denied`).
    pub denied: bool,
    /// The last start error, shown while idle.
    pub error: Option<String>,
    /// Why the connection was lost, while retrying.
    pub retry_reason: Option<String>,
    /// The search ran 60 s without finding the other device.
    pub no_peer: bool,
    /// The search is on the known track: a paired phone is expected to appear.
    pub waiting_for_phone: bool,
    /// Searching with no usable network on this computer.
    pub no_network: bool,
    /// The user chose "Pair new device": a code is asked for.
    pub pairing_new: bool,
    /// Digits typed so far.
    pub code_digits: usize,
    pub connected_secs: Option<u64>,
    /// (sent, received) bytes as the laptop sees them.
    pub traffic: Option<(u64, u64)>,
    pub last_connected_ago: Option<u64>,
    pub retry_secs_left: Option<u64>,
    /// The host firewall is dropping discovery (`App::firewall_blocked`).
    pub firewall: Option<FirewallKind>,
    /// The LAN interface a rule would be scoped to, if known.
    pub firewall_iface: Option<String>,
    /// A password prompt is expected for the next privileged tunnel call.
    /// Published by `App::polkit_prompt_expected`, never guessed from state.
    pub polkit_note: bool,
    /// While Connected, the uplink carrying the peer route is down.
    pub link_lost: bool,
    /// The main-screen verdict from jafnify-core, or none. The moving
    /// handshake age belongs in Details.
    pub health: Option<String>,
    /// Another Jafnify instance owns jafn0: show it and do not connect.
    pub foreign_tunnel: bool,
    /// The local code track expired before pairing completed.
    pub code_expired: bool,
    /// The kill switch is on: the internet is blocked while the tunnel is down.
    pub kill_switch: bool,
    /// The product name shown when no device is paired. The platform passes
    /// its own build identity ("Jafnify" / "Jafnify Test").
    pub app_name: &'static str,
}

impl ViewInput {
    pub fn new(state: ConnectionState) -> Self {
        Self {
            state,
            paired_name: None,
            denied: false,
            error: None,
            retry_reason: None,
            no_peer: false,
            waiting_for_phone: false,
            no_network: false,
            pairing_new: false,
            code_digits: 0,
            connected_secs: None,
            traffic: None,
            last_connected_ago: None,
            retry_secs_left: None,
            firewall: None,
            firewall_iface: None,
            polkit_note: false,
            link_lost: false,
            health: None,
            foreign_tunnel: false,
            code_expired: false,
            kill_switch: false,
            app_name: "Jafnify",
        }
    }

    /// The parts that come from the app itself. The caller fills in what only
    /// the window knows (`error`, `no_peer`, `pairing_new`, `code_digits`).
    pub fn from_app<S: ViewSource>(app: &S) -> Self {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let state = app.state();
        let reason = retry_reason(&app.status_text()).to_string();
        Self {
            paired_name: app.paired_name(),
            denied: app.denied(),
            retry_reason: (state == ConnectionState::Retry && !reason.is_empty()).then_some(reason),
            connected_secs: app.connected_secs(),
            traffic: app.traffic(),
            last_connected_ago: app
                .last_connected()
                .filter(|&t| t > 0)
                .map(|t| now.saturating_sub(t)),
            firewall: app.firewall_blocked(),
            firewall_iface: app.firewall_iface(),
            waiting_for_phone: app.waiting_for_phone(),
            no_network: app.no_network(),
            retry_secs_left: app.retry_secs_left(),
            polkit_note: app.polkit_prompt_expected(),
            link_lost: app.link_lost(),
            health: app.health_verdict(),
            foreign_tunnel: app.foreign_tunnel(),
            code_expired: app.code_expired(),
            kill_switch: app.kill_switch(),
            app_name: app.app_name(),
            ..Self::new(state)
        }
    }
}

/// The facts `from_app` reads from a platform app object.
pub trait ViewSource {
    fn state(&self) -> ConnectionState;
    fn status_text(&self) -> String;
    fn paired_name(&self) -> Option<String>;
    fn denied(&self) -> bool;
    fn connected_secs(&self) -> Option<u64>;
    fn traffic(&self) -> Option<(u64, u64)>;
    fn last_connected(&self) -> Option<u64>;
    fn firewall_blocked(&self) -> Option<FirewallKind>;
    fn firewall_iface(&self) -> Option<String>;
    fn waiting_for_phone(&self) -> bool;
    fn no_network(&self) -> bool;
    fn retry_secs_left(&self) -> Option<u64>;
    fn polkit_prompt_expected(&self) -> bool;
    fn link_lost(&self) -> bool;
    fn health_verdict(&self) -> Option<String>;
    fn foreign_tunnel(&self) -> bool;
    fn code_expired(&self) -> bool;
    fn kill_switch(&self) -> bool;
    /// The product name for this build.
    fn app_name(&self) -> &'static str;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct View {
    pub title: String,
    pub intro: Option<String>,
    pub status: String,
    /// The connection state, for the status indicator's colour.
    pub state: ConnectionState,
    /// The status line is an error: draw a warning prefix.
    pub warning: bool,
    /// The health sentence under the status, when there is one.
    pub health: Option<String>,
    pub elapsed: Option<String>,
    pub traffic: Option<String>,
    pub last: Option<String>,
    /// A second line under the status ("normal internet restored").
    pub extra: Option<String>,
    /// Shown while the kill switch is on: the internet is blocked whenever the
    /// tunnel is down, by design.
    pub kill_switch_note: Option<String>,
    pub show_code_entry: bool,
    pub code_hint: Option<String>,
    /// While asking for a code to pair a new device: a way back.
    pub show_cancel_entry: bool,
    /// Say, before it happens, that a password prompt is coming.
    pub polkit_note: bool,
    pub primary: Primary,
    pub primary_enabled: bool,
    /// "Pair new device" and "Forget device".
    pub show_pair_row: bool,
    /// Under the status when the firewall blocks the search: what is wrong
    /// and the rule that fixes it.
    pub help: Option<String>,
    /// Offer the rule (ufw, with a LAN interface to scope it to).
    pub offer_rule: bool,
}

pub fn view(i: &ViewInput) -> View {
    let idle = i.state == ConnectionState::Idle;
    let connected = i.state == ConnectionState::Connected;
    let paired = i.paired_name.is_some();
    let name = i.paired_name.as_deref().unwrap_or("");
    let entering = idle && !i.denied && (!paired || i.pairing_new);

    // A blocked firewall only means something while still searching.
    let blocked = i.firewall.filter(|_| i.state == ConnectionState::Searching);
    let (status, warning) = status_line(i, name);
    let (primary, primary_enabled) = if i.denied {
        (Primary::TryAgain, true)
    } else {
        match i.state {
            ConnectionState::Idle => (
                Primary::Connect,
                (!entering || i.code_digits == 4) && !i.foreign_tunnel,
            ),
            ConnectionState::Connected => (Primary::Disconnect, true),
            _ => (Primary::Cancel, true),
        }
    };

    View {
        title: if paired && !i.pairing_new {
            name
        } else {
            i.app_name
        }
        .to_string(),
        intro: (!paired && idle && !i.denied).then(|| {
            "This laptop uses the phone's internet through an encrypted tunnel.\n\n\
             Turn on the hotspot on the phone and connect this laptop to it."
                .to_string()
        }),
        status,
        state: i.state,
        warning,
        health: i.health.clone(),
        elapsed: i
            .connected_secs
            .filter(|_| connected)
            .map(|s| format!("Connected for {}", format_elapsed(s))),
        traffic: i.traffic.filter(|_| connected).map(|(sent, received)| {
            format!(
                "Sent {}   Received {}",
                format_bytes(sent),
                format_bytes(received)
            )
        }),
        last: (paired && idle && !i.denied && !i.pairing_new).then(|| match i.last_connected_ago {
            Some(ago) => format!("Last connected: {name}, {}", format_ago(ago)),
            None => format!("Last connected: {name}"),
        }),
        extra: (i.state == ConnectionState::Retry)
            .then(|| "Your normal internet connection was restored.".to_string()),
        kill_switch_note: i.kill_switch.then(|| KILL_SWITCH_NOTE.to_string()),
        show_code_entry: entering,
        code_hint: entering.then(|| {
            "Tap Share internet on the phone, then enter the code it shows here.".to_string()
        }),
        show_cancel_entry: idle && !i.denied && i.pairing_new,
        polkit_note: i.polkit_note,
        primary,
        primary_enabled,
        show_pair_row: paired && !i.pairing_new,
        help: blocked.map(|kind| blocked_help(kind, i.firewall_iface.as_deref())),
        offer_rule: blocked == Some(FirewallKind::Ufw) && i.firewall_iface.is_some(),
    }
}

// ---- CLI lines (the same names and sentences, one per line) ----------------

pub fn device_line(name: Option<&str>) -> String {
    match name {
        Some(n) => format!("Device: {n}"),
        None => "Device: none (not paired)".to_string(),
    }
}

pub fn forgotten_line(name: Option<&str>) -> String {
    match name {
        Some(n) => format!("Device forgotten: {n}"),
        None => "No paired device".to_string(),
    }
}

pub fn peer_line(name: &str, pair_id: &str) -> String {
    format!("{name}  {pair_id}")
}

/// The technical reason, indented under the status line it belongs to.
pub fn details_line(reason: &str) -> String {
    format!("  details: {reason}")
}

/// The core status line for this state, with the optional retry countdown.
fn core_status(i: &ViewInput) -> String {
    let retry = (i.state == ConnectionState::Retry).then(|| RetryView {
        remaining_secs: i.retry_secs_left.unwrap_or(0),
        reason: i.retry_reason.clone(),
    });
    // A Retry without a countdown still uses the core fallback line.
    let retry = if i.state == ConnectionState::Retry && i.retry_secs_left.is_none() {
        None
    } else {
        retry
    };
    Snapshot {
        state: i.state,
        code: None,
        retry,
        health: Health::default(),
    }
    .status_line()
}

/// One sentence for the status line, and whether it is a warning.
///
/// Every normal state uses the core's `Snapshot::status_line`; the name is
/// only the core's documented ` - <peer>` suffix. The platform-specific
/// sentences below are recoveries the core does not know (a blocked firewall,
/// a refused password, a code the user must replace).
fn status_line(i: &ViewInput, name: &str) -> (String, bool) {
    if i.denied {
        return ("Permission was denied - try again".to_string(), true);
    }
    if i.foreign_tunnel && i.state == ConnectionState::Idle {
        return (FOREIGN_TUNNEL.to_string(), false);
    }
    if i.state == ConnectionState::Idle {
        if let Some(error) = &i.error {
            if error == TEARDOWN_DENIED {
                return (TEARDOWN_DENIED.to_string(), true);
            }
            return match problem(error) {
                Problem::NoNetwork if i.paired_name.is_some() => {
                    (WAITING_FOR_PHONE.to_string(), false)
                }
                Problem::NoNetwork => (NO_NETWORK.to_string(), true),
                _ => ("Could not connect - see Details".to_string(), true),
            };
        }
    }
    // The local code track ran out before pairing completed. There is no
    // countdown on Linux; the card says so and never swaps in a new code.
    if i.code_expired
        && matches!(
            i.state,
            ConnectionState::Searching | ConnectionState::Pairing | ConnectionState::Connecting
        )
    {
        return (
            CodeView {
                digits: String::new(),
                expires_in_secs: None,
                expired: true,
            }
            .expiry_line(),
            true,
        );
    }
    if i.state == ConnectionState::Searching {
        if i.firewall.is_some() {
            return (BLOCKED_STATUS.to_string(), true);
        }
        if i.no_network {
            return (NO_NETWORK.to_string(), true);
        }
        if i.waiting_for_phone {
            return (WAITING_FOR_PHONE.to_string(), false);
        }
        if i.no_peer {
            return (NO_PEER.to_string(), false);
        }
    }
    if i.state == ConnectionState::Connected && i.link_lost {
        return (LINK_LOST.to_string(), true);
    }
    if i.state == ConnectionState::Retry
        && problem(i.retry_reason.as_deref().unwrap_or("")) == Problem::NoNetwork
    {
        return (NO_NETWORK.to_string(), true);
    }

    let status = if i.state == ConnectionState::Connected {
        let snapshot = Snapshot {
            state: i.state,
            code: None,
            retry: None,
            health: Health::default(),
        };
        snapshot.status_line_with_peer(Some(name).filter(|n| !n.is_empty()))
    } else {
        core_status(i)
    };
    (status, false)
}
