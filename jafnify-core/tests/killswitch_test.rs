//! Laptop kill switch. Opt-in. While
//! it is on, every outgoing packet that does not leave via the tunnel interface
//! is dropped, except loopback, the hotspot LAN (which carries the WireGuard
//! packets to the phone) and DHCP. The rules live in their own chain or table,
//! so they are removed as one unit.

use jafnify_core::config::{
    kill_switch_down, kill_switch_up, ConfigGenerator, InboundGuard, KillSwitch,
};

const PRIV: &str = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";
const PUB: &str = "BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB=";

#[test]
fn the_iptables_kill_switch_drops_everything_except_tunnel_lan_and_dhcp() {
    let up = kill_switch_up(true, "192.168.77.0/24");
    assert!(up.contains("-N JAFNIFY_KS"), "{up}");
    assert!(up.contains("-A JAFNIFY_KS -o %i -j ACCEPT"), "{up}");
    assert!(up.contains("-A JAFNIFY_KS -o lo -j ACCEPT"), "{up}");
    assert!(
        up.contains("-A JAFNIFY_KS -d 192.168.77.0/24 -j ACCEPT"),
        "{up}"
    );
    assert!(up.contains("--dport 67:68 -j ACCEPT"), "{up}");
    // The drop is the last rule of the chain.
    assert!(up.trim_end().ends_with("-A JAFNIFY_KS -j DROP"), "{up}");
}

#[test]
fn the_nft_kill_switch_is_one_table_with_a_final_drop() {
    let up = kill_switch_up(false, "192.168.77.0/24");
    assert!(up.contains("nft add table inet jafnify_killswitch"), "{up}");
    assert!(up.contains("oifname %i accept"), "{up}");
    assert!(up.contains("ip daddr 192.168.77.0/24 accept"), "{up}");
    assert!(up.trim_end().ends_with("output drop"), "{up}");
}

#[test]
fn the_down_commands_remove_the_whole_kill_switch() {
    let ipt = kill_switch_down(true);
    assert!(ipt.contains("-D OUTPUT -j JAFNIFY_KS"), "{ipt}");
    assert!(ipt.contains("-X JAFNIFY_KS"), "{ipt}");
    assert!(kill_switch_down(false).contains("nft delete table inet jafnify_killswitch"));
}

#[test]
fn the_joiner_with_a_kill_switch_adds_it_after_the_inbound_guard() {
    let ks = KillSwitch {
        iptables: true,
        lan: "192.168.43.0/24".to_string(),
    };
    let cfg = ConfigGenerator::joiner_config_full(
        PRIV,
        PUB,
        "192.168.43.1:9797",
        InboundGuard::Iptables,
        false,
        Some(&ks),
    );
    let up = cfg.interface.post_up.clone().unwrap();
    let down = cfg.interface.post_down.clone().unwrap();
    let guard_at = up.find("--ctstate NEW -j DROP").expect("no guard");
    let ks_at = up.find("JAFNIFY_KS").expect("no kill switch");
    assert!(guard_at < ks_at, "{up}");
    assert!(down.contains("-X JAFNIFY_KS"), "{down}");
    assert_eq!(
        ConfigGenerator::parse(&ConfigGenerator.to_wg_quick_text(&cfg)).unwrap(),
        cfg
    );
}

#[test]
fn without_the_kill_switch_the_config_has_no_kill_chain() {
    let cfg = ConfigGenerator::joiner_config_full(
        PRIV,
        PUB,
        "192.168.43.1:9797",
        InboundGuard::Iptables,
        false,
        None,
    );
    assert!(!cfg.interface.post_up.unwrap().contains("JAFNIFY_KS"));
}
