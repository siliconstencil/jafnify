//! Why the phone app's process last ended, in words and in rules.
//!
//! Android 11 (API 30) tells an app why its previous process died
//! (`ActivityManager.getHistoricalProcessExitReasons`). The phone sends that
//! word once in the new session, on the summary line as `exit=<word>`. This
//! module owns the mapping from Android's numeric reason to the fixed wire
//! word, how old a reason may be, and what the laptop makes of it.
//!
//! Nothing here is a fault by itself: an app update and a user stop are
//! normal, a crash is still a drop. Pure arithmetic and wording.

/// How old the previous process exit may be and still describe this session:
/// ten minutes. An older reason belongs to an earlier run and says nothing
/// about this one.
pub const RECENT_MS: i64 = 10 * 60 * 1_000;

/// How close to the process start a package update must be to count as the
/// update that started this process. Two minutes covers a slow package
/// install and the service restart that follows it. Android does not always
/// keep an exit record with `PACKAGE_UPDATED` for the replaced process, so the
/// package's own install time is the reliable signal.
pub const UPDATE_SLACK_MS: i64 = 2 * 60 * 1_000;

/// The wire words of the exit reason.
pub const WORD_UPDATED: &str = "updated";
pub const WORD_USER: &str = "user";
pub const WORD_CRASH: &str = "crash";
pub const WORD_ANR: &str = "anr";
pub const WORD_LOW_MEMORY: &str = "low-memory";
pub const WORD_KILLED: &str = "killed";
pub const WORD_OTHER: &str = "other";

/// The label of an app update: not a drop, not a fault.
pub const UPDATED_LABEL: &str = "Phone app updated";

/// The hint of a drop whose phone app crashed or stopped answering.
pub const HINT_APP_STOPPED: &str = "likely the phone app stopped";

/// The hint of a drop the system closed the phone app on, for memory.
pub const HINT_ANDROID_CLOSED: &str = "likely Android closed the phone app";

/// Why the phone app's previous process ended, as far as it matters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitReason {
    /// The package was installed over the running app.
    Updated,
    /// The user stopped or force-stopped the app.
    User,
    /// The app crashed (Java or native).
    Crash,
    /// The app stopped answering: an ANR.
    Anr,
    /// The system killed it to reclaim memory.
    LowMemory,
    /// The system killed it for another reason (a signal, excessive use).
    Killed,
    /// A reason this build does not classify.
    Other,
}

impl ExitReason {
    /// The fixed wire word. The phone's own line carries this, never free text.
    pub fn word(self) -> &'static str {
        match self {
            ExitReason::Updated => WORD_UPDATED,
            ExitReason::User => WORD_USER,
            ExitReason::Crash => WORD_CRASH,
            ExitReason::Anr => WORD_ANR,
            ExitReason::LowMemory => WORD_LOW_MEMORY,
            ExitReason::Killed => WORD_KILLED,
            ExitReason::Other => WORD_OTHER,
        }
    }

    /// The reason of a wire word. An unknown word is `None`: a newer phone
    /// must not make an older laptop guess.
    pub fn parse(word: &str) -> Option<Self> {
        match word {
            WORD_UPDATED => Some(ExitReason::Updated),
            WORD_USER => Some(ExitReason::User),
            WORD_CRASH => Some(ExitReason::Crash),
            WORD_ANR => Some(ExitReason::Anr),
            WORD_LOW_MEMORY => Some(ExitReason::LowMemory),
            WORD_KILLED => Some(ExitReason::Killed),
            WORD_OTHER => Some(ExitReason::Other),
            _ => None,
        }
    }

    /// Whether the outage this exit caused is neutral: an update or the user's
    /// own stop is not a fault and is not counted.
    pub fn is_neutral(self) -> bool {
        matches!(self, ExitReason::Updated | ExitReason::User)
    }

    /// The hint this exit gives a drop that stays a drop.
    pub fn drop_hint(self) -> Option<&'static str> {
        match self {
            ExitReason::Crash | ExitReason::Anr => Some(HINT_APP_STOPPED),
            ExitReason::LowMemory | ExitReason::Killed => Some(HINT_ANDROID_CLOSED),
            ExitReason::Updated | ExitReason::User | ExitReason::Other => None,
        }
    }
}

