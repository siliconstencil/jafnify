//! The fixed laptop window and its type sizes.
//!
//! The window is the phone home: a 640 x 1000 point layout shown at a fixed
//! zoom of 0.85, so the native window is 544 x 850 points, not resizable. One
//! exception is decided at start-up only: if the monitor's work area is
//! smaller than the window, one smaller zoom is applied once and never
//! changed while running.
//!
//! The text uses the same core tokens as the phone, whole physical pixels at
//! the desktop scale, with a Linux caption floor of 12 px at native 1.0. The
//! Inter Regular/SemiBold and JetBrains Mono faces give the phone's regular
//! and medium weights instead of egui's light default.

use jafnify_core::tokens;

/// The layout box every page is drawn in, in egui points at zoom one.
pub const DESIGN_SIZE: [f32; 2] = [640.0, 1000.0];

/// The whole-window zoom: 85 % of the design box. It is never changed while
/// the window runs.
pub const ZOOM: f32 = 0.85;

/// The fixed native inner size of the window: the design box at [ZOOM].
/// At native 1.0 this is 544 x 850 px; at 1.1 it is about 598 x 935 px.
pub const WINDOW_SIZE: [f32; 2] = [544.0, 850.0];

/// The smallest inner size: the same fixed box.
pub const MIN_SIZE: [f32; 2] = WINDOW_SIZE;

/// The default inner size: the same fixed box.
pub const DEFAULT_SIZE: [f32; 2] = WINDOW_SIZE;

/// The central panel's inner margin on each side, in points.
pub const PANEL_MARGIN: f32 = 8.0;

/// The Linux caption floor at native 1.0. The phone's caption is 11 sp; the
/// laptop standard is at least 12 px.
pub const MIN_CAPTION_PX: f32 = 12.0;

/// The body floor. The body token is 14, so this only guards a tiny zoom.
pub const MIN_BODY_PX: f32 = 12.0;

/// The room reserved for the title bar, the taskbar and window borders when
/// the start-up work area is estimated. egui exposes the monitor size, not the
/// work area; this keeps the one-time fit conservative.
pub const STARTUP_CHROME_PX: f32 = 88.0;

fn finite_or(value: f32, fallback: f32) -> f32 {
    if value.is_finite() {
        value
    } else {
        fallback
    }
}

fn native_or_one(native_ppp: f32) -> f32 {
    if native_ppp.is_finite() && native_ppp >= 0.5 {
        native_ppp
    } else {
        1.0
    }
}

fn valid_ppp(ppp: f32) -> f32 {
    if ppp.is_finite() && ppp >= 0.05 {
        ppp
    } else {
        1.0
    }
}

/// The physical pixel size a text token lands on, at or above `floor_px`, as a
/// whole number of pixels. Snapping the em size to a whole physical pixel is
/// what makes the hinted glyphs sharp.
pub fn text_px(token_points: f32, pixels_per_point: f32, floor_px: f32) -> f32 {
    let ppp = valid_ppp(pixels_per_point);
    let floor = if floor_px.is_finite() {
        floor_px.max(1.0)
    } else {
        1.0
    };
    (token_points * ppp).max(floor).ceil()
}

/// The point size to hand to egui so the text renders at [text_px].
pub fn text_points(token_points: f32, pixels_per_point: f32, floor_px: f32) -> f32 {
    text_px(token_points, pixels_per_point, floor_px) / valid_ppp(pixels_per_point)
}

/// The phone's type tokens at one `pixels_per_point`, whole physical pixels,
/// with the Linux caption floor. The same sizes are used by the home and the
/// pages: caption 12, body 14, body_large 17, title 22, display 34 at native
/// 1.0.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TextSizes {
    pub pixels_per_point: f32,
    pub caption: f32,
    pub body: f32,
    pub body_large: f32,
    pub title: f32,
    pub display: f32,
}

impl TextSizes {
    pub fn new(pixels_per_point: f32) -> Self {
        let ppp = valid_ppp(pixels_per_point);
        Self {
            pixels_per_point: ppp,
            caption: text_points(tokens::TYPE_CAPTION as f32, ppp, MIN_CAPTION_PX),
            body: text_points(tokens::TYPE_BODY as f32, ppp, MIN_BODY_PX),
            body_large: text_points(tokens::TYPE_BODY_LARGE as f32, ppp, MIN_BODY_PX),
            title: text_points(tokens::TYPE_TITLE as f32, ppp, MIN_BODY_PX),
            display: text_points(tokens::TYPE_DISPLAY as f32, ppp, MIN_BODY_PX),
        }
    }

    /// A one-off size (the About mark, a code digit) with the body floor.
    pub fn scale(&self, token_points: f32) -> f32 {
        text_points(token_points, self.pixels_per_point, MIN_BODY_PX)
    }

    /// The physical size a point size lands on.
    pub fn physical(&self, points: f32) -> f32 {
        points * self.pixels_per_point
    }
}

/// The one-time zoom for a monitor whose work area may be smaller than the
/// fixed window. A monitor that fits the window keeps zoom one. The estimate
/// reserves [STARTUP_CHROME_PX] on the height for the title bar and taskbar.
pub fn startup_zoom(monitor_size_points: [f32; 2], native_ppp: f32) -> f32 {
    let native = native_or_one(native_ppp);
    let work_width = finite_or(monitor_size_points[0], DESIGN_SIZE[0]) - 16.0 / native;
    let work_height = finite_or(monitor_size_points[1], DESIGN_SIZE[1]) - STARTUP_CHROME_PX / native;
    if work_width <= 0.0 || work_height <= 0.0 {
        return 0.5;
    }
    (work_width / DESIGN_SIZE[0])
        .min(work_height / DESIGN_SIZE[1])
        .clamp(0.5, ZOOM)
}

