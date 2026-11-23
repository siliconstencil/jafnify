use jafnify_core::wifi_leg::{WifiLeg, WifiWindow, MIN_SAMPLES, WINDOW};

fn window_of(samples: &[Option<u64>]) -> WifiWindow {
    let mut window = WifiWindow::new();
    for sample in samples {
        window.push(*sample);
    }
    window
}

#[test]
fn the_window_says_nothing_before_enough_samples() {
    let mut window = WifiWindow::new();
    for _ in 0..MIN_SAMPLES - 1 {
        window.push(Some(20));
    }
    assert!(window.leg().is_none());
}

#[test]
fn the_window_reports_a_leg_once_the_samples_are_enough() {
    let mut window = WifiWindow::new();
    for _ in 0..MIN_SAMPLES {
        window.push(Some(20));
    }
    let leg = window.leg().expect("the window holds enough samples");
    assert_eq!(leg.median_ms, 20);
    assert_eq!(leg.loss_pct, 0);
    assert_eq!(leg.samples, MIN_SAMPLES);
}

#[test]
fn the_window_keeps_only_the_newest_samples() {
    let mut window = WifiWindow::new();
    for _ in 0..WINDOW {
        window.push(Some(10));
    }
    for _ in 0..WINDOW {
        window.push(Some(900));
    }
    assert_eq!(window.len(), WINDOW);
    let leg = window.leg().expect("the window holds enough samples");
    assert_eq!(leg.median_ms, 900);
}

#[test]
fn the_median_of_an_odd_number_of_samples_is_the_middle_one() {
    let window = window_of(&[
        Some(10),
        Some(20),
        Some(30),
        Some(40),
        Some(50),
        Some(60),
        Some(70),
        Some(80),
        Some(90),
        Some(100),
        Some(110),
    ]);
    assert_eq!(window.leg().expect("a leg").median_ms, 60);
}

#[test]
fn the_median_of_an_even_number_of_samples_is_the_average_of_the_two_middle_ones() {
    let window = window_of(&[
        Some(10),
        Some(20),
        Some(30),
        Some(40),
        Some(60),
        Some(70),
        Some(80),
        Some(90),
        Some(100),
        Some(110),
    ]);
    assert_eq!(window.leg().expect("a leg").median_ms, 65);
}

#[test]
fn the_median_of_an_even_number_of_samples_rounds_down() {
    let window = window_of(&[
        Some(1),
        Some(2),
        Some(3),
        Some(4),
        Some(11),
        Some(12),
        Some(13),
        Some(14),
        Some(15),
        Some(16),
    ]);
    assert_eq!(window.leg().expect("a leg").median_ms, 11);
}

#[test]
fn lost_replies_count_as_lost_samples() {
    let window = window_of(&[
        Some(10),
        Some(10),
        Some(10),
        Some(10),
        Some(10),
        Some(10),
        Some(10),
        Some(10),
        None,
        None,
    ]);
    let leg = window.leg().expect("a leg");
    assert_eq!(leg.loss_pct, 20);
    assert_eq!(leg.samples, 10);
}

#[test]
fn a_window_without_a_single_reply_has_no_leg() {
    let window = window_of(&[None, None, None, None, None, None, None, None, None, None]);
    assert!(window.leg().is_none());
}

#[test]
fn clearing_the_window_forgets_every_sample() {
    let mut window = window_of(&[Some(10); 12]);
    window.clear();
    assert_eq!(window.len(), 0);
    assert!(window.leg().is_none());
}

#[test]
fn the_leg_line_names_the_median_the_loss_and_the_sample_count() {
    let leg = WifiLeg::new(42, 3, 30);
    assert_eq!(
        leg.line(),
        "Wi-Fi to phone: 42 ms, 3% lost (last 30 checks)"
    );
}

#[test]
fn the_leg_becomes_the_pc_leg_of_the_verdict() {
    let leg = WifiLeg::new(42, 3, 30);
    let link = leg.link_leg();
    assert_eq!(link.avg_ms, 42);
    assert_eq!(link.loss_pct, 3);
}
