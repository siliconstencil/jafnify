//! Feature flags shared by every frontend.
//!
//! One table is the source of truth for the id, bucket, default, platform
//! mask and wording of every opt-in switch in version 0.2. Storage and UI
//! live in the frontends; this module only describes the rows.

/// The fixed note under the Experimental bucket.
pub const EXPERIMENTAL_NOTE: &str = "May change or be removed.";

/// A standard row is always on and has no switch. It stays in the table so
/// the id, the platform mask and the consumer are still written down in one
/// place; `render_list` never shows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bucket {
    Standard,
    Settings,
    Advanced,
    Experimental,
}

impl Bucket {
    /// The order every settings screen renders the buckets in. `Standard`
    /// has no switch and is deliberately absent.
    pub const ORDER: [Bucket; 3] = [Bucket::Settings, Bucket::Advanced, Bucket::Experimental];

    /// The stable lowercase key used on the wire and in storage.
    pub fn key(self) -> &'static str {
        match self {
            Bucket::Standard => "standard",
            Bucket::Settings => "settings",
            Bucket::Advanced => "advanced",
            Bucket::Experimental => "experimental",
        }
    }

    /// The visible bucket title.
    pub fn title(self) -> &'static str {
        match self {
            Bucket::Standard => "Standard",
            Bucket::Settings => "Settings",
            Bucket::Advanced => "Advanced",
            Bucket::Experimental => "Experimental",
        }
    }

    /// Settings starts open; the other two start collapsed. A standard row
    /// has no switch and is never collapsed.
    pub fn collapsed_by_default(self) -> bool {
        matches!(self, Bucket::Advanced | Bucket::Experimental)
    }

    /// The fixed note a bucket shows above its rows, if any.
    pub fn note(self) -> Option<&'static str> {
        match self {
            Bucket::Experimental => Some(EXPERIMENTAL_NOTE),
            Bucket::Standard | Bucket::Settings | Bucket::Advanced => None,
        }
    }
}

/// Hand-written platform bits for [`FeatureFlag::platforms`].
pub mod platform {
    pub const ANDROID: u8 = 1;
    pub const LINUX: u8 = 2;
    pub const WINDOWS: u8 = 4;
    pub const IOS: u8 = 8;
    pub const ALL: u8 = ANDROID | LINUX | WINDOWS | IOS;
}

/// One opt-in switch in the shared settings model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FeatureFlag {
    /// Stable lowercase snake-case key; also the storage key.
    pub id: &'static str,
    pub bucket: Bucket,
    pub default: bool,
    pub platforms: u8,
    pub label: &'static str,
    pub help: &'static str,
    /// False until a frontend really reads the row. The render list drops
    /// unwired rows, so a released build never shows a switch that does
    /// nothing.
    pub wired: bool,
}

