//! The canonical kinds the phone reports and the stored signal sample line.

use jafnify_core::network_report::Generation;
use jafnify_core::signal_history::SignalSample;

#[test]
fn the_phone_kinds_map_to_the_core_wire_words() {
    assert_eq!(Generation::from_kind("lte").map(|g| g.wire()), Some("4g"));
    assert_eq!(
        Generation::from_kind("lte_ca").map(|g| g.wire()),
        Some("4g-ca")
    );
    assert_eq!(
        Generation::from_kind("lte_advanced_pro").map(|g| g.wire()),
        Some("4g+")
    );
    assert_eq!(Generation::from_kind("nr").map(|g| g.wire()), Some("5g"));
    assert_eq!(
        Generation::from_kind("nr_nsa").map(|g| g.wire()),
        Some("5g-nsa")
    );
    assert_eq!(
        Generation::from_kind("nr_nsa_mmwave").map(|g| g.wire()),
        Some("5g-mmwave")
    );
    assert_eq!(
        Generation::from_kind("nr_advanced").map(|g| g.wire()),
        Some("5g-adv")
    );
    assert!(Generation::from_kind("unknown").is_none());
    assert!(Generation::from_kind("").is_none());
}

#[test]
fn a_signal_sample_has_one_stored_line() {
    let sample = SignalSample {
        at_ms: 1_700_000_000_000,
        level: 3,
    };
    assert_eq!(sample.line(), "JSG1 1 at=1700000000000 level=3");
    assert_eq!(
        SignalSample::parse_line("JSG1 1 at=1700000000000 level=3"),
        Some(sample)
    );
    assert_eq!(SignalSample::parse_line("JSG1 1 at=1 level=9"), None);
    assert_eq!(SignalSample::parse_line("nope"), None);
}
