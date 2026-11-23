//! The desktop notification lines and the rate model.
//!
//! Pure: no I/O, no clock of its own. The caller decides when a transition
//! happened and asks the policy whether a line may go out. The lines are fixed
//! and carry no peer name, key, endpoint, network name or data total.

use std::time::{Duration, Instant};

use crate::orchestrator::ConnectionState;

/// The same notification again is dropped for this long.
pub const SAME_KIND_DEDUPE: Duration = Duration::from_secs(5 * 60);

/// At most this many notifications go out in an hour.
pub const MAX_PER_HOUR: usize = 5;

/// The window the hourly count covers.
pub const HOUR: Duration = Duration::from_secs(60 * 60);

/// One notification the app can show.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Notification {
    /// The tunnel came up.
    Connected,
    /// The tunnel went down.
    Lost,
    /// The tunnel is being retried.
    Retrying,
    /// The phone lost its mobile data while the tunnel stayed up.
    PhoneLostMobileData,
    /// The phone's mobile generation dropped.
    PhoneGenerationDrop {
        /// The generation the phone was on.
        from: &'static str,
        /// The generation the phone is on now.
        to: &'static str,
    },
    /// The phone got too hot for the mobile data.
    PhoneHot {
        /// The temperature of the phone, in tenths of a degree.
        temp_c_x10: i16,
    },
    /// The phone limits itself because of the heat.
    PhoneThrottling,
    /// The link is good enough for a big download.
    GoodTime {
        /// The phone's network, in words.
        net: &'static str,
        /// The phone's signal, of four.
        signal: u8,
    },
}

impl Notification {
    /// The one line the desktop shows.
    pub fn line(&self) -> String {
        match self {
            Notification::Connected => "Jafnify connected.".to_string(),
            Notification::Lost => "Jafnify lost the connection.".to_string(),
            Notification::Retrying => "Jafnify is retrying.".to_string(),
            Notification::PhoneLostMobileData => "Phone lost mobile data.".to_string(),
            Notification::PhoneGenerationDrop { from, to } => {
                format!("Phone network changed: {from} to {to}.")
            }
            Notification::PhoneHot { temp_c_x10 } => crate::phone_heat::title(*temp_c_x10),
            Notification::PhoneThrottling => crate::phone_heat::throttling_title().to_string(),
            Notification::GoodTime { net, signal } => {
                format!("Good time for big downloads: {net}, signal {signal}/4")
            }
        }
    }

    /// The longer text under the line, when there is one.
    pub fn body(&self) -> Option<&'static str> {
        match self {
            Notification::PhoneHot { .. } => Some(crate::phone_heat::help()),
            Notification::PhoneThrottling => Some(crate::phone_heat::throttling_help()),
            _ => None,
        }
    }

    /// Whether the notification carries its own episode: the caller makes it
    /// only when something really happened, so the five-minute dedupe must
    /// not swallow a second episode of the same kind.
    fn is_episode(&self) -> bool {
        matches!(
            self,
            Notification::PhoneHot { .. }
                | Notification::PhoneThrottling
                | Notification::GoodTime { .. }
        )
    }
}

/// The notification a connection state change means, or nothing when the
/// change is not worth a line. A lost connection is one line: leaving
/// `Connected` says the connection is lost, not that it is retrying.
pub fn connection_transition(
    previous: ConnectionState,
    current: ConnectionState,
) -> Option<Notification> {
    if current == ConnectionState::Connected {
        return (previous != ConnectionState::Connected).then_some(Notification::Connected);
    }
    // Idle is the user's own stop (Disconnect, quit): not a lost connection.
    if current == ConnectionState::Idle {
        return None;
    }
    if previous == ConnectionState::Connected {
        return Some(Notification::Lost);
    }
    if current == ConnectionState::Retry {
        return Some(Notification::Retrying);
    }
    None
}

/// The rate model: one line per transition, a five-minute dedupe for the same
/// line and at most five notifications an hour.
#[derive(Debug, Default)]
pub struct NotificationPolicy {
    last_sent: Option<(Notification, Instant)>,
    sent: Vec<Instant>,
    /// True once this outage was announced (lost or retrying); cleared when
    /// the tunnel comes back, so one outage says "retrying" at most once.
    outage_announced: bool,
}

impl NotificationPolicy {
    pub fn new() -> Self {
        Self::default()
    }

    /// The line to show, or nothing when this notification is dropped.
    pub fn decide(&mut self, notification: Notification, now: Instant) -> Option<String> {
        match notification {
            Notification::Connected => self.outage_announced = false,
            Notification::Retrying if self.outage_announced => return None,
            _ => {}
        }
        if !notification.is_episode() {
            if let Some((last, at)) = self.last_sent {
                if last == notification && now.saturating_duration_since(at) < SAME_KIND_DEDUPE {
                    return None;
                }
            }
        }
        self.sent
            .retain(|at| now.saturating_duration_since(*at) < HOUR);
        if self.sent.len() >= MAX_PER_HOUR {
            return None;
        }
        self.sent.push(now);
        self.last_sent = Some((notification, now));
        if matches!(notification, Notification::Lost | Notification::Retrying) {
            self.outage_announced = true;
        }
        Some(notification.line())
    }
}