/// Every row of version 0.2, grouped by bucket. Wording lives here so Linux,
/// Android and a future Windows client show the same words. A standard row is
/// always on and is never rendered as a switch.
pub const FLAGS: &[FeatureFlag] = &[
    FeatureFlag {
        id: "tile_home",
        bucket: Bucket::Standard,
        default: true,
        platforms: platform::ANDROID | platform::LINUX,
        label: "Show the home screen as instrument tiles",
        help: "Shows the connection, the signal, the heat and the battery as tiles on the phone, and the DNS answer time, the signal, the phone, today's data, the Wi-Fi to the phone and the drops as tiles on the laptop.",
        wired: true,
    },
    FeatureFlag {
        id: "phone_stats",
        bucket: Bucket::Standard,
        default: true,
        platforms: platform::LINUX | platform::ANDROID,
        label: "Share phone measurements with the paired computer",
        help: "Sends the phone's network type, speed, battery and data totals to the paired computer inside the tunnel. Nothing leaves the two devices.",
        wired: true,
    },
    FeatureFlag {
        id: "carrier_quality",
        bucket: Bucket::Standard,
        default: true,
        platforms: platform::ANDROID,
        label: "Show DNS answer time",
        help: "Adds the DNS answer time of the lookups the phone already makes to the measurements sent to the paired computer. No extra packets and no phone permission.",
        wired: true,
    },
    FeatureFlag {
        id: "battery_status",
        bucket: Bucket::Standard,
        default: true,
        platforms: platform::ANDROID,
        label: "Show phone battery and temperature",
        help: "Shows the phone's battery level and temperature on this phone and on the paired computer.",
        wired: true,
    },
    FeatureFlag {
        id: "wifi_range",
        bucket: Bucket::Standard,
        default: true,
        platforms: platform::LINUX,
        label: "Show how far the laptop is from the phone",
        help: "Shows the Wi-Fi signal the laptop hears from the phone's hotspot as a tile, with the likely distance, so you can see a drop coming when you walk away. Walls and pockets weaken the signal too.",
        wired: true,
    },
    FeatureFlag {
        id: "usage_statistics",
        bucket: Bucket::Standard,
        default: true,
        platforms: platform::LINUX | platform::ANDROID,
        label: "Usage and speed statistics",
        help: "Shows totals and speeds per session and over time: data shared, average, peak and lowest speed, busy hours, network type. Stays on this device.",
        wired: true,
    },
    FeatureFlag {
        id: "drop_history",
        bucket: Bucket::Standard,
        default: true,
        platforms: platform::LINUX,
        label: "Keep a history of connection drops",
        help: "Keeps what the phone saw when the link went down, so you can read the drops of the last day and their likely cause. Stays on this device.",
        wired: true,
    },
    FeatureFlag {
        id: "phone_heat_warning",
        bucket: Bucket::Standard,
        default: true,
        platforms: platform::LINUX,
        label: "Warn when the phone gets hot",
        help: "Says once when Android reports that the phone is limiting itself because of the heat, and again after it cooled down. A phone that does not report it is warned about when it gets too hot. The temperature stays on this device.",
        wired: true,
    },
    FeatureFlag {
        id: "phone_network_warning",
        bucket: Bucket::Standard,
        default: true,
        platforms: platform::LINUX,
        label: "Warn when the phone loses its network",
        help: "Shows a desktop notification when the phone's network changes, for example when it loses mobile data.",
        wired: true,
    },
    FeatureFlag {
        id: "start_after_restart",
        bucket: Bucket::Settings,
        default: false,
        platforms: platform::ANDROID,
        label: "Start sharing after the phone restarts",
        help: "Starts sharing again after the device restarts, if a device is paired.",
        wired: true,
    },
    FeatureFlag {
        id: "resume_if_stopped",
        bucket: Bucket::Settings,
        default: false,
        platforms: platform::ANDROID,
        label: "Resume sharing if the system stops the app",
        help: "Starts sharing again after the system stops the app, if a device is paired.",
        wired: true,
    },
    FeatureFlag {
        id: "notify_connection_change",
        bucket: Bucket::Settings,
        default: false,
        platforms: platform::LINUX,
        label: "Notify when the connection changes",
        help: "Shows a desktop notification when the connection is made, lost or retried.",
        wired: true,
    },
    FeatureFlag {
        id: "data_budget",
        bucket: Bucket::Settings,
        default: false,
        platforms: platform::ANDROID,
        label: "Mobile data budget",
        help: "Counts the data this app shares on mobile and warns at 80% of the allowance you enter.",
        wired: true,
    },
    FeatureFlag {
        id: "keep_screen_on",
        bucket: Bucket::Settings,
        default: false,
        platforms: platform::ANDROID,
        label: "Keep the screen on while sharing",
        help: "Keeps the screen on while Jafnify is on the screen and sharing. It only works while the app is open. It uses more battery and a screen left on warms the phone. Only for phones that stop sharing when the screen turns off.",
        wired: true,
    },
    FeatureFlag {
        id: "network_details",
        bucket: Bucket::Settings,
        default: false,
        platforms: platform::ANDROID,
        label: "Network details",
        help: "Shows how long the phone was on each network generation, how often it switched, the battery drain of the same hours and the signal history of the day. Needs the phone's network permission.",
        wired: true,
    },
    FeatureFlag {
        id: "unicast_search",
        bucket: Bucket::Advanced,
        default: false,
        platforms: platform::LINUX | platform::ANDROID,
        label: "Find the other device without multicast",
        help: "Uses direct messages instead of multicast to look for the other device.",
        wired: false,
    },
    FeatureFlag {
        id: "best_spot",
        bucket: Bucket::Experimental,
        default: false,
        platforms: platform::ANDROID | platform::LINUX,
        label: "Find the best spot for the phone",
        help: "Shows the signal and the delay every second for two minutes, so you can compare where the phone sits. On the phone the same switch starts the session.",
        wired: true,
    },
    FeatureFlag {
        id: "good_time_notice",
        bucket: Bucket::Experimental,
        default: false,
        platforms: platform::LINUX,
        label: "Notice a good time for big downloads",
        help: "Says once when the phone has 5G, a good signal and a quiet line for a while, so a big download can start.",
        wired: true,
    },
    FeatureFlag {
        id: "good_hours",
        bucket: Bucket::Experimental,
        default: false,
        platforms: platform::LINUX,
        label: "Learn the busy hours of the network",
        help: "Learns over days which hours the phone's network is usually quick or busy, from the DNS answer time the phone already measures. Stays on this computer. Places change the network too; this learns from where the laptop has been.",
        wired: true,
    },
    FeatureFlag {
        id: "device_panel",
        bucket: Bucket::Experimental,
        default: false,
        platforms: platform::ANDROID,
        label: "Show what the phone can do right now",
        help: "Shows the phone's battery, memory and the limits Android puts on Jafnify. Nothing is sent anywhere.",
        wired: true,
    },
];

