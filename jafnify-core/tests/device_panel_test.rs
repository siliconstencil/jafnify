//! The device panel: the facts, the hints and the fixed wording.
//!
//! Pure module: the caller hands over one "key=value;key=value" blob and
//! gets back the lines.

use jafnify_core::device_panel::DeviceFacts;

fn full() -> DeviceFacts {
    DeviceFacts {
        maker: Some("Pixel".to_string()),
        model: Some("8".to_string()),
        android: Some("Android 14".to_string()),
        sdk: Some(34),
        app_version: Some("0.3.0".to_string()),
        battery_pct: Some(78),
        charging: Some(true),
        temp_c_x10: Some(395),
        thermal: Some(1),
        battery_saver: Some(true),
        exempt_from_optimisation: Some(true),
        background_restricted: Some(false),
        data_saver: Some(1),
        mem_free_mb: Some(1200),
        mem_total_mb: Some(4000),
        uptime_secs: Some(11_520),
    }
}

#[test]
fn the_facts_are_shown_as_lines() {
    assert_eq!(
        full().lines(),
        [
            "Phone: Pixel 8",
            "Android: 14 (SDK 34)",
            "Jafnify: 0.3.0",
            "Battery: 78%, charging",
            "Phone temperature: 39.5 C",
            "Battery saver: on",
            "Battery optimisation: off",
            "Background restricted: no",
            "Data saver: on",
            "Memory free: 1200 MB of 4000 MB",
            "Uptime: 3 h 12 min",
        ]
    );
}

#[test]
fn an_unknown_fact_leaves_its_line_out() {
    let facts = DeviceFacts::default();
    assert!(facts.lines().is_empty());
    assert!(facts.hints().is_empty());
    assert_eq!(facts.text(), "");

    let only_battery = DeviceFacts {
        battery_pct: Some(50),
        charging: Some(false),
        ..DeviceFacts::default()
    };
    assert_eq!(only_battery.lines(), ["Battery: 50%"]);
}

#[test]
fn the_battery_saver_hint_appears_only_when_it_is_on() {
    let on = DeviceFacts {
        battery_saver: Some(true),
        ..DeviceFacts::default()
    };
    assert_eq!(
        on.hints(),
        ["Battery saver is on: Android may stop sharing in the background."]
    );
    let off = DeviceFacts {
        battery_saver: Some(false),
        ..DeviceFacts::default()
    };
    assert!(off.hints().is_empty());
}

#[test]
fn the_battery_optimisation_hint_appears_when_the_phone_is_not_exempt() {
    let not_exempt = DeviceFacts {
        exempt_from_optimisation: Some(false),
        ..DeviceFacts::default()
    };
    assert_eq!(
        not_exempt.hints(),
        [
            "Battery optimisation is on for Jafnify: the phone can stop sharing when the screen is off."
        ]
    );
    let exempt = DeviceFacts {
        exempt_from_optimisation: Some(true),
        ..DeviceFacts::default()
    };
    assert!(exempt.hints().is_empty());
}

#[test]
fn the_background_restricted_hint_appears_only_when_it_is_restricted() {
    let restricted = DeviceFacts {
        background_restricted: Some(true),
        ..DeviceFacts::default()
    };
    assert_eq!(
        restricted.hints(),
        ["Background restricted: the phone can stop sharing in the background."]
    );
    let free = DeviceFacts {
        background_restricted: Some(false),
        ..DeviceFacts::default()
    };
    assert!(free.hints().is_empty());
}

#[test]
fn the_heat_hint_appears_from_the_severe_thermal_status_on() {
    let mild = DeviceFacts {
        thermal: Some(2),
        ..DeviceFacts::default()
    };
    assert!(
        mild.hints().is_empty(),
        "a mild thermal status says nothing"
    );
    for status in 3..=4 {
        let hot = DeviceFacts {
            thermal: Some(status),
            ..DeviceFacts::default()
        };
        assert_eq!(
            hot.hints(),
            ["The phone is very hot: Android can slow the sharing down."]
        );
    }
}

#[test]
fn the_data_saver_hint_appears_for_the_statuses_one_to_three() {
    let off = DeviceFacts {
        data_saver: Some(0),
        ..DeviceFacts::default()
    };
    assert!(off.hints().is_empty(), "the data saver is off");
    for status in 1..=3 {
        let on = DeviceFacts {
            data_saver: Some(status),
            ..DeviceFacts::default()
        };
        assert_eq!(on.hints(), ["Data saver is on: it can slow sharing."]);
    }
}

#[test]
fn the_limit_hints_never_call_themselves_a_certainty() {
    let facts = DeviceFacts {
        battery_saver: Some(true),
        exempt_from_optimisation: Some(false),
        background_restricted: Some(true),
        data_saver: Some(1),
        thermal: Some(3),
        ..DeviceFacts::default()
    };
    for hint in facts.hints() {
        assert!(
            hint.contains(" can ") || hint.contains(" may "),
            "{hint} must not be a certainty"
        );
    }
}

#[test]
fn the_text_is_the_lines_and_then_the_hints() {
    let mut facts = full();
    facts.exempt_from_optimisation = Some(false);
    let text = facts.text();
    let lines = facts.lines();
    let hints = facts.hints();
    assert_eq!(lines.len() + hints.len(), text.lines().count());
    assert_eq!(text.lines().next().unwrap(), lines[0]);
    assert_eq!(text.lines().last().unwrap(), hints[hints.len() - 1]);
}

#[test]
fn the_blob_parses_and_an_unknown_key_is_ignored() {
    let facts = DeviceFacts::parse(
        "maker=Pixel;model=8;android=Android 14;sdk=34;app_version=0.3.0;battery_pct=78;\
         charging=true;temp_c_x10=395;thermal=1;battery_saver=true;\
         exempt_from_optimisation=true;background_restricted=false;data_saver=1;\
         mem_free_mb=1200;mem_total_mb=4000;uptime_secs=11520;future_key=42",
    );
    assert_eq!(facts, full());
}

#[test]
fn a_bad_value_leaves_its_fact_unknown() {
    let facts = DeviceFacts::parse("battery_pct=abc;sdk=;maker=Pixel");
    assert_eq!(facts.battery_pct, None);
    assert_eq!(facts.sdk, None);
    assert_eq!(facts.maker.as_deref(), Some("Pixel"));
    assert_eq!(facts.lines(), ["Phone: Pixel"]);
}

#[test]
fn an_empty_blob_has_nothing_to_show() {
    let facts = DeviceFacts::parse("");
    assert_eq!(facts, DeviceFacts::default());
    assert_eq!(facts.text(), "");
}