/// Clamp a saved outer position so the fixed window opens on the monitor.
/// The position is in the same points as the monitor size.
pub fn clamp_position(
    position: [f32; 2],
    window_size: [f32; 2],
    monitor_size: [f32; 2],
) -> [f32; 2] {
    let max_x = (finite_or(monitor_size[0], 0.0) - finite_or(window_size[0], 0.0)).max(0.0);
    let max_y = (finite_or(monitor_size[1], 0.0) - finite_or(window_size[1], 0.0)).max(0.0);
    [
        finite_or(position[0], 0.0).clamp(0.0, max_x),
        finite_or(position[1], 0.0).clamp(0.0, max_y),
    ]
}

/// The layout width in points. The content fills the panel interior; the
/// fixed window has no band on the right.
pub fn design_width(available_width: f32, _window_width: f32, _native_ppp: f32) -> f32 {
    available_width.max(1.0)
}

/// One row of the fixed-size table: the native scale, the physical window and
/// the smallest home text at that scale.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FixedWindow {
    pub native_ppp: f32,
    pub width: f32,
    pub height: f32,
    pub caption_px: f32,
    pub body_px: f32,
}

/// The fixed window and its text in physical pixels at the native scales the
/// release supports.
pub fn fixed_window_table() -> [FixedWindow; 5] {
    let natives = [1.0_f32, 1.1, 1.25, 1.5, 2.0];
    natives.map(|native| {
        let text = TextSizes::new(native * ZOOM);
        FixedWindow {
            native_ppp: native,
            width: WINDOW_SIZE[0] * native,
            height: WINDOW_SIZE[1] * native,
            caption_px: text.physical(text.caption),
            body_px: text.physical(text.body),
        }
    })
}

/// A window box in native points, which is what the window manager restores
/// on the next open. The box egui reports is in scaled points, so the zoom
/// factor is undone before it is stored. The size is fixed; only the position
/// is read back on the next open.
pub fn native_geometry(
    inner_width: f32,
    inner_height: f32,
    outer: Option<(f32, f32)>,
    zoom: f32,
) -> (f32, f32, Option<(i32, i32)>) {
    let zoom = if zoom > 0.0 { zoom } else { 1.0 };
    let position = outer.map(|(x, y)| ((x * zoom).round() as i32, (y * zoom).round() as i32));
    (inner_width * zoom, inner_height * zoom, position)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_window_is_the_phone_box() {
        assert_eq!(DESIGN_SIZE, [640.0, 1000.0]);
        assert_eq!(ZOOM, 0.85);
        assert_eq!(WINDOW_SIZE, [544.0, 850.0]);
        assert!((DESIGN_SIZE[0] * ZOOM - WINDOW_SIZE[0]).abs() < 0.01);
        assert!((DESIGN_SIZE[1] * ZOOM - WINDOW_SIZE[1]).abs() < 0.01);
        assert_eq!(MIN_SIZE, WINDOW_SIZE);
        assert_eq!(DEFAULT_SIZE, WINDOW_SIZE);
    }

    #[test]
    fn the_text_is_the_phone_tokens_with_the_caption_floor() {
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
    fn a_1200_screen_fits_the_window_at_1_1() {
        let row = fixed_window_table()
            .into_iter()
            .find(|row| row.native_ppp == 1.1)
            .expect("the 1.1 row");
        assert!((row.width - 598.4).abs() < 0.01);
        assert!((row.height - 935.0).abs() < 0.01);
        assert!(row.height <= 1200.0);
    }

    #[test]
    fn the_startup_zoom_only_shrinks_a_small_work_area() {
        assert_eq!(startup_zoom([1600.0, 1400.0], 1.0), ZOOM);
        assert_eq!(startup_zoom([1920.0, 1200.0], 1.1), ZOOM);
        let small = startup_zoom([800.0, 700.0], 1.0);
        assert!(small < ZOOM);
        assert!(small >= 0.5);
    }

    #[test]
    fn the_saved_position_is_clamped_onto_the_screen() {
        assert_eq!(
            clamp_position([-20.0, -40.0], WINDOW_SIZE, [1600.0, 1000.0]),
            [0.0, 0.0]
        );
        assert_eq!(
            clamp_position([2000.0, 2000.0], WINDOW_SIZE, [1600.0, 1000.0]),
            [1056.0, 150.0]
        );
        assert_eq!(
            clamp_position([100.0, 50.0], WINDOW_SIZE, [1600.0, 1400.0]),
            [100.0, 50.0]
        );
    }

    #[test]
    fn the_fixed_table_has_whole_pixel_text() {
        for row in fixed_window_table() {
            assert_eq!(row.caption_px, row.caption_px.ceil());
            assert_eq!(row.body_px, row.body_px.ceil());
            assert!(row.caption_px >= MIN_CAPTION_PX);
            assert!(row.body_px >= MIN_BODY_PX);
            assert_eq!(row.width, WINDOW_SIZE[0] * row.native_ppp);
            assert_eq!(row.height, WINDOW_SIZE[1] * row.native_ppp);
        }
    }

    #[test]
    fn a_saved_geometry_is_stored_in_native_points() {
        let (w, h, pos) = native_geometry(640.0, 1000.0, Some((120.0, 90.0)), 0.75);
        assert!((w - 480.0).abs() < 0.001);
        assert!((h - 750.0).abs() < 0.001);
        assert_eq!(pos, Some((90, 68)));
        let (w, h, _) = native_geometry(640.0, 1000.0, None, 1.0);
        assert_eq!((w, h), (640.0, 1000.0));
    }
}