/// Whether a row applies to a platform.
pub fn supports(flag: &FeatureFlag, platform: u8) -> bool {
    flag.platforms & platform != 0
}

/// The row with this id, if it is declared.
pub fn find(id: &str) -> Option<&'static FeatureFlag> {
    FLAGS.iter().find(|flag| flag.id == id)
}

/// True when a row is standard: always on, with no switch to show.
pub fn is_standard(flag: &FeatureFlag) -> bool {
    flag.bucket == Bucket::Standard
}

/// The merged "Network details" switch. It replaced `network_report` and
/// `signal_history`; a stored choice for the new key wins, and an update that
/// has no new key yet is on when either old key was on. Nothing turns off.
pub fn network_details_on(stored: Option<bool>, old_report: bool, old_signal: bool) -> bool {
    stored.unwrap_or(old_report || old_signal)
}

/// Whether a row is a standard feature by id. An unknown id is not standard.
pub fn standard_id(id: &str) -> bool {
    find(id).is_some_and(is_standard)
}

/// True when a row may be shown: it is a switch, it is wired and the platform
/// is supported. A standard row is never shown as a switch.
pub fn is_rendered(flag: &FeatureFlag, platform: u8) -> bool {
    flag.wired && !is_standard(flag) && supports(flag, platform)
}

/// The rows a UI on `platform` may render. Unwired rows are dropped, so a
/// released build cannot display a switch that does nothing.
pub fn render_list(platform: u8) -> impl Iterator<Item = &'static FeatureFlag> {
    FLAGS.iter().filter(move |flag| is_rendered(flag, platform))
}

#[cfg(test)]
mod tests {
    use super::*;

    const KNOWN: [u8; 4] = [
        platform::ANDROID,
        platform::LINUX,
        platform::WINDOWS,
        platform::IOS,
    ];

