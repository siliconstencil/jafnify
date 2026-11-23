//! The phone network changes the laptop warns about.
//!
//! Pure: the caller passes the previous and the current network kind from the
//! phone measurements and gets back the warning, if any. Only the network kind
//! is used, never a network name.

use crate::notification::Notification;
use crate::phone_stats::NetworkKind;

/// The warning a phone network change produces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PhoneNetworkWarning {
    /// The phone lost its mobile data.
    LostMobileData,
    /// The phone's mobile generation dropped.
    GenerationDrop {
        /// The generation the phone was on.
        from: &'static str,
        /// The generation the phone is on now.
        to: &'static str,
    },
}

impl PhoneNetworkWarning {
    /// The notification this warning becomes.
    pub fn notification(&self) -> Notification {
        match self {
            PhoneNetworkWarning::LostMobileData => Notification::PhoneLostMobileData,
            PhoneNetworkWarning::GenerationDrop { from, to } => {
                Notification::PhoneGenerationDrop { from, to }
            }
        }
    }
}

/// The warning for a change, if it is worth a notification. An improvement, a
/// recovery or no change at all is silent.
pub fn warning(previous: NetworkKind, current: NetworkKind) -> Option<PhoneNetworkWarning> {
    match (previous, current) {
        (NetworkKind::Unknown, _) | (_, NetworkKind::Unknown) => None,
        (previous, NetworkKind::None) if previous.is_mobile() => {
            Some(PhoneNetworkWarning::LostMobileData)
        }
        // A drop of the generation, whatever variant of it the modem reports
        // (5G non-standalone to 4G+ is a 5G to 4G drop; 5G to 5G (advanced)
        // is not a drop).
        (previous, current) => match (previous.generation(), current.generation()) {
            (Some((from_rank, from)), Some((to_rank, to))) if to_rank < from_rank => {
                Some(PhoneNetworkWarning::GenerationDrop { from, to })
            }
            _ => None,
        },
    }
}