/// Android's `ApplicationExitInfo` reason codes, by number: the phone passes
/// `getReason()` through and the core owns the meaning. The constants are
/// Android's (API 30) and never change.
pub mod android {
    pub const EXIT_SELF: i32 = 1;
    pub const SIGNALED: i32 = 2;
    pub const LOW_MEMORY: i32 = 3;
    pub const CRASH: i32 = 4;
    pub const CRASH_NATIVE: i32 = 5;
    pub const ANR: i32 = 6;
    pub const EXCESSIVE_RESOURCE_USAGE: i32 = 9;
    pub const USER_REQUESTED: i32 = 10;
    pub const USER_STOPPED: i32 = 11;
    pub const PACKAGE_UPDATED: i32 = 16;
}

/// The exit reason of an Android reason code and the age of that exit, when
/// it is recent enough to describe the session that sends it.
pub fn from_android(reason: i32, age_ms: i64) -> Option<ExitReason> {
    if !(0..=RECENT_MS).contains(&age_ms) {
        return None;
    }
    Some(match reason {
        android::PACKAGE_UPDATED => ExitReason::Updated,
        android::USER_REQUESTED | android::USER_STOPPED | android::EXIT_SELF => ExitReason::User,
        android::CRASH | android::CRASH_NATIVE => ExitReason::Crash,
        android::ANR => ExitReason::Anr,
        android::LOW_MEMORY => ExitReason::LowMemory,
        android::SIGNALED | android::EXCESSIVE_RESOURCE_USAGE => ExitReason::Killed,
        _ => ExitReason::Other,
    })
}

/// The wall-clock start of this process, from the wall clock and the two
/// monotonic clocks Android reports: elapsed realtime now and at the process
/// start. `None` when the clocks do not add up (a mocked or broken device).
pub fn process_start_ms(now_ms: i64, elapsed_ms: i64, start_elapsed_ms: i64) -> Option<i64> {
    if start_elapsed_ms < 0 || elapsed_ms < start_elapsed_ms {
        return None;
    }
    Some(now_ms.saturating_sub(elapsed_ms - start_elapsed_ms))
}

/// Whether the package was installed so close to this process's start that
/// the install is what started it. This is the reliable update signal: the
/// package's `lastUpdateTime` survives where an exit record may not.
pub fn update_since(last_update_ms: i64, process_start_ms: i64) -> bool {
    last_update_ms > 0
        && process_start_ms > 0
        && last_update_ms >= process_start_ms.saturating_sub(UPDATE_SLACK_MS)
}

/// The summary key that carries the exit reason to the laptop.
pub const SUMMARY_KEY: &str = "exit";

/// Append the `exit=` key to a summary line.
pub fn with_summary_key(line: &str, exit: Option<ExitReason>) -> String {
    match exit {
        Some(exit) => format!("{line} {SUMMARY_KEY}={}", exit.word()),
        None => line.to_string(),
    }
}

/// The exit reason of a summary line, when the phone sent one. Unknown words
/// and missing keys are `None`: the phone is not punished for being newer.
pub fn from_summary(bytes: &[u8]) -> Option<ExitReason> {
    let text = std::str::from_utf8(bytes).ok()?;
    let prefix = format!("{SUMMARY_KEY}=");
    text.split_whitespace()
        .find_map(|token| token.strip_prefix(&prefix))
        .and_then(ExitReason::parse)
}

/// What the laptop does with a neutral exit reason: the event kind and the
/// line it writes to the drop file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Neutral {
    /// The app was updated: "Phone app updated".
    Updated,
    /// The user stopped it: the same event as a Bye.
    Stopped,
}