    #[test]
    fn ids_are_non_empty_and_unique() {
        let mut seen = std::collections::BTreeSet::new();
        for flag in FLAGS {
            assert!(!flag.id.is_empty(), "empty flag id");
            assert!(seen.insert(flag.id), "duplicate flag id {}", flag.id);
        }
    }

    #[test]
    fn labels_and_help_are_ascii_single_line_and_non_empty() {
        for flag in FLAGS {
            for (name, text) in [("label", flag.label), ("help", flag.help)] {
                assert!(!text.is_empty(), "{} of {} is empty", name, flag.id);
                assert!(text.is_ascii(), "{} of {} is not ASCII", name, flag.id);
                assert!(
                    !text.contains('\t') && !text.contains('\n') && !text.contains('\r'),
                    "{} of {} is not one line",
                    name,
                    flag.id
                );
            }
        }
    }

    #[test]
    fn standard_rows_default_on_and_the_switches_default_off() {
        for flag in FLAGS {
            if is_standard(flag) {
                assert!(flag.default, "{} must default ON", flag.id);
            } else {
                assert!(!flag.default, "{} must default OFF", flag.id);
            }
        }
    }

    #[test]
    fn standard_rows_are_one_source_and_never_rendered() {
        let standard: Vec<&str> = FLAGS
            .iter()
            .filter(|flag| is_standard(flag))
            .map(|flag| flag.id)
            .collect();
        assert_eq!(
            standard,
            vec![
                "tile_home",
                "phone_stats",
                "carrier_quality",
                "battery_status",
                "wifi_range",
                "usage_statistics",
                "drop_history",
                "phone_heat_warning",
                "phone_network_warning",
            ]
        );
        for flag in FLAGS.iter().filter(|flag| is_standard(flag)) {
            for platform_id in KNOWN {
                assert!(
                    !is_rendered(flag, platform_id),
                    "{} is standard and must have no switch",
                    flag.id
                );
            }
            assert!(flag.wired, "{} still names a consumer", flag.id);
        }
        assert!(!standard_id("no_such_flag"));
        assert!(standard_id("tile_home"));
        assert!(!standard_id("best_spot"));
    }

    #[test]
    fn no_help_tells_the_user_to_turn_on_a_standard_row() {
        // The standard rows are always on; a help that asks for them would
        // send the user to a switch that no longer exists.
        for flag in FLAGS {
            for standard in FLAGS.iter().filter(|flag| is_standard(flag)) {
                assert!(
                    !flag.help.contains(standard.id),
                    "{} help names the standard id {}",
                    flag.id,
                    standard.id
                );
            }
            assert!(
                !flag.help.contains("measurements switch"),
                "{} help asks for a removed switch",
                flag.id
            );
            assert!(
                !flag.help.contains("Needs the tile home"),
                "{} help asks for a removed switch",
                flag.id
            );
        }
    }

    #[test]
    fn platform_masks_use_only_known_platforms() {
        for flag in FLAGS {
            assert_ne!(
                flag.platforms & platform::ALL,
                0,
                "{} names no platform",
                flag.id
            );
            assert_eq!(
                flag.platforms & !platform::ALL,
                0,
                "{} has unknown platform bits",
                flag.id
            );
        }
    }

    #[test]
    fn the_reviewed_id_list_is_frozen() {
        let ids: Vec<&str> = FLAGS.iter().map(|flag| flag.id).collect();
        assert_eq!(
            ids,
            vec![
                "tile_home",
                "phone_stats",
                "carrier_quality",
                "battery_status",
                "wifi_range",
                "usage_statistics",
                "drop_history",
                "phone_heat_warning",
                "phone_network_warning",
                "start_after_restart",
                "resume_if_stopped",
                "notify_connection_change",
                "data_budget",
                "keep_screen_on",
                "network_details",
                "unicast_search",
                "best_spot",
                "good_time_notice",
                "good_hours",
                "device_panel",
            ]
        );
    }

