//! The desktop notifier: one line per transition, off the daemon loop.
//!
//! The daemon hands a line over and returns immediately; a worker thread makes
//! the session bus call. The bus is behind the [`Bus`] trait so the tests use
//! a fake and never touch a real session bus. The worker keeps the id the
//! notification server returned for the heat line, so a phone that cooled down
//! can have its popup withdrawn by that id.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender};
use std::sync::{Arc, Mutex};
use std::thread;

use crate::flavor;

/// The app name the desktop shows.
pub const APP_NAME: &str = "Jafnify";

/// How long the desktop keeps the line.
pub const EXPIRE_MS: i32 = 5_000;

/// How many lines may wait for the bus.
const QUEUE: usize = 8;

/// Which line of the worker keeps its popup id for a later withdrawal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Track {
    /// The heat line, withdrawn when the phone cools down.
    Heat,
    /// The lost-connection line, withdrawn when the outage turns out to be
    /// an app update.
    Lost,
    /// Every other line: nothing to withdraw.
    Plain,
}

/// One job for the notifier worker.
enum Job {
    /// Show a line. `track` marks the line whose popup is withdrawn later.
    Show {
        title: String,
        body: String,
        track: Track,
    },
    /// Withdraw the last heat popup, if it is still on screen.
    WithdrawHeat,
    /// Withdraw the last lost-connection popup, if it is still on screen.
    WithdrawLost,
}

/// The bus the notifier talks to.
pub trait Bus: Send + 'static {
    /// Show one line with an optional body and return the id the server gave
    /// it. An error means the bus or the daemon is gone.
    fn notify(&self, title: &str, body: &str) -> Result<u32, String>;
    /// Withdraw the popup of this id.
    fn close(&self, id: u32) -> Result<(), String>;
}

/// The session bus, `org.freedesktop.Notifications`.
pub struct SessionBus {
    connection: zbus::blocking::Connection,
}

impl SessionBus {
    /// Connect to the session bus.
    pub fn connect() -> Result<Self, String> {
        zbus::blocking::Connection::session()
            .map(|connection| SessionBus { connection })
            .map_err(|e| e.to_string())
    }

    fn call(&self, member: &str, body: &(impl serde::Serialize + zbus::zvariant::DynamicType)) -> Result<u32, String> {
        let reply = self
            .connection
            .call_method(
                Some("org.freedesktop.Notifications"),
                "/org/freedesktop/Notifications",
                Some("org.freedesktop.Notifications"),
                member,
                body,
            )
            .map_err(|e| e.to_string())?;
        reply.body().deserialize::<u32>().map_err(|e| e.to_string())
    }
}

impl Bus for SessionBus {
    fn notify(&self, title: &str, body: &str) -> Result<u32, String> {
        let actions: Vec<&str> = Vec::new();
        let hints: HashMap<&str, zbus::zvariant::Value<'_>> = HashMap::new();
        self.call(
            "Notify",
            &(
                APP_NAME,
                0u32,
                flavor::NAME,
                title,
                body,
                &actions,
                hints,
                EXPIRE_MS,
            ),
        )
    }

    fn close(&self, id: u32) -> Result<(), String> {
        self.call("CloseNotification", &(id,)).map(|_| ())
    }
}

/// Something that can show a line on the desktop.
pub trait Notifier: Send + Sync {
    /// Show one line with an optional body. It must never block the caller.
    fn notify(&self, title: &str, body: &str);

    /// Show the heat line: the worker keeps its id so it can be withdrawn.
    fn notify_heat(&self, title: &str, body: &str) {
        self.notify(title, body);
    }

    /// Withdraw the heat popup when the phone cooled down.
    fn withdraw_heat(&self) {}

    /// Show the lost-connection line: the worker keeps its id so it can be
    /// withdrawn when the outage turns out to be an app update.
    fn notify_lost(&self, title: &str, body: &str) {
        self.notify(title, body);
    }

    /// Withdraw the lost-connection popup when the phone reports an update.
    fn withdraw_lost(&self) {}

    /// True when this notifier has no bus and can never reach one. A unit
    /// test must never put a line on the operator's desktop.
    fn is_silent(&self) -> bool {
        false
    }
}

/// The desktop notifier. The daemon hands the line over and returns; a worker
/// thread makes the bus call.
pub struct DesktopNotifier {
    sender: Option<SyncSender<Job>>,
    dropped: AtomicUsize,
}

impl DesktopNotifier {
    /// Start a worker on the session bus. A missing bus is logged once and the
    /// notifier stays silent.
    ///
    /// The bus connection is made on the worker thread, so a slow or hung
    /// session bus never holds up the daemon loop.
    pub fn start() -> Self {
        Self::spawn(|| match SessionBus::connect() {
            Ok(bus) => Some(bus),
            Err(e) => {
                eprintln!("jafnify: no session bus for desktop notifications: {e}");
                None
            }
        })
    }

