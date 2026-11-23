//! One fixed window, the 640 x 1000 design box at zoom 0.85, the phone's type
//! tokens with a 12 px caption floor, and a one-time start-up fit for a small
//! work area. These tests pin the contract before the window draws it.

use jafnify_linux::scale::{
    self, TextSizes, DEFAULT_SIZE, DESIGN_SIZE, MIN_CAPTION_PX, MIN_SIZE, WINDOW_SIZE, ZOOM,
};

#[test]
fn the_window_is_fixed_at_the_phone_size() {
    assert_eq!(DESIGN_SIZE, [640.0, 1000.0]);
    assert_eq!(ZOOM, 0.85);
    assert_eq!(WINDOW_SIZE, [544.0, 850.0]);
    assert_eq!(MIN_SIZE, WINDOW_SIZE);
    assert_eq!(DEFAULT_SIZE, WINDOW_SIZE);
    assert_eq!(scale::design_width(624.0, 640.0, 1.0), 624.0);
}

#[test]
fn the_text_is_the_phone_tokens_at_native_one() {
    let text = TextSizes::new(1.0);
    assert_eq!(text.physical(text.caption), 12.0);
    assert_eq!(text.physical(text.body), 14.0);
    assert_eq!(text.physical(text.body_large), 17.0);
    assert_eq!(text.physical(text.title), 22.0);
    assert_eq!(text.physical(text.display), 34.0);
    for size in [
        text.caption,
        text.body,
        text.body_large,
        text.title,
        text.display,
    ] {
        assert_eq!(text.physical(size), text.physical(size).ceil());
    }
}

#[test]
fn the_caption_floor_holds_at_every_native_scale() {
    for native in [1.0_f32, 1.1, 1.25, 1.5, 2.0] {
        let text = TextSizes::new(native);
        assert!(text.physical(text.caption) >= MIN_CAPTION_PX);
        assert!(text.physical(text.body) >= scale::MIN_BODY_PX);
    }
}

#[test]
fn the_telephone_scale_matches_the_core_tokens() {
    assert_eq!(scale::MIN_CAPTION_PX, 12.0);
    assert_eq!(
        TextSizes::new(1.0).physical(TextSizes::new(1.0).title),
        22.0
    );
}

#[test]
fn the_fixed_table_covers_the_supported_native_scales() {
    let table = scale::fixed_window_table();
    assert_eq!(table.len(), 5);
    for row in table {
        assert_eq!(row.width, WINDOW_SIZE[0] * row.native_ppp);
        assert_eq!(row.height, WINDOW_SIZE[1] * row.native_ppp);
        assert!(row.caption_px >= MIN_CAPTION_PX);
        assert_eq!(row.caption_px, row.caption_px.ceil());
    }
}

#[test]
fn the_1_1_window_fits_a_1200_screen() {
    let row = scale::fixed_window_table()
        .into_iter()
        .find(|row| row.native_ppp == 1.1)
        .expect("the 1.1 row");
    assert!((row.width - 598.4).abs() < 0.01);
    assert!((row.height - 935.0).abs() < 0.01);
    assert!(row.height <= 1200.0);
}

#[test]
fn the_startup_zoom_is_the_fixed_zoom_on_a_normal_monitor() {
    assert_eq!(scale::startup_zoom([1600.0, 1400.0], 1.0), ZOOM);
    assert_eq!(scale::startup_zoom([1920.0, 1200.0], 1.1), ZOOM);
}

#[test]
fn the_startup_zoom_shrinks_once_on_a_small_work_area() {
    let zoom = scale::startup_zoom([800.0, 700.0], 1.0);
    assert!(zoom < ZOOM);
    assert!(zoom >= 0.5);
    assert_eq!(scale::startup_zoom([200.0, 200.0], 1.0), 0.5);
}

#[test]
fn the_saved_position_is_clamped_onto_the_screen() {
    assert_eq!(
        scale::clamp_position([-10.0, -10.0], WINDOW_SIZE, [1600.0, 1000.0]),
        [0.0, 0.0]
    );
    assert_eq!(
        scale::clamp_position([9999.0, 9999.0], WINDOW_SIZE, [1600.0, 1000.0]),
        [1056.0, 150.0]
    );
    assert_eq!(
        scale::clamp_position([100.0, 20.0], WINDOW_SIZE, [1600.0, 1200.0]),
        [100.0, 20.0]
    );
}

#[test]
fn the_text_never_shrinks_below_the_floor() {
    let tiny = TextSizes::new(0.5);
    assert!(tiny.physical(tiny.caption) >= MIN_CAPTION_PX);
    assert!(tiny.physical(tiny.body) >= scale::MIN_BODY_PX);
}
