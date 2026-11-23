//! The merged Android "Network details" row and its migration.

use jafnify_core::flags::{find, network_details_on, platform, supports, Bucket};

#[test]
fn the_merged_network_details_row_is_android_settings_and_opt_in() {
    let flag = find("network_details").expect("network_details is declared");
    assert_eq!(flag.bucket, Bucket::Settings);
    assert!(!flag.default, "the phone's network permission stays opt-in");
    assert!(supports(flag, platform::ANDROID));
    assert!(!supports(flag, platform::LINUX));
    assert!(flag.wired, "network_details names a consumer");
}

#[test]
fn the_two_old_rows_are_gone() {
    assert!(find("network_report").is_none());
    assert!(find("signal_history").is_none());
}

#[test]
fn an_update_never_turns_network_details_off() {
    // No new key yet: on when either old switch was on.
    assert!(network_details_on(None, true, false));
    assert!(network_details_on(None, false, true));
    assert!(network_details_on(None, true, true));
    assert!(!network_details_on(None, false, false));
    // A stored choice of the new key is the truth, both ways.
    assert!(network_details_on(Some(true), false, false));
    assert!(!network_details_on(Some(false), true, true));
}
