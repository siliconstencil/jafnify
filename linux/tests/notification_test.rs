//! The desktop notifier: the daemon hands over a line, a worker thread makes
//! the bus call. No test touches a real session bus.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use jafnify_linux::notification::{Bus, DesktopNotifier, FakeNotifier, Notifier};

/// A bus that records the lines and can be told to fail or to be slow.
struct FakeBus {
    lines: Mutex<Vec<String>>,
    bodies: Mutex<Vec<String>>,
    calls: AtomicUsize,
    /// The ids the bus handed out, and the ids it was told to close.
    next_id: AtomicUsize,
    closed: Mutex<Vec<u32>>,
    fail: bool,
    slow: bool,
}

impl FakeBus {
    fn new() -> Arc<FakeBus> {
        Arc::new(FakeBus {
            lines: Mutex::new(Vec::new()),
            bodies: Mutex::new(Vec::new()),
            calls: AtomicUsize::new(0),
            next_id: AtomicUsize::new(7),
            closed: Mutex::new(Vec::new()),
            fail: false,
            slow: false,
        })
    }

    fn failing() -> Arc<FakeBus> {
        Arc::new(FakeBus {
            lines: Mutex::new(Vec::new()),
            bodies: Mutex::new(Vec::new()),
            calls: AtomicUsize::new(0),
            next_id: AtomicUsize::new(7),
            closed: Mutex::new(Vec::new()),
            fail: true,
            slow: false,
        })
    }

    fn slow() -> Arc<FakeBus> {
        Arc::new(FakeBus {
            lines: Mutex::new(Vec::new()),
            bodies: Mutex::new(Vec::new()),
            calls: AtomicUsize::new(0),
            next_id: AtomicUsize::new(7),
            closed: Mutex::new(Vec::new()),
            fail: false,
            slow: true,
        })
    }

    fn lines(&self) -> Vec<String> {
        self.lines
            .lock()
            .map(|lines| lines.clone())
            .unwrap_or_default()
    }

    fn bodies(&self) -> Vec<String> {
        self.bodies
            .lock()
            .map(|bodies| bodies.clone())
            .unwrap_or_default()
    }
}

impl Bus for FakeBus {
    fn notify(&self, title: &str, body: &str) -> Result<u32, String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.slow {
            thread::sleep(Duration::from_millis(200));
        }
        if self.fail {
            return Err("no notification daemon".to_string());
        }
        if let Ok(mut lines) = self.lines.lock() {
            lines.push(title.to_string());
        }
        if let Ok(mut bodies) = self.bodies.lock() {
            bodies.push(body.to_string());
        }
        Ok(self.next_id.fetch_add(1, Ordering::SeqCst) as u32)
    }

    fn close(&self, id: u32) -> Result<(), String> {
        self.closed.lock().unwrap().push(id);
        Ok(())
    }
}

/// The fake bus behind a shared handle.
struct SharedBus(Arc<FakeBus>);

impl Bus for SharedBus {
    fn notify(&self, title: &str, body: &str) -> Result<u32, String> {
        self.0.notify(title, body)
    }

    fn close(&self, id: u32) -> Result<(), String> {
        self.0.close(id)
    }
}

/// Wait for the worker to deliver, up to two seconds.
fn wait_for(mut condition: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        if condition() {
            return true;
        }
        thread::sleep(Duration::from_millis(10));
    }
    condition()
}

#[test]
fn the_fake_notifier_records_the_lines_in_order() {
    let notifier = FakeNotifier::new();
    notifier.notify("Jafnify connected.", "");
    notifier.notify("Jafnify lost the connection.", "");
    assert_eq!(
        notifier.lines(),
        vec![
            "Jafnify connected.".to_string(),
            "Jafnify lost the connection.".to_string()
        ]
    );
}

#[test]
fn the_fake_notifier_records_the_bodies_next_to_the_lines() {
    let notifier = FakeNotifier::new();
    notifier.notify("Phone is hot (43.1 C)", "Move the phone out of the sun.");
    notifier.notify("Jafnify connected.", "");
    assert_eq!(
        notifier.bodies(),
        vec!["Move the phone out of the sun.".to_string(), String::new()]
    );
}

#[test]
fn the_desktop_notifier_shows_the_line_on_the_bus() {
    let bus = FakeBus::new();
    let notifier = DesktopNotifier::with_bus(SharedBus(bus.clone()));

    notifier.notify("Jafnify connected.", "");

    assert!(
        wait_for(|| bus.lines().len() == 1),
        "the worker did not deliver the line"
    );
    assert_eq!(bus.lines(), vec!["Jafnify connected.".to_string()]);
}

#[test]
fn the_desktop_notifier_carries_the_title_and_the_body() {
    let bus = FakeBus::new();
    let notifier = DesktopNotifier::with_bus(SharedBus(bus.clone()));

    notifier.notify(
        "Phone is hot (43.1 C)",
        "Move the phone out of the sun; unplug the charger if it is charging.",
    );

    assert!(
        wait_for(|| bus.lines().len() == 1),
        "the worker did not deliver the line"
    );
    assert_eq!(bus.lines(), vec!["Phone is hot (43.1 C)".to_string()]);
    assert_eq!(
        bus.bodies(),
        vec!["Move the phone out of the sun; unplug the charger if it is charging.".to_string()]
    );
}

#[test]
fn the_caller_never_waits_for_the_bus() {
    let bus = FakeBus::slow();
    let notifier = DesktopNotifier::with_bus(SharedBus(bus.clone()));

    let start = Instant::now();
    notifier.notify("Jafnify connected.", "");
    let elapsed = start.elapsed();

    assert!(
        elapsed < Duration::from_millis(100),
        "the daemon loop waited {elapsed:?} for the bus"
    );
    assert!(wait_for(|| bus.lines().len() == 1));
}

#[test]
fn a_failing_bus_is_still_tried_for_the_next_line() {
    // A notification daemon that restarts (for example plasmashell) must work
    // again without restarting Jafnify; only the first failure is logged.
    let bus = FakeBus::failing();
    let notifier = DesktopNotifier::with_bus(SharedBus(bus.clone()));

    notifier.notify("Jafnify connected.", "");
    assert!(wait_for(|| bus.calls.load(Ordering::SeqCst) >= 1));
    notifier.notify("Jafnify lost the connection.", "");

    assert!(
        wait_for(|| bus.calls.load(Ordering::SeqCst) == 2),
        "the second line was not tried"
    );
}

#[test]
fn a_silent_notifier_drops_the_line_without_a_bus() {
    let notifier = DesktopNotifier::silent();
    notifier.notify("Jafnify connected.", "");
    assert_eq!(notifier.dropped(), 1);
    assert!(notifier.is_silent());
}

#[test]
fn the_heat_popup_is_closed_by_the_id_the_bus_returned() {
    let bus = FakeBus::new();
    let notifier = DesktopNotifier::with_bus(SharedBus(bus.clone()));
    notifier.notify_heat("Phone is hot (43.1 C)", "help");
    assert!(wait_for(|| bus.lines().len() == 1));
    notifier.withdraw_heat();
    assert!(wait_for(|| !bus.closed.lock().unwrap().is_empty()));
    assert_eq!(*bus.closed.lock().unwrap(), vec![7]);
}
