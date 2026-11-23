//! One-line notices shown around sharing, in one place.
//!
//! The phone draws them, but the words live here so the phone and the laptop
//! say the same thing, and so a test can hold the wording still. Nothing here
//! is a fault claim: the exit reasons carry "likely" where they are a guess.

use crate::phone_exit::ExitReason;

/// The phone shared with notifications off: the notification's Stop action is
/// then not in the shade, so the user must stop from the app.
pub const NOTIFICATIONS_OFF: &str = "Notifications are off: stop sharing here";

/// The phone app was closed while sharing, by the app itself (a crash or an
/// ANR). The laptop says "likely the phone app stopped" for the same reason.
pub const STOPPED_APP: &str = "Sharing stopped: likely the phone app stopped";

/// The phone app was closed while sharing, by Android (memory or a kill).
pub const STOPPED_ANDROID: &str = "Sharing stopped: likely Android closed the phone app";

/// The phone app ended while sharing for a reason this build cannot name.
pub const STOPPED_OTHER: &str = "Sharing stopped: the phone app ended";

/// The sharer loop died (a Rust panic): sharing really stopped, so the card
/// asks the user to start it again instead of showing a stale "Connected".
pub const WORKER_STOPPED: &str = "Sharing stopped: start sharing again";

/// A second `jafnify tray`: the one window and the tray already run.
pub const ALREADY_IN_TRAY: &str = "Jafnify is already running in the tray.";

/// The About row and the notices screen title.
pub const THIRD_PARTY_NOTICES: &str = "Third-party notices";

/// The line under the phone's status on the first open after the system
/// closed the app while sharing, or "" when there is nothing to say.
///
/// A user stop and an app update are not shown: the user did the first and
/// knows the second. Anything else that ended the app is a stop the user did
/// not ask for, and is named with the same "likely" as the laptop's hints.
pub fn stopped_line(reason: Option<ExitReason>) -> &'static str {
    match reason {
        Some(ExitReason::Crash) | Some(ExitReason::Anr) => STOPPED_APP,
        Some(ExitReason::LowMemory) | Some(ExitReason::Killed) => STOPPED_ANDROID,
        Some(ExitReason::Other) => STOPPED_OTHER,
        Some(ExitReason::Updated) | Some(ExitReason::User) | None => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_system_stop_gets_one_line_and_a_user_stop_gets_none() {
        assert_eq!(
            stopped_line(Some(ExitReason::Crash)),
            "Sharing stopped: likely the phone app stopped"
        );
        assert_eq!(
            stopped_line(Some(ExitReason::Anr)),
            "Sharing stopped: likely the phone app stopped"
        );
        assert_eq!(
            stopped_line(Some(ExitReason::LowMemory)),
            "Sharing stopped: likely Android closed the phone app"
        );
        assert_eq!(
            stopped_line(Some(ExitReason::Killed)),
            "Sharing stopped: likely Android closed the phone app"
        );
        assert_eq!(stopped_line(Some(ExitReason::User)), "");
        assert_eq!(stopped_line(Some(ExitReason::Updated)), "");
        assert_eq!(stopped_line(None), "");
    }

    #[test]
    fn the_fixed_lines_are_ascii_and_non_empty() {
        for line in [
            NOTIFICATIONS_OFF,
            STOPPED_APP,
            STOPPED_ANDROID,
            STOPPED_OTHER,
            WORKER_STOPPED,
            ALREADY_IN_TRAY,
            THIRD_PARTY_NOTICES,
        ] {
            assert!(!line.is_empty());
            assert!(line.is_ascii(), "{line}");
        }
    }
}