impl ExitReason {
    /// The neutral event this reason turns an outage into, when it is one.
    pub fn neutral(self) -> Option<Neutral> {
        match self {
            ExitReason::Updated => Some(Neutral::Updated),
            ExitReason::User => Some(Neutral::Stopped),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_android_reason_maps_to_a_wire_word_and_back() {
        for (code, reason) in [
            (android::PACKAGE_UPDATED, ExitReason::Updated),
            (android::USER_REQUESTED, ExitReason::User),
            (android::USER_STOPPED, ExitReason::User),
            (android::EXIT_SELF, ExitReason::User),
            (android::CRASH, ExitReason::Crash),
            (android::CRASH_NATIVE, ExitReason::Crash),
            (android::ANR, ExitReason::Anr),
            (android::LOW_MEMORY, ExitReason::LowMemory),
            (android::SIGNALED, ExitReason::Killed),
            (android::EXCESSIVE_RESOURCE_USAGE, ExitReason::Killed),
            (0, ExitReason::Other),
            (13, ExitReason::Other),
        ] {
            assert_eq!(from_android(code, 1_000), Some(reason), "code {code}");
            assert_eq!(ExitReason::parse(reason.word()), Some(reason));
        }
    }

    #[test]
    fn a_package_install_that_started_this_process_is_an_update() {
        let start = 1_700_000_000_000;
        assert!(update_since(start - 5_000, start), "the install just before");
        assert!(update_since(start - UPDATE_SLACK_MS, start), "at the edge");
        assert!(!update_since(start - UPDATE_SLACK_MS - 1, start), "too long ago");
        assert!(update_since(start + 1_000, start), "a same-moment install");
        assert!(!update_since(0, start), "no install time");
        assert!(!update_since(start, 0), "no process start");
        // An update that happened while the app was closed and opened much
        // later is not this start's update.
        assert!(!update_since(start - 3_600_000, start));
    }

    #[test]
    fn the_process_start_comes_from_the_two_monotonic_clocks() {
        // The process began 7 s before "now".
        assert_eq!(
            process_start_ms(1_700_000_000_000, 500_000, 493_000),
            Some(1_699_999_993_000)
        );
        // A process that began at this instant.
        assert_eq!(
            process_start_ms(1_700_000_000_000, 500_000, 500_000),
            Some(1_700_000_000_000)
        );
        // A start in the future of the elapsed clock is nonsense: no answer.
        assert_eq!(process_start_ms(1_700_000_000_000, 500_000, 500_001), None);
        assert_eq!(process_start_ms(1_700_000_000_000, 500_000, -1), None);
        // This process started 3 s after an install 30 s ago: updated.
        let start = process_start_ms(1_700_000_060_000, 60_000, 57_000).unwrap();
        assert!(update_since(1_700_000_030_000, start));
    }

    #[test]
    fn a_reason_older_than_ten_minutes_is_not_this_session() {
        assert_eq!(from_android(android::PACKAGE_UPDATED, RECENT_MS), Some(ExitReason::Updated));
        assert_eq!(from_android(android::PACKAGE_UPDATED, RECENT_MS + 1), None);
        assert_eq!(from_android(android::PACKAGE_UPDATED, -1), None);
    }

    #[test]
    fn an_unknown_word_and_a_missing_key_are_tolerated() {
        assert_eq!(from_summary(b"JFS1 1 summary v=1 thr=0 exit=updated"), Some(ExitReason::Updated));
        assert_eq!(from_summary(b"JFS1 1 summary v=1 thr=0"), None);
        assert_eq!(
            from_summary(b"JFS1 1 summary v=1 exit=teleported"),
            None,
            "a newer phone must not make an older laptop guess"
        );
        assert_eq!(from_summary(b"\xff\xfe"), None);
    }

    #[test]
    fn the_key_is_appended_only_when_there_is_a_reason() {
        assert_eq!(
            with_summary_key("JFS1 1 summary v=1", Some(ExitReason::Updated)),
            "JFS1 1 summary v=1 exit=updated"
        );
        assert_eq!(with_summary_key("JFS1 1 summary v=1", None), "JFS1 1 summary v=1");
    }

    #[test]
    fn an_update_and_a_user_stop_are_neutral_and_a_crash_is_not() {
        assert_eq!(ExitReason::Updated.neutral(), Some(Neutral::Updated));
        assert_eq!(ExitReason::User.neutral(), Some(Neutral::Stopped));
        for reason in [
            ExitReason::Crash,
            ExitReason::Anr,
            ExitReason::LowMemory,
            ExitReason::Killed,
            ExitReason::Other,
        ] {
            assert_eq!(reason.neutral(), None, "{reason:?}");
            assert!(!reason.is_neutral());
        }
        assert!(ExitReason::Updated.is_neutral());
        assert!(ExitReason::User.is_neutral());
    }

    #[test]
    fn the_drop_hints_say_likely_and_name_the_app_or_android() {
        assert_eq!(ExitReason::Crash.drop_hint(), Some("likely the phone app stopped"));
        assert_eq!(ExitReason::Anr.drop_hint(), Some("likely the phone app stopped"));
        assert_eq!(
            ExitReason::LowMemory.drop_hint(),
            Some("likely Android closed the phone app")
        );
        assert_eq!(
            ExitReason::Killed.drop_hint(),
            Some("likely Android closed the phone app")
        );
        assert_eq!(ExitReason::Other.drop_hint(), None);
    }
}
