//! The four code boxes of the GUI as plain state: which digit goes where and
//! where the focus moves next. egui only draws and forwards events.

use jafnify_linux::code_entry::CodeBoxes;

#[test]
fn a_typed_digit_fills_its_box_and_moves_on() {
    let mut b = CodeBoxes::default();
    assert_eq!(b.input(0, "4"), Some(1));
    assert_eq!(b.input(1, "3"), Some(2));
    assert_eq!(b.input(2, "2"), Some(3));
    assert_eq!(b.input(3, "1"), None, "the last box has nowhere to go");
    assert_eq!(b.code(), "4321");
    assert!(b.is_complete());
}

#[test]
fn anything_but_a_digit_is_dropped() {
    let mut b = CodeBoxes::default();
    assert_eq!(b.input(0, "a"), None);
    assert_eq!(b.text(0), "");
    assert_eq!(b.input(0, "-"), None);
    assert_eq!(b.code(), "");
}

#[test]
fn typing_into_a_filled_box_replaces_its_digit() {
    let mut b = CodeBoxes::default();
    b.input(0, "1");
    // The box held "1" and the user typed "2": the field now reads "12".
    assert_eq!(b.input(0, "12"), Some(1));
    assert_eq!(b.text(0), "2");
}

#[test]
fn an_emptied_box_stays_empty() {
    let mut b = CodeBoxes::default();
    b.input(0, "7");
    assert_eq!(b.input(0, ""), None);
    assert_eq!(b.text(0), "");
}

#[test]
fn backspace_in_an_empty_box_clears_and_focuses_the_previous_one() {
    let mut b = CodeBoxes::default();
    b.input(0, "1");
    b.input(1, "2");
    assert_eq!(b.backspace(2), Some(1));
    assert_eq!(b.text(1), "");
    assert_eq!(b.text(0), "1");
}

#[test]
fn backspace_in_a_filled_box_or_the_first_box_moves_nothing() {
    let mut b = CodeBoxes::default();
    b.input(0, "1");
    assert_eq!(b.backspace(0), None);
    assert_eq!(
        b.backspace(1),
        Some(0),
        "box 1 is empty, so box 0 is cleared"
    );
    assert_eq!(b.text(0), "");
    b.input(2, "5");
    assert_eq!(
        b.backspace(2),
        None,
        "a filled box is emptied by the field itself"
    );
}

#[test]
fn pasting_four_digits_fills_every_box() {
    let mut b = CodeBoxes::default();
    assert_eq!(b.paste("4321"), Some(3));
    assert_eq!(b.code(), "4321");
    assert!(b.is_complete());
}

#[test]
fn pasting_keeps_only_the_first_four_digits_and_skips_other_characters() {
    let mut b = CodeBoxes::default();
    b.paste("code: 4 3 2 1 9");
    assert_eq!(b.code(), "4321");
}

#[test]
fn a_short_paste_leaves_the_focus_on_the_next_empty_box() {
    let mut b = CodeBoxes::default();
    assert_eq!(b.paste("12"), Some(2));
    assert_eq!(b.code(), "12");
    assert!(!b.is_complete());
    assert_eq!(b.paste("no digits"), None);
    assert_eq!(b.code(), "12", "a paste without digits changes nothing");
}

#[test]
fn clear_empties_all_boxes() {
    let mut b = CodeBoxes::default();
    b.paste("4321");
    b.clear();
    assert_eq!(b.code(), "");
    assert_eq!(b.len(), 0);
}