    /// Start a worker on any bus. The tests use a fake bus.
    pub fn with_bus<B: Bus>(bus: B) -> Self {
        Self::spawn(move || Some(bus))
    }

    /// Start the worker; `open` runs on the worker thread. With no bus the
    /// worker drains the queue silently.
    fn spawn<B: Bus, F: FnOnce() -> Option<B> + Send + 'static>(open: F) -> Self {
        let (sender, receiver) = sync_channel(QUEUE);
        match thread::Builder::new()
            .name("jafnify-notify".to_string())
            .spawn(move || match open() {
                Some(bus) => worker(bus, receiver),
                None => while receiver.recv().is_ok() {},
            }) {
            Ok(_) => DesktopNotifier {
                sender: Some(sender),
                dropped: AtomicUsize::new(0),
            },
            Err(e) => {
                eprintln!("jafnify: cannot start the notification worker: {e}");
                Self::silent()
            }
        }
    }

    /// A notifier with no bus at all: every line is dropped.
    pub fn silent() -> Self {
        DesktopNotifier {
            sender: None,
            dropped: AtomicUsize::new(0),
        }
    }

    /// How many lines were dropped: no bus, or the queue was full.
    pub fn dropped(&self) -> usize {
        self.dropped.load(Ordering::SeqCst)
    }

    fn enqueue(&self, title: &str, body: &str, track: Track) {
        let Some(sender) = &self.sender else {
            self.dropped.fetch_add(1, Ordering::SeqCst);
            return;
        };
        let job = Job::Show {
            title: title.to_string(),
            body: body.to_string(),
            track,
        };
        if sender.try_send(job).is_err() {
            self.dropped.fetch_add(1, Ordering::SeqCst);
        }
    }
}

impl Notifier for DesktopNotifier {
    fn notify(&self, title: &str, body: &str) {
        self.enqueue(title, body, Track::Plain);
    }

    fn notify_heat(&self, title: &str, body: &str) {
        self.enqueue(title, body, Track::Heat);
    }

    fn withdraw_heat(&self) {
        if let Some(sender) = &self.sender {
            // A full queue must not block the daemon; a missed withdrawal is
            // better than a stalled state loop.
            let _ = sender.try_send(Job::WithdrawHeat);
        }
    }

    fn notify_lost(&self, title: &str, body: &str) {
        self.enqueue(title, body, Track::Lost);
    }

    fn withdraw_lost(&self) {
        if let Some(sender) = &self.sender {
            let _ = sender.try_send(Job::WithdrawLost);
        }
    }

    fn is_silent(&self) -> bool {
        self.sender.is_none()
    }
}

/// The worker: one bus call per line, off the daemon thread. The first bus
/// failure is logged; later failures are silent, but every line is still
/// tried, so a restarted notification daemon (for example plasmashell) works
/// again without restarting Jafnify.
fn worker<B: Bus>(bus: B, receiver: Receiver<Job>) {
    let mut logged = false;
    let mut heat_id: Option<u32> = None;
    let mut lost_id: Option<u32> = None;
    while let Ok(job) = receiver.recv() {
        match job {
            Job::Show { title, body, track } => match bus.notify(&title, &body) {
                Ok(id) => match track {
                    Track::Heat => heat_id = Some(id),
                    Track::Lost => lost_id = Some(id),
                    Track::Plain => {}
                },
                Err(e) => {
                    if !logged {
                        eprintln!("jafnify: the desktop notification failed: {e}");
                        logged = true;
                    }
                }
            },
            Job::WithdrawHeat => {
                if let Some(id) = heat_id.take() {
                    if let Err(e) = bus.close(id) {
                        if !logged {
                            eprintln!("jafnify: withdrawing the heat popup failed: {e}");
                            logged = true;
                        }
                    }
                }
            }
            Job::WithdrawLost => {
                if let Some(id) = lost_id.take() {
                    if let Err(e) = bus.close(id) {
                        if !logged {
                            eprintln!("jafnify: withdrawing the lost popup failed: {e}");
                            logged = true;
                        }
                    }
                }
            }
        }
    }
}

/// A notifier that records the lines instead of showing them.
#[derive(Debug, Default)]
pub struct FakeNotifier {
    lines: Arc<Mutex<Vec<String>>>,
    bodies: Arc<Mutex<Vec<String>>>,
    withdrawals: Arc<AtomicUsize>,
    lost_withdrawals: Arc<AtomicUsize>,
}

impl FakeNotifier {
    pub fn new() -> Self {
        Self::default()
    }

    /// The lines recorded so far.
    pub fn lines(&self) -> Vec<String> {
        self.lines
            .lock()
            .map(|lines| lines.clone())
            .unwrap_or_default()
    }

    /// The bodies recorded so far, in the same order as the lines.
    pub fn bodies(&self) -> Vec<String> {
        self.bodies
            .lock()
            .map(|bodies| bodies.clone())
            .unwrap_or_default()
    }

