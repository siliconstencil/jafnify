//! The pure user-interface layer shared by every platform.
//!
//! Nothing here reads a sensor, opens a socket or draws: the platform fills
//! the plain input structs and draws the resulting texts and controls. The
//! wording and the decisions live here so the phone, the Linux window and a
//! future Windows window say the same thing.
//!
//! Moved from the Linux frontend in W2a with no behaviour change: the Linux
//! modules now re-export these types.

pub mod code_entry;
pub mod details;
pub mod firewall;
pub mod owner;
pub mod terms;
pub mod time;
pub mod ui_text;
pub mod view;