    #[test]
    fn the_tile_home_flag_is_standard_and_always_on() {
        let flag = find("tile_home").expect("tile_home is declared");
        assert_eq!(flag.bucket, Bucket::Standard);
        assert!(flag.default);
        assert!(supports(flag, platform::LINUX));
        assert!(supports(flag, platform::ANDROID));
        assert!(flag.wired, "the row names a consumer");
        assert!(!is_rendered(flag, platform::ANDROID));
    }

    #[test]
    fn the_merged_network_details_row_replaces_the_two_old_switches() {
        let flag = find("network_details").expect("network_details is declared");
        assert_eq!(flag.bucket, Bucket::Settings);
        assert!(!flag.default);
        assert!(supports(flag, platform::ANDROID));
        assert!(flag.wired);
        assert!(find("network_report").is_none(), "the old row is gone");
        assert!(find("signal_history").is_none(), "the old row is gone");
    }

    #[test]
    fn the_phone_network_warning_is_a_standard_linux_row() {
        let flag = find("phone_network_warning").expect("phone_network_warning is declared");
        assert_eq!(flag.bucket, Bucket::Standard);
        assert!(flag.default);
        assert!(supports(flag, platform::LINUX));
        assert!(!supports(flag, platform::ANDROID));
        assert!(flag.wired, "the row names a consumer");
    }

    #[test]
    fn the_good_hours_help_carries_the_note_about_the_places() {
        let flag = find("good_hours").expect("good_hours is declared");
        assert!(
            flag.help.ends_with(crate::good_hours::PLACE_NOTE),
            "the help must carry the note about the places: {}",
            flag.help
        );
    }

    #[test]
    fn supports_matches_the_platform_mask() {
        for flag in FLAGS {
            for platform_id in KNOWN {
                assert_eq!(
                    supports(flag, platform_id),
                    flag.platforms & platform_id != 0,
                    "{} on platform {platform_id}",
                    flag.id
                );
            }
        }
        let start = find("start_after_restart").expect("start_after_restart is declared");
        assert!(supports(start, platform::ANDROID));
        assert!(!supports(start, platform::LINUX));
    }

    #[test]
    fn find_returns_none_for_an_unknown_id() {
        assert!(find("no_such_flag").is_none());
    }

    #[test]
    fn buckets_have_a_fixed_order_and_title() {
        assert_eq!(
            Bucket::ORDER,
            [Bucket::Settings, Bucket::Advanced, Bucket::Experimental]
        );
        assert_eq!(Bucket::Settings.title(), "Settings");
        assert_eq!(Bucket::Advanced.title(), "Advanced");
        assert_eq!(Bucket::Experimental.title(), "Experimental");
    }

    #[test]
    fn buckets_have_stable_lowercase_keys() {
        assert_eq!(Bucket::Settings.key(), "settings");
        assert_eq!(Bucket::Advanced.key(), "advanced");
        assert_eq!(Bucket::Experimental.key(), "experimental");
    }

    #[test]
    fn only_advanced_and_experimental_start_collapsed() {
        assert!(!Bucket::Settings.collapsed_by_default());
        assert!(Bucket::Advanced.collapsed_by_default());
        assert!(Bucket::Experimental.collapsed_by_default());
    }

    #[test]
    fn only_the_experimental_bucket_carries_the_fixed_note() {
        assert_eq!(Bucket::Settings.note(), None);
        assert_eq!(Bucket::Advanced.note(), None);
        assert_eq!(Bucket::Experimental.note(), Some(EXPERIMENTAL_NOTE));
        assert_eq!(EXPERIMENTAL_NOTE, "May change or be removed.");
    }

