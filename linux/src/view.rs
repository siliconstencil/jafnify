//! What the Linux window (and the CLI status) shows for each state. W2a moved
//! the pure view model into `jafnify_core::ui::view`; this module keeps the
//! Linux path and wires the `App` into the core's `ViewSource`.

use jafnify_core::ui::firewall::FirewallKind;
use jafnify_core::ConnectionState;

use crate::app::App;

pub use jafnify_core::ui::view::*;

impl ViewSource for App {
    fn state(&self) -> ConnectionState {
        App::state(self)
    }

    fn status_text(&self) -> String {
        App::status_text(self)
    }

    fn paired_name(&self) -> Option<String> {
        App::paired_name(self)
    }

    fn denied(&self) -> bool {
        App::denied(self)
    }

    fn connected_secs(&self) -> Option<u64> {
        App::connected_secs(self)
    }

    fn traffic(&self) -> Option<(u64, u64)> {
        App::traffic(self)
    }

    fn last_connected(&self) -> Option<u64> {
        App::last_connected(self)
    }

    fn firewall_blocked(&self) -> Option<FirewallKind> {
        App::firewall_blocked(self)
    }

    fn firewall_iface(&self) -> Option<String> {
        App::firewall_iface(self).map(str::to_string)
    }

    fn waiting_for_phone(&self) -> bool {
        App::waiting_for_phone(self)
    }

    fn no_network(&self) -> bool {
        App::no_network(self)
    }

    fn retry_secs_left(&self) -> Option<u64> {
        App::retry_secs_left(self)
    }

    fn polkit_prompt_expected(&self) -> bool {
        App::polkit_prompt_expected(self)
    }

    fn link_lost(&self) -> bool {
        App::link_lost(self)
    }

    fn health_verdict(&self) -> Option<String> {
        App::health_verdict(self)
    }

    fn foreign_tunnel(&self) -> bool {
        App::foreign_tunnel(self)
    }

    fn code_expired(&self) -> bool {
        App::code_expired(self)
    }

    fn kill_switch(&self) -> bool {
        App::kill_switch(self)
    }

    fn app_name(&self) -> &'static str {
        crate::flavor::DISPLAY
    }
}
