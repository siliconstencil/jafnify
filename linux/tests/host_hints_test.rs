//! The host hint rules: only an explicit positive counts, and no SSID or
//! BSSID ever reaches the text.

use jafnify_linux::firewall::FirewallKind;
use jafnify_linux::host_hints::{
    firewall_hint, parse_access_points, parse_iw_power_save, parse_wifi_devices, HostHint,
};

#[test]
fn the_wifi_devices_come_from_the_real_nmcli_type_column() {
    // Real output from NetworkManager on Ubuntu 26.04.
    assert_eq!(
        parse_wifi_devices("wlo1:802-11-wireless\nlo:loopback\njafn0:wireguard\n"),
        vec!["wlo1".to_string()]
    );
    assert!(parse_wifi_devices("enp3s0:802-3-ethernet\n").is_empty());
    assert!(parse_wifi_devices(":802-11-wireless\n").is_empty());
    assert!(parse_wifi_devices("").is_empty());
    assert!(parse_wifi_devices(
        "Error: invalid field '802-11-wireless.powersave'; allowed fields: NAME,UUID"
    )
    .is_empty());
}

#[test]
fn only_an_explicit_on_counts_in_the_iw_output() {
    assert!(parse_iw_power_save("Power save: on\n"));
    assert!(!parse_iw_power_save("Power save: off\n"));
    assert!(!parse_iw_power_save(
        "command failed: No such device (-19)\n"
    ));
    assert!(!parse_iw_power_save(""));
}

#[test]
fn two_access_points_on_the_connected_network_are_the_hint() {
    assert!(parse_access_points(
        "yes:AA\\:BB\\:CC\\:DD\\:EE\\:FF:Home\nno:11\\:22\\:33\\:44\\:55\\:66:Home\nno:77\\:88\\:99\\:AA\\:BB\\:CC:Neighbour\n"
    ));
    assert!(!parse_access_points(
        "yes:AA\\:BB\\:CC\\:DD\\:EE\\:FF:Home\nno:11\\:22\\:33\\:44\\:55\\:66:Neighbour\n"
    ));
    assert!(!parse_access_points(
        "no:AA\\:BB\\:CC\\:DD\\:EE\\:FF:Home\n"
    ));
    assert!(!parse_access_points("yes:AA\\:BB\\:CC\\:DD\\:EE\\:FF:\n"));
    // The real marker is `*` for the connected network and a space for the
    // others (NetworkManager).
    assert!(parse_access_points(
        "*:AA\\:BB\\:CC\\:DD\\:EE\\:FF:Home\n :11\\:22\\:33\\:44\\:55\\:66:Home\n"
    ));
    assert!(!parse_access_points(
        "*:AA\\:BB\\:CC\\:DD\\:EE\\:FF:Home\n :11\\:22\\:33\\:44\\:55\\:66:Other\n"
    ));
    assert!(!parse_access_points(""));
}

#[test]
fn the_wording_is_the_preflight_wording() {
    let wifi = HostHint::WifiPowerSaving {
        iface: "wlan0".to_string(),
    };
    assert_eq!("Wi-Fi power saving is on for wlan0.", wifi.title());
    assert_eq!(
        "This can add short stalls. Jafnify does not change it; turn it off in your network \
         settings if stalls persist.",
        wifi.help()
    );

    let aps = HostHint::SeveralAccessPoints;
    assert_eq!(
        "This network name is seen from several access points.",
        aps.title()
    );
    assert_eq!(
        "Roaming between them can cause short stalls. Jafnify does not change your network \
         settings.",
        aps.help()
    );
}

#[test]
fn the_firewall_hint_uses_the_existing_help_text() {
    let hint = firewall_hint(FirewallKind::Ufw, Some("wlan0"));
    assert_eq!("Your firewall is blocking the search", hint.title());
    assert_eq!(
        jafnify_linux::firewall::blocked_help(FirewallKind::Ufw, Some("wlan0")),
        hint.help()
    );
}

#[test]
fn the_text_never_carries_a_network_name_or_an_address() {
    let output = "yes:AA\\:BB\\:CC\\:DD\\:EE\\:FF:Home\nno:11\\:22\\:33\\:44\\:55\\:66:Home\n";
    assert!(parse_access_points(output));
    let hint = HostHint::SeveralAccessPoints;
    assert!(!hint.title().contains("Home"));
    assert!(!hint.help().contains("Home"));
    assert!(!hint.title().contains("AA:BB"));
}
