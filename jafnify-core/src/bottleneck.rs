//! The one-line bottleneck verdict for the Linux Experimental block.
//!
//! The PC leg is the last link test of this window session; the phone leg is
//! the current phone summary. Both are already measured, so this module is
//! arithmetic and wording only: no clock, no socket, no storage.

use crate::phone_stats::{NetworkKind, PhoneStats};

/// The PC leg of the last link test: average RTT and loss.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LinkLeg {
    pub avg_ms: u64,
    pub loss_pct: u64,
}

/// Wi-Fi is slow above this average.
pub const PC_AVG_MS: u64 = 50;
/// Wi-Fi is slow at or above this loss percentage.
pub const PC_LOSS_PCT: u64 = 2;
/// The phone leg is slow above this DNS answer time.
pub const PHONE_DNS_MS: u32 = 100;

const BOTH: &str = "Slow because of: Wi-Fi to the phone and mobile network";
const WIFI: &str = "Slow because of: Wi-Fi to the phone";
const PHONE: &str = "Slow because of: mobile network";
const NONE: &str = "No clear bottleneck";
const NEED_LINK: &str = "Not enough data: run the link test.";
const NEED_PHONE: &str = "Not enough data: phone measurements are off or unavailable.";
const NEED_BOTH: &str = "Not enough data yet.";
const LIKELY_BOTH: &str = "Likely slow parts: Wi-Fi to the phone and mobile network";
const LIKELY_WIFI: &str = "Likely slow part: Wi-Fi to the phone";

/// The verdict for the two legs. A missing leg produces its own line; when
/// both are present the rule compares the PC average/loss and the phone DNS
/// answer time or a missing network.
pub fn line(link: Option<LinkLeg>, phone: Option<&PhoneStats>) -> String {
    match verdict(link, phone) {
        Verdict::Both => BOTH,
        Verdict::Wifi => WIFI,
        Verdict::Phone => PHONE,
        Verdict::None => NONE,
        Verdict::NeedLink => NEED_LINK,
        Verdict::NeedPhone => NEED_PHONE,
        Verdict::NeedBoth => NEED_BOTH,
    }
    .to_string()
}

/// The verdict for the passive PC leg. That leg is the round trip to the
/// phone app, so it also carries the phone's own reply delay: the Wi-Fi part
/// is only "likely" and never a certainty.
pub fn passive_line(link: Option<LinkLeg>, phone: Option<&PhoneStats>) -> String {
    match verdict(link, phone) {
        Verdict::Both => LIKELY_BOTH,
        Verdict::Wifi => LIKELY_WIFI,
        Verdict::Phone => PHONE,
        Verdict::None => NONE,
        Verdict::NeedLink => NEED_LINK,
        Verdict::NeedPhone => NEED_PHONE,
        Verdict::NeedBoth => NEED_BOTH,
    }
    .to_string()
}

/// What the two legs say, without the wording.
enum Verdict {
    Both,
    Wifi,
    Phone,
    None,
    NeedLink,
    NeedPhone,
    NeedBoth,
}

fn verdict(link: Option<LinkLeg>, phone: Option<&PhoneStats>) -> Verdict {
    let (link, phone) = match (link, phone) {
        (Some(link), Some(phone)) => (link, phone),
        (None, None) => return Verdict::NeedBoth,
        (None, Some(_)) => return Verdict::NeedLink,
        (Some(_), None) => return Verdict::NeedPhone,
    };
    let wifi_slow = link.avg_ms > PC_AVG_MS || link.loss_pct >= PC_LOSS_PCT;
    let carrier_slow = matches!(phone.net, NetworkKind::Mobile2g | NetworkKind::Mobile3g)
        || phone.rtt_ms.map(|rtt| rtt > PHONE_DNS_MS).unwrap_or(false)
        || phone.net == NetworkKind::None;
    match (wifi_slow, carrier_slow) {
        (true, true) => Verdict::Both,
        (true, false) => Verdict::Wifi,
        (false, true) => Verdict::Phone,
        (false, false) => Verdict::None,
    }
}