    #[test]
    fn every_wired_row_names_its_consumer() {
        // Flipping `wired` is not a one-line change: add the id and the code
        // that reads it here first, so a dead switch cannot ship.
        const CONSUMERS: &[(&str, &str)] = &[
            ("tile_home", "linux/src/daemon.rs:home"),
            (
                "tile_home",
                "app/app/src/main/java/com/siliconstencil/jafnify/MainActivity.java:renderTileHome",
            ),
            ("phone_stats", "linux/src/daemon.rs:tiles"),
            (
                "phone_stats",
                "app/app/src/main/java/com/siliconstencil/jafnify/PhoneSampler.java:onTick",
            ),
            (
                "carrier_quality",
                "app/app/src/main/java/com/siliconstencil/jafnify/PhoneSampler.java:onTick",
            ),
            (
                "battery_status",
                "app/app/src/main/java/com/siliconstencil/jafnify/PhoneSampler.java:onTick",
            ),
            ("wifi_range", "linux/src/daemon.rs:range_tile"),
            ("usage_statistics", "linux/src/daemon.rs:tiles"),
            (
                "usage_statistics",
                "app/app/src/main/java/com/siliconstencil/jafnify/StatisticsActivity.java:onCreate",
            ),
            ("drop_history", "linux/src/app.rs:record_drop"),
            (
                "phone_heat_warning",
                "linux/src/daemon.rs:update_phone_heat",
            ),
            (
                "phone_network_warning",
                "linux/src/daemon.rs:notify_phone_network",
            ),
            (
                "start_after_restart",
                "app/app/src/main/java/com/siliconstencil/jafnify/BootReceiver.java:onReceive",
            ),
            (
                "resume_if_stopped",
                "app/app/src/main/java/com/siliconstencil/jafnify/SharerService.java:resumeAfterSystemRestart",
            ),
            (
                "notify_connection_change",
                "linux/src/daemon.rs:notify_connection",
            ),
            (
                "data_budget",
                "app/app/src/main/java/com/siliconstencil/jafnify/PhoneSampler.java:onTick",
            ),
            (
                "keep_screen_on",
                "app/app/src/main/java/com/siliconstencil/jafnify/MainActivity.java:applyKeepScreenOn",
            ),
            (
                "network_details",
                "app/app/src/main/java/com/siliconstencil/jafnify/SharerService.java:networkDetails",
            ),
            (
                "best_spot",
                "app/app/src/main/java/com/siliconstencil/jafnify/BestSpotActivity.java:onCreate",
            ),
            ("best_spot", "linux/src/daemon.rs:update_best_spot"),
            ("good_time_notice", "linux/src/daemon.rs:update_good_time"),
            ("good_hours", "linux/src/daemon.rs:update_good_hours"),
            (
                "device_panel",
                "app/app/src/main/java/com/siliconstencil/jafnify/DevicePanelActivity.java:onCreate",
            ),
        ];
        assert_eq!(
            CONSUMERS
                .iter()
                .filter(|(id, _)| *id == "phone_stats")
                .count(),
            2,
            "phone_stats has a Linux reader and an Android producer"
        );
        for flag in FLAGS.iter().filter(|flag| flag.wired) {
            assert!(
                CONSUMERS.iter().any(|(id, _)| *id == flag.id),
                "{} is wired but names no consumer",
                flag.id
            );
        }
    }

    #[test]
    fn unwired_rows_are_never_in_the_render_list() {
        let wired = FeatureFlag {
            id: "test_only",
            bucket: Bucket::Settings,
            default: false,
            platforms: platform::ANDROID,
            label: "Test only",
            help: "Test only.",
            wired: true,
        };
        assert!(is_rendered(&wired, platform::ANDROID));
        assert!(!is_rendered(&wired, platform::LINUX));
        for flag in FLAGS {
            for platform_id in KNOWN {
                assert_eq!(
                    is_rendered(flag, platform_id),
                    flag.wired && !is_standard(flag) && supports(flag, platform_id),
                    "{} on platform {platform_id}",
                    flag.id
                );
            }
        }
        for platform_id in KNOWN {
            assert!(
                render_list(platform_id).all(|flag| flag.wired && !is_standard(flag)),
                "an unwired or standard row reached the render list"
            );
        }
    }
}
