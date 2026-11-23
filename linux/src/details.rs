//! The Details panel. W2a moved the pure rows and wording into
//! `jafnify_core::ui::details`; this module keeps the Linux snapshot adapter.

use crate::control::Snapshot;

pub use jafnify_core::ui::details::*;

impl DetailsSource for Snapshot {
    fn status_text(&self) -> &str {
        &self.status_text
    }

    fn path(&self) -> Option<&str> {
        self.path.as_deref()
    }

    fn health_line(&self) -> Option<&str> {
        self.health_line.as_deref()
    }

    fn dns_through_tunnel(&self) -> bool {
        self.dns_through_tunnel
    }

    fn peer_endpoint(&self) -> Option<&str> {
        self.peer_endpoint.as_deref()
    }

    fn health_detail(&self) -> Option<&str> {
        self.health_detail.as_deref()
    }

    fn peer_key_prefix(&self) -> Option<&str> {
        self.peer_key_prefix.as_deref()
    }

    fn pair_id(&self) -> Option<&str> {
        self.pair_id.as_deref()
    }

    fn dns_override(&self) -> Option<&str> {
        self.dns_override.as_deref()
    }

    fn host_hints(&self) -> Vec<(String, String)> {
        self.host_hints
            .iter()
            .map(|hint| (hint.title.clone(), hint.help.clone()))
            .collect()
    }

    fn current_session_started_at(&self) -> i64 {
        self.current_session.started_at
    }

    fn current_session_rows(&self) -> Vec<DetailRow> {
        session_rows_with_drops(&self.current_session, self.session_drops)
    }
}