    /// The same list, shared; a test can read it after the notifier moved.
    pub fn shared(&self) -> Arc<Mutex<Vec<String>>> {
        Arc::clone(&self.lines)
    }

    /// The bodies, shared; a test can read them after the notifier moved.
    pub fn bodies_shared(&self) -> Arc<Mutex<Vec<String>>> {
        Arc::clone(&self.bodies)
    }

    /// How many heat popups were withdrawn.
    pub fn withdrawals(&self) -> usize {
        self.withdrawals.load(Ordering::SeqCst)
    }

    /// The withdrawal count, shared; a test reads it after the notifier moved.
    pub fn withdrawals_shared(&self) -> Arc<AtomicUsize> {
        Arc::clone(&self.withdrawals)
    }

    /// How many lost-connection popups were withdrawn.
    pub fn lost_withdrawals(&self) -> usize {
        self.lost_withdrawals.load(Ordering::SeqCst)
    }
}

impl Notifier for FakeNotifier {
    fn notify(&self, title: &str, body: &str) {
        if let Ok(mut lines) = self.lines.lock() {
            lines.push(title.to_string());
        }
        if let Ok(mut bodies) = self.bodies.lock() {
            bodies.push(body.to_string());
        }
    }

    fn withdraw_heat(&self) {
        self.withdrawals.fetch_add(1, Ordering::SeqCst);
    }

    fn notify_lost(&self, title: &str, body: &str) {
        self.notify(title, body);
    }

    fn withdraw_lost(&self) {
        self.lost_withdrawals.fetch_add(1, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU32;

    /// A bus that hands out ids and records the Notify and CloseNotification
    /// calls.
    struct FakeBus {
        next_id: AtomicU32,
        notified: Arc<Mutex<Vec<(String, u32)>>>,
        closed: Arc<Mutex<Vec<u32>>>,
    }

    impl FakeBus {
        fn new() -> Self {
            FakeBus {
                next_id: AtomicU32::new(41),
                notified: Arc::new(Mutex::new(Vec::new())),
                closed: Arc::new(Mutex::new(Vec::new())),
            }
        }
    }

    impl Bus for FakeBus {
        fn notify(&self, title: &str, _body: &str) -> Result<u32, String> {
            let id = self.next_id.fetch_add(1, Ordering::SeqCst);
            self.notified
                .lock()
                .unwrap()
                .push((title.to_string(), id));
            Ok(id)
        }

        fn close(&self, id: u32) -> Result<(), String> {
            self.closed.lock().unwrap().push(id);
            Ok(())
        }
    }

    fn wait_for(closed: &Arc<Mutex<Vec<u32>>>) -> Vec<u32> {
        for _ in 0..200 {
            let ids = closed.lock().unwrap().clone();
            if !ids.is_empty() {
                return ids;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        closed.lock().unwrap().clone()
    }

    #[test]
    fn the_id_of_the_heat_line_closes_that_popup() {
        let bus = FakeBus::new();
        let closed = Arc::clone(&bus.closed);
        let notified = Arc::clone(&bus.notified);
        let notifier = DesktopNotifier::with_bus(bus);
        notifier.notify_heat("Phone is hot (43.1 C)", "help");
        notifier.withdraw_heat();
        assert_eq!(wait_for(&closed), vec![41], "{notified:?}");
    }

    #[test]
    fn the_id_of_the_lost_line_closes_that_popup() {
        let bus = FakeBus::new();
        let closed = Arc::clone(&bus.closed);
        let notified = Arc::clone(&bus.notified);
        let notifier = DesktopNotifier::with_bus(bus);
        notifier.notify_lost("Jafnify lost the connection.", "");
        notifier.withdraw_lost();
        assert_eq!(wait_for(&closed), vec![41], "{notified:?}");
    }

    #[test]
    fn a_plain_line_is_never_withdrawn() {
        let bus = FakeBus::new();
        let closed = Arc::clone(&bus.closed);
        let notifier = DesktopNotifier::with_bus(bus);
        notifier.notify("Jafnify connected.", "");
        notifier.withdraw_heat();
        // Give the worker a moment to misbehave, then assert nothing closed.
        std::thread::sleep(std::time::Duration::from_millis(50));
        assert!(closed.lock().unwrap().is_empty());
    }

    #[test]
    fn a_silent_notifier_has_no_bus() {
        assert!(DesktopNotifier::silent().is_silent());
        let silent = DesktopNotifier::silent();
        silent.notify("Phone is hot (43.1 C)", "");
        assert_eq!(silent.dropped(), 1, "nothing may reach a bus");
    }

    #[test]
    fn the_fake_records_the_withdrawal() {
        let fake = FakeNotifier::new();
        fake.notify_heat("Phone is hot (43.1 C)", "");
        fake.withdraw_heat();
        assert_eq!(fake.lines(), vec!["Phone is hot (43.1 C)".to_string()]);
        assert_eq!(fake.withdrawals(), 1);
    }
}
