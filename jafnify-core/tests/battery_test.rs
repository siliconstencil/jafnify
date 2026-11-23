use jafnify_core::battery::{level_line, lines, temperature_line};

#[test]
fn a_normal_level_names_the_percentage() {
    assert_eq!(level_line(Some(80)).as_deref(), Some("Phone battery: 80%."));
    assert_eq!(level_line(Some(21)).as_deref(), Some("Phone battery: 21%."));
}

#[test]
fn twenty_percent_and_below_is_low() {
    assert_eq!(
        level_line(Some(20)).as_deref(),
        Some("Phone battery is low: 20%.")
    );
    assert_eq!(
        level_line(Some(18)).as_deref(),
        Some("Phone battery is low: 18%.")
    );
    assert_eq!(
        level_line(Some(0)).as_deref(),
        Some("Phone battery is low: 0%.")
    );
}

#[test]
fn an_unknown_level_has_no_line() {
    assert_eq!(level_line(None), None);
    assert_eq!(level_line(Some(255)), None);
}

#[test]
fn a_normal_temperature_names_the_value() {
    assert_eq!(
        temperature_line(Some(315)).as_deref(),
        Some("Phone temperature: 31.5 C.")
    );
    assert_eq!(
        temperature_line(Some(399)).as_deref(),
        Some("Phone temperature: 39.9 C.")
    );
}

#[test]
fn forty_degrees_is_hot_and_forty_five_is_critical() {
    assert_eq!(
        temperature_line(Some(400)).as_deref(),
        Some("Phone is hot: 40 C.")
    );
    assert_eq!(
        temperature_line(Some(410)).as_deref(),
        Some("Phone is hot: 41 C.")
    );
    assert_eq!(
        temperature_line(Some(415)).as_deref(),
        Some("Phone is hot: 41.5 C.")
    );
    assert_eq!(
        temperature_line(Some(449)).as_deref(),
        Some("Phone is hot: 44.9 C.")
    );
    assert_eq!(
        temperature_line(Some(450)).as_deref(),
        Some("Phone temperature: 45 C.")
    );
    assert_eq!(
        temperature_line(Some(460)).as_deref(),
        Some("Phone temperature: 46 C.")
    );
}

#[test]
fn an_unknown_temperature_has_no_line() {
    assert_eq!(temperature_line(None), None);
    assert_eq!(temperature_line(Some(i16::MIN)), None);
}

#[test]
fn the_two_lines_are_emitted_in_a_fixed_order() {
    assert_eq!(
        lines(Some(80), Some(315)),
        vec![
            "Phone battery: 80%.".to_string(),
            "Phone temperature: 31.5 C.".to_string(),
        ]
    );
    assert_eq!(lines(None, None), Vec::<String>::new());
}
