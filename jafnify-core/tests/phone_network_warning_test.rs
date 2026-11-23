//! The phone network changes the laptop warns about.

use jafnify_core::notification::Notification;
use jafnify_core::phone_network_warning::{warning, PhoneNetworkWarning};
use jafnify_core::phone_stats::NetworkKind;

#[test]
fn a_phone_that_lost_its_mobile_data_is_warned_about() {
    assert_eq!(
        warning(NetworkKind::Mobile, NetworkKind::None),
        Some(PhoneNetworkWarning::LostMobileData)
    );
    assert_eq!(
        warning(NetworkKind::Mobile4g, NetworkKind::None),
        Some(PhoneNetworkWarning::LostMobileData)
    );
    assert_eq!(
        PhoneNetworkWarning::LostMobileData.notification(),
        Notification::PhoneLostMobileData
    );
}

#[test]
fn a_phone_that_lost_wifi_is_not_a_mobile_data_warning() {
    assert_eq!(warning(NetworkKind::Wifi, NetworkKind::None), None);
    assert_eq!(warning(NetworkKind::Ethernet, NetworkKind::None), None);
}

#[test]
fn a_generation_drop_names_both_generations() {
    assert_eq!(
        warning(NetworkKind::Mobile5g, NetworkKind::Mobile4g),
        Some(PhoneNetworkWarning::GenerationDrop {
            from: "5G",
            to: "4G"
        })
    );
    assert_eq!(
        warning(NetworkKind::Mobile4g, NetworkKind::Mobile3g),
        Some(PhoneNetworkWarning::GenerationDrop {
            from: "4G",
            to: "3G"
        })
    );
    assert_eq!(
        PhoneNetworkWarning::GenerationDrop {
            from: "5G",
            to: "4G"
        }
        .notification(),
        Notification::PhoneGenerationDrop {
            from: "5G",
            to: "4G"
        }
    );
}

#[test]
fn an_improvement_a_recovery_or_no_change_is_silent() {
    assert_eq!(warning(NetworkKind::Mobile, NetworkKind::Wifi), None);
    assert_eq!(warning(NetworkKind::None, NetworkKind::Mobile), None);
    assert_eq!(warning(NetworkKind::Mobile4g, NetworkKind::Mobile4g), None);
    assert_eq!(warning(NetworkKind::Unknown, NetworkKind::None), None);
    assert_eq!(warning(NetworkKind::Mobile, NetworkKind::Unknown), None);
    assert_eq!(warning(NetworkKind::None, NetworkKind::Wifi), None);
}

#[test]
fn the_modem_variants_count_as_their_generation() {
    // A phone mostly reports 5G non-standalone; losing data from there must warn.
    assert_eq!(
        warning(NetworkKind::Mobile5gNsa, NetworkKind::None),
        Some(PhoneNetworkWarning::LostMobileData)
    );
    assert_eq!(
        warning(NetworkKind::Mobile4gPlus, NetworkKind::None),
        Some(PhoneNetworkWarning::LostMobileData)
    );
    assert_eq!(
        warning(NetworkKind::Mobile5gNsa, NetworkKind::Mobile4gCa),
        Some(PhoneNetworkWarning::GenerationDrop {
            from: "5G",
            to: "4G"
        })
    );
    assert_eq!(
        warning(NetworkKind::Mobile5gAdv, NetworkKind::Mobile5gNsa),
        None,
        "a change inside 5G is not a drop"
    );
    assert_eq!(
        warning(NetworkKind::Mobile4g, NetworkKind::Mobile5gMmwave),
        None
    );
    assert_eq!(warning(NetworkKind::Wifi, NetworkKind::None), None);
}
