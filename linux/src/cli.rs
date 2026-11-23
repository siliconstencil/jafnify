//! Headless CLI frontend (clap). Optional companion to the GUI.

use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(
    name = "jafnify",
    version,
    about = "Jafnify P2P tunnel (Linux)",
    after_help = jafnify_core::terms::HELP_FOOTER
)]
pub struct Cli {
    /// Accept the use-at-your-own-risk terms (asked once; needed before
    /// start, join or pair when the terms are not accepted yet)
    #[arg(long, global = true)]
    pub accept_terms: bool,
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Start the tunnel with the persisted role (joiner default). Without
    /// --code the paired device is used (see `pair`)
    Start {
        #[arg(long)]
        code: Option<String>,
    },
    /// Join a sharer peer (force joiner role). Without --code the paired
    /// device is used
    Join {
        #[arg(long)]
        code: Option<String>,
    },
    /// Pair a new peer with the given code and connect (replaces the paired
    /// device)
    Pair {
        #[arg(long)]
        code: String,
    },
    /// Forget the paired device (deletes its stored key)
    Forget,
    /// Tear the tunnel down
    Stop,
    /// Remove a dead tunnel's routing so normal internet works; keeps jafn0
    /// for a fast return. Used by systemd ExecStopPost; safe to run twice
    Fallback,
    /// Show connection status
    Status,
    /// Measure the round trip to the paired device. Linux only, while
    /// connected; one shot, never background traffic
    LinkTest,
    /// Show the phone measurements it shared. Linux only, while connected;
    /// one request, never background traffic
    PhoneStats,
    /// Show the drop history of the last day: how many drops, how long the
    /// longest one lasted and what the phone saw. Linux only, read-only
    Drops,
    /// Show the hours in which the phone's network was usually quick and the
    /// hours in which it was usually busy. Linux only, read-only
    Hours,
    /// List known peers
    Peers,
    /// Safe check of the whole discovery path: finds the other device and runs
    /// the key exchange with a code, then stops. No tunnel, no WireGuard
    /// config, never uses port 9797; exit code 0 only if the key exchange
    /// completed. Also says whether the host firewall blocks discovery
    Probe {
        /// The 4-digit code the other device shows (or runs its probe with)
        #[arg(long)]
        code: String,
        /// How long to listen at most
        #[arg(long, default_value_t = 30)]
        seconds: u64,
    },
    /// Host firewall: show, or add, the one narrow rule discovery needs
    Firewall {
        #[command(subcommand)]
        action: FirewallCommand,
    },
    /// Kill switch (off by default): while the tunnel is up, block traffic that
    /// would leave this computer outside the tunnel. On applies at the next connection
    KillSwitch {
        #[command(subcommand)]
        action: KillSwitchCommand,
    },
    /// Run the windowless tray daemon (normally started by the window or
    /// autostart; owns the tunnel, tray icon and log)
    Tray,
    /// Install the root helper, the polkit action, the desktop entry and the
    /// icons (one password prompt). Run from the build directory or after
    /// installing the binaries together.
    InstallHelper,
    /// Remove the installed binaries, helper, policy, per-user rule, desktop
    /// entry and icons (one password prompt). The paired state is kept.
    Uninstall,
}

#[derive(Debug, Subcommand)]
pub enum KillSwitchCommand {
    /// Turn the kill switch on (applies at the next connection)
    On,
    /// Turn the kill switch off (its rules are removed now if the tunnel is up)
    Off,
}

#[derive(Debug, Subcommand)]
pub enum FirewallCommand {
    /// Print the rule that lets discovery through (UDP 9799 to the discovery
    /// group, on your Wi-Fi interface only), for ufw and for nftables
    Rule,
    /// Add that rule to ufw. Shows it, asks first, and uses your password;
    /// never disables or changes anything else
    Allow {
        /// Do not ask
        #[arg(long)]
        yes: bool,
    },
}
