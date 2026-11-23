//! Echo watch: did our own multicast come back to us?
//!
//! Multicast loopback re-enters the host's input path on the LAN interface, so
//! a host firewall that drops the discovery group drops our own announcements
//! as well. Never hearing ourselves while we are announcing therefore means
//! inbound discovery is blocked locally. It works for any firewall, needs no
//! root and adds nothing to the wire: the watch only listens on the group and
//! looks at the source address.
//!
//! The same watch also says whether any datagram came from another host (the
//! peer), on any path. Unicast discovery reaches us without the group, so the
//! firewall question is "did the peer's traffic get through at all".

use std::net::{IpAddr, Ipv4Addr};
use std::time::Duration;

use crate::discovery::{DiscoveryConfig, DiscoveryError};
use crate::wire::{Accept, Endpoint};

pub struct EchoWatch {
    ep: Endpoint,
    own: Vec<Ipv4Addr>,
    heard: bool,
    peer: bool,
}

impl EchoWatch {
    /// Joins the group like the other sockets. `own` are this host's addresses
    /// on the interfaces in `cfg`: a datagram from one of them is our echo.
    pub fn bind(cfg: &DiscoveryConfig, own: Vec<Ipv4Addr>) -> Result<Self, DiscoveryError> {
        Ok(Self {
            ep: Endpoint::open(cfg, Accept::Any)?,
            own,
            heard: false,
            peer: false,
        })
    }

    /// Reads what is queued (so the socket never fills up) and says whether an
    /// echo has been seen so far. Once true, stays true.
    pub fn poll(&mut self) -> bool {
        while let Ok(Some(d)) = self.ep.recv(Duration::ZERO) {
            if let IpAddr::V4(ip) = d.from.ip() {
                if self.own.contains(&ip) {
                    self.heard = true;
                } else {
                    self.peer = true;
                }
            }
        }
        self.heard
    }

    /// A datagram from another host arrived (any path). Once true, stays true.
    /// Call `poll` first to read what is queued.
    pub fn peer_heard(&self) -> bool {
        self.peer
    }
}
