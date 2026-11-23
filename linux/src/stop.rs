//! One process-wide stop flag shared by the headless loop and the tray daemon.
//!
//! SIGTERM/SIGINT/SIGHUP only set the flag; the loop removes the dead tunnel's
//! routing and exits in order, so a service stop never leaves the laptop
//! offline because of jafn0. SIGHUP is included so closing the terminal of a
//! `jafnify join` ends the attempt the same way Ctrl-C does.

use std::sync::atomic::{AtomicBool, Ordering};

static STOP_REQUESTED: AtomicBool = AtomicBool::new(false);

extern "C" fn request_stop(_signal: libc::c_int) {
    STOP_REQUESTED.store(true, Ordering::SeqCst);
}

pub fn install() {
    let handler: extern "C" fn(libc::c_int) = request_stop;
    unsafe {
        libc::signal(libc::SIGTERM, handler as libc::sighandler_t);
        libc::signal(libc::SIGINT, handler as libc::sighandler_t);
        libc::signal(libc::SIGHUP, handler as libc::sighandler_t);
    }
}

pub fn requested() -> bool {
    STOP_REQUESTED.load(Ordering::SeqCst)
}

/// What a stop request does with the tunnel. A Connected session keeps jafn0
/// for a fast return (the tray may adopt it), so only its routing is removed.
/// An attempt that never reached Connected ends with its tunnel: a join closed
/// while "Connecting" must not leave an orphan jafn0 that the tray then calls
/// another Jafnify connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShutdownAction {
    FallBack,
    TearDown,
}

pub fn shutdown_action(connected: bool) -> ShutdownAction {
    if connected {
        ShutdownAction::FallBack
    } else {
        ShutdownAction::TearDown
    }
}

#[cfg(test)]
mod tests {
    use super::{shutdown_action, ShutdownAction};

    #[test]
    fn an_interrupted_attempt_tears_its_tunnel_down() {
        assert_eq!(shutdown_action(false), ShutdownAction::TearDown);
    }

    #[test]
    fn a_connected_session_keeps_the_tunnel_and_only_falls_back() {
        assert_eq!(shutdown_action(true), ShutdownAction::FallBack);
    }
}
