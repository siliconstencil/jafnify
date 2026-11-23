//! Who owns the `jafn0` interface. One pure decision, shared by every root
//! path so a second app instance never takes down or re-routes a tunnel that
//! belongs to another instance (a live incident).
//!
//! An instance owns a tunnel when it brought it up itself, when `inspect`
//! showed the live peer key equals its stored peer key, or when the marker
//! matches the live peer key (with or without a saved pair; an interrupted
//! join may leave no pair but does leave the marker). A tunnel whose live peer
//! key is known and different is foreign even when a stale marker is present.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TunnelOwner {
    /// No `jafn0`.
    None,
    /// This instance's own tunnel.
    Ours,
    /// Another Jafnify instance's tunnel: never touch it.
    Foreign,
}

/// The facts the decision needs. A missing live peer key means `inspect`
/// could not read it; the marker is then the only evidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OwnershipInputs<'a> {
    pub tunnel_present: bool,
    /// The live peer key, when `inspect` could read it.
    pub live_peer_key: Option<&'a str>,
    /// This instance's stored peer key, when it has a pair.
    pub our_peer_key: Option<&'a str>,
    /// This process brought `jafn0` up (or adopted it after proving it ours).
    pub brought_up_by_us: bool,
    /// The marker in our config directory names our stored pair's peer.
    pub marker_matches: bool,
}

/// One decision for every root path. A known live peer key is authoritative:
/// a contradicting key is foreign even if a stale marker still matches.
pub fn decide(inputs: &OwnershipInputs) -> TunnelOwner {
    if !inputs.tunnel_present {
        return TunnelOwner::None;
    }
    if inputs.brought_up_by_us {
        return TunnelOwner::Ours;
    }
    if let (Some(ours), Some(live)) = (inputs.our_peer_key, inputs.live_peer_key) {
        return if live == ours {
            TunnelOwner::Ours
        } else {
            TunnelOwner::Foreign
        };
    }
    if inputs.marker_matches {
        TunnelOwner::Ours
    } else {
        TunnelOwner::Foreign
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inputs<'a>() -> OwnershipInputs<'a> {
        OwnershipInputs {
            tunnel_present: true,
            live_peer_key: None,
            our_peer_key: None,
            brought_up_by_us: false,
            marker_matches: false,
        }
    }

    #[test]
    fn no_tunnel_is_none_even_when_other_evidence_exists() {
        let i = OwnershipInputs {
            tunnel_present: false,
            brought_up_by_us: true,
            marker_matches: true,
            ..inputs()
        };
        assert_eq!(decide(&i), TunnelOwner::None);
    }

    #[test]
    fn a_tunnel_this_process_brought_up_is_ours() {
        assert_eq!(
            decide(&OwnershipInputs {
                brought_up_by_us: true,
                ..inputs()
            }),
            TunnelOwner::Ours
        );
    }

    #[test]
    fn a_live_peer_key_equal_to_ours_is_ours_even_without_a_marker() {
        assert_eq!(
            decide(&OwnershipInputs {
                live_peer_key: Some("ours"),
                our_peer_key: Some("ours"),
                ..inputs()
            }),
            TunnelOwner::Ours
        );
    }

    #[test]
    fn a_known_different_live_peer_key_is_foreign_even_with_a_marker() {
        assert_eq!(
            decide(&OwnershipInputs {
                live_peer_key: Some("theirs"),
                our_peer_key: Some("ours"),
                marker_matches: true,
                ..inputs()
            }),
            TunnelOwner::Foreign
        );
    }

    #[test]
    fn an_unreadable_live_key_falls_back_to_the_marker() {
        assert_eq!(
            decide(&OwnershipInputs {
                our_peer_key: Some("ours"),
                marker_matches: true,
                ..inputs()
            }),
            TunnelOwner::Ours
        );
        assert_eq!(
            decide(&OwnershipInputs {
                our_peer_key: Some("ours"),
                marker_matches: false,
                ..inputs()
            }),
            TunnelOwner::Foreign
        );
    }

    #[test]
    fn an_unpaired_instance_never_owns_a_tunnel() {
        // The live incident: the test build had no pair and saw the other
        // instance's jafn0. It must be foreign, never ours.
        assert_eq!(
            decide(&OwnershipInputs {
                live_peer_key: Some("theirs"),
                ..inputs()
            }),
            TunnelOwner::Foreign
        );
        assert_eq!(decide(&inputs()), TunnelOwner::Foreign);
    }

    #[test]
    fn an_unpaired_marker_that_names_the_live_peer_is_ours() {
        // An interrupted join leaves no saved pair but writes the marker
        // right after `up`. When the marker matches the live key, stop may
        // remove that orphan.
        assert_eq!(
            decide(&OwnershipInputs {
                live_peer_key: Some("orphan"),
                marker_matches: true,
                ..inputs()
            }),
            TunnelOwner::Ours
        );
    }

    #[test]
    fn an_unpaired_marker_for_another_tunnel_stays_foreign() {
        assert_eq!(
            decide(&OwnershipInputs {
                live_peer_key: Some("theirs"),
                marker_matches: false,
                ..inputs()
            }),
            TunnelOwner::Foreign
        );
    }
}
