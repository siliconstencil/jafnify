//! LAN multicast discovery. A device broadcasts an HMAC discovery token
//! (never the raw code or device name) and listens for matching peers.

use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
use std::time::Duration;

use socket2::{Domain, Protocol, Socket, Type};

use crate::pairing::DiscoveryToken;
use crate::wire::{enable_pktinfo, Accept, Endpoint, Path};
use crate::DISCOVERY_ADDR;

/// Wire frame: 1 byte version (0x01) + 48 bytes token.
const FRAME_VERSION: u8 = 0x01;
const FRAME_LEN: usize = 49;

#[derive(Debug, Clone)]
pub struct DiscoveryConfig {
    pub addr: SocketAddr,
    pub ttl: u8,
    /// Interface to join the multicast group on. None = kernel default route.
    pub iface: Option<Ipv4Addr>,
    /// Join the group and announce on every one of these interfaces (by IPv4
    /// address). Non-empty replaces `iface`. A phone acting as hotspot has a
    /// cellular default route; unpinned multicast follows it and never reaches
    /// the hotspot LAN, so the caller lists the interfaces to use.
    pub ifaces: Vec<Ipv4Addr>,
    /// Hosts that get every frame by unicast too, on the group port: the
    /// default gateway of a laptop on a hotspot. Used when multicast from the
    /// other device does not reach us.
    pub unicast: Vec<Ipv4Addr>,
}

impl Default for DiscoveryConfig {
    fn default() -> Self {
        Self {
            addr: DISCOVERY_ADDR
                .parse()
                .expect("DISCOVERY_ADDR constant is valid"),
            ttl: 4,
            iface: None,
            ifaces: Vec::new(),
            unicast: Vec::new(),
        }
    }
}

impl DiscoveryConfig {
    /// The interfaces to use: `ifaces`, else the single `iface`, else none
    /// (the kernel default route).
    pub fn interfaces(&self) -> Vec<Ipv4Addr> {
        if self.ifaces.is_empty() {
            self.iface.into_iter().collect()
        } else {
            self.ifaces.clone()
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum DiscoveryError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("multicast join failed: {0}")]
    JoinMulticast(String),
    #[error("no LAN interface selected")]
    NoLanInterface,
}

pub struct Discovery {
    ep: Endpoint,
}

/// Send one frame to the group: once per egress interface, or once the
/// default way when none is pinned. Fails only if every send failed.
pub(crate) fn send_multicast(
    socket: &UdpSocket,
    egress: &[Ipv4Addr],
    addr: SocketAddr,
    frame: &[u8],
) -> std::io::Result<()> {
    if egress.is_empty() {
        return socket.send_to(frame, addr).map(drop);
    }
    let mut result = Err(std::io::Error::from(std::io::ErrorKind::NotConnected));
    for iface in egress {
        let sent = socket2::SockRef::from(socket)
            .set_multicast_if_v4(iface)
            .and_then(|()| socket.send_to(frame, addr).map(drop));
        if sent.is_ok() || result.is_err() {
            result = sent;
        }
    }
    result
}

/// The shared multicast socket recipe (used by the per-process hub). Returns
/// the socket and the interfaces it can send through.
pub(crate) fn bind_multicast(
    cfg: &DiscoveryConfig,
) -> Result<(UdpSocket, Vec<Ipv4Addr>), DiscoveryError> {
    let ip = match cfg.addr {
        SocketAddr::V4(v4) => *v4.ip(),
        SocketAddr::V6(_) => {
            return Err(DiscoveryError::JoinMulticast(
                "only IPv4 multicast discovery is supported".into(),
            ));
        }
    };
    // Multiple Discovery instances on one host (and multiple apps on one
    // LAN) must share the discovery port; without SO_REUSEADDR/SO_REUSEPORT
    // the second bind fails with AddrInUse.
    let sock = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
    sock.set_reuse_address(true)?;
    // W1: Windows has no SO_REUSEPORT; only SO_REUSEADDR is set there, so a
    // second process on one host cannot share the port (the module-level limit
    // already says one socket per process).
    #[cfg(unix)]
    sock.set_reuse_port(true)?;
    sock.bind(&SocketAddr::from((Ipv4Addr::UNSPECIFIED, cfg.addr.port())).into())?;
    sock.set_multicast_loop_v4(true)?;
    sock.set_multicast_ttl_v4(u32::from(cfg.ttl))?;
    let ifaces = cfg.interfaces();
    if ifaces.is_empty() {
        sock.join_multicast_v4(&ip, &Ipv4Addr::UNSPECIFIED)?;
        return finish(sock, Vec::new());
    }
    // Pin join and egress to the listed interfaces. Needed when the default
    // route is a tunnel (e.g. wg0) that swallows multicast, or a cellular
    // link that never reaches the hotspot LAN. One that cannot be used (gone,
    // no address) must not stop the others.
    //
    // W1c: `join_multicast_v4` is IP_ADD_MEMBERSHIP with the interface's own
    // IPv4 address, and `set_multicast_if_v4` is IP_MULTICAST_IF, on both
    // Unix and Windows; no cfg split is needed here. Only SO_REUSEPORT above
    // is Unix-only.
    let mut usable = Vec::new();
    let mut failed = None;
    for iface in ifaces {
        match sock.join_multicast_v4(&ip, &iface) {
            Ok(()) => usable.push(iface),
            // Another listed address on a link already joined.
            Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => usable.push(iface),
            Err(e) => failed = failed.or(Some(e)),
        }
    }
    let Some(first) = usable.first() else {
        return Err(DiscoveryError::Io(failed.expect("an interface was listed")));
    };
    sock.set_multicast_if_v4(first)?;
    finish(sock, usable)
}

fn finish(
    sock: Socket,
    egress: Vec<Ipv4Addr>,
) -> Result<(UdpSocket, Vec<Ipv4Addr>), DiscoveryError> {
    let socket: UdpSocket = sock.into();
    enable_pktinfo(&socket)?;
    socket.set_nonblocking(true)?;
    Ok((socket, egress))
}

impl Discovery {
    pub fn bind(cfg: &DiscoveryConfig) -> Result<Self, DiscoveryError> {
        if cfg.interfaces().is_empty() {
            return Err(DiscoveryError::NoLanInterface);
        }
        Ok(Self {
            ep: Endpoint::open(cfg, Accept::Discovery)?,
        })
    }

    pub fn announce(&self, token: &DiscoveryToken) -> Result<(), DiscoveryError> {
        let mut frame = [0u8; FRAME_LEN];
        frame[0] = FRAME_VERSION;
        frame[1..].copy_from_slice(&token.to_bytes());
        self.ep.send(&frame)?;
        Ok(())
    }

    /// Remembers a verified peer's address: later announcements go to it by
    /// unicast as well.
    pub fn learn(&self, from: SocketAddr) {
        self.ep.learn(from);
    }

    pub fn try_receive(
        &self,
        timeout: Duration,
    ) -> Result<Option<(SocketAddr, DiscoveryToken)>, DiscoveryError> {
        Ok(self
            .try_receive_via(timeout)?
            .map(|(from, _, token)| (from, token)))
    }

    /// As `try_receive`, also saying whether the frame came by multicast or
    /// unicast.
    pub fn try_receive_via(
        &self,
        timeout: Duration,
    ) -> Result<Option<(SocketAddr, Path, DiscoveryToken)>, DiscoveryError> {
        let Some(d) = self.ep.recv(timeout)? else {
            return Ok(None);
        };
        let mut token_bytes = [0u8; 48];
        token_bytes.copy_from_slice(&d.frame[1..FRAME_LEN]);
        Ok(Some((
            d.from,
            d.path,
            DiscoveryToken::from_bytes(&token_bytes),
        )))
    }
}
