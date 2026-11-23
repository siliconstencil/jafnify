//! Routing and smoltcp-facing part of the library.
//! 
//! This moduile allows a pair of tokio::sync::mpsc channels that represent sent and transmitted IP bytes to be used to access outer
//! network using operating system's sockets.
//! 
//! "host" network represents operating system's sockets and communication counterparts that are accessed using them.
//! 
//! "internal" network represents senders and receivers of `BytesMut`-wrapped IP packet queues.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};
use std::sync::Arc;
use std::time::Instant;

use bytes::BytesMut;
use hashbrown::HashMap;

use smoltcp::wire::{IpAddress, IpProtocol, Ipv4Packet, Ipv6Packet, UdpPacket};
use smoltcp::wire::{IpEndpoint, IpVersion, TcpPacket};

use tokio::sync::mpsc::{channel, Receiver, Sender};
use tracing::{debug, error, info, warn};

use crate::ArmedJoinHandle;

/// Upper bound on live per-flow sockets. Each TCP entry can allocate
/// `tcp_buffer_size` for receive plus `tcp_buffer_size` for transmit; the
/// sharer sets that to 262144, so 1024 established TCP flows can reserve about
/// 512 MiB before the cap starts evicting. UDP entries are much smaller.
const MAX_NAT_ENTRIES: usize = 1024;

/// A TCP flow that has not reached ESTABLISHED is cheap to lose and is
/// preferred for eviction over one carrying data.
const TCP_STATE_OTHER: u8 = 0;
const TCP_STATE_ESTABLISHED: u8 = 2;

/// One live NAT flow. `last_active_ms` is shared with its worker so both
/// directions of traffic move the eviction clock.
struct NatEntry {
    sender: Sender<BytesMut>,
    last_active_ms: Arc<AtomicU64>,
    tcp_state: Arc<AtomicU8>,
    permanent: bool,
}

impl NatEntry {
    fn new(sender: Sender<BytesMut>, now_ms: u64, permanent: bool) -> Self {
        Self {
            sender,
            last_active_ms: Arc::new(AtomicU64::new(now_ms)),
            tcp_state: Arc::new(AtomicU8::new(TCP_STATE_OTHER)),
            permanent,
        }
    }

    fn touch(&self, now_ms: u64) {
        self.last_active_ms.store(now_ms, Ordering::Relaxed);
    }
}

fn eviction_class(key: &NatKey, entry: &NatEntry) -> u8 {
    match key {
        NatKey::Udp { .. } | NatKey::Pingable { .. } => 0,
        NatKey::Tcp { .. }
            if entry.tcp_state.load(Ordering::Relaxed) == TCP_STATE_ESTABLISHED =>
        {
            2
        }
        NatKey::Tcp { .. } => 1,
    }
}

fn evict_one(table: &mut HashMap<NatKey, NatEntry>, now_ms: u64) -> Option<NatKey> {
    let victim = table
        .iter()
        .filter(|(_, entry)| !entry.permanent)
        .min_by_key(|(key, entry)| {
            (
                eviction_class(key, entry),
                entry.last_active_ms.load(Ordering::Relaxed),
            )
        })
        .map(|(key, _)| *key)?;
    let entry = table.remove(&victim)?;
    let idle_ms = now_ms.saturating_sub(entry.last_active_ms.load(Ordering::Relaxed));
    info!("NAT full: evicting {victim} after {idle_ms} ms idle");
    Some(victim)
}

fn make_room(table: &mut HashMap<NatKey, NatEntry>, max: usize, now_ms: u64) {
    while table.len() >= max {
        if evict_one(table, now_ms).is_none() {
            break;
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum NatKey {
    Tcp {
        client_side: IpEndpoint,
        external_side: IpEndpoint,
    },
    Udp {
        client_side: IpEndpoint,
    },
    Pingable {
        client_side: IpAddress,
        external_side: IpAddress,
    },
}

impl std::fmt::Display for NatKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NatKey::Tcp {
                client_side,
                external_side,
            } => write!(f, "TCP {client_side} -> {external_side}"),
            NatKey::Udp { client_side } => write!(f, "UDP {client_side} -> *"),
            NatKey::Pingable {
                client_side,
                external_side,
            } => write!(f, "Pinger {client_side} -> {external_side}"),
        }
    }
}

/// Supplies the body of a stats response. The provider decides whether the
/// request is valid and returns `None` when there is no data to send.
pub trait StatsProvider: Send + Sync {
    /// The response body for an authenticated request, or `None` to stay
    /// silent (switch off, no data, or an unsupported request).
    fn respond(&self, request: &[u8]) -> Option<Vec<u8>>;
}

/// Options regarding interaction with smoltcp and host sockets.
pub struct Opts {
    /// If UDP datagrams are directed at this socket address then attempt to reply to a DNS request internally instead of forwarding the datagram properly
    pub dns_addr: Option<SocketAddr>,

    /// If UDP datagrams are directed at this socket address then answer with
    /// the stats provider internally. `None` disables the handler entirely.
    pub stats_addr: Option<SocketAddr>,

    /// The in-process source of stats responses. `None` or an empty provider
    /// answers nothing, while still consuming the datagram.
    pub stats_provider: Option<Arc<dyn StatsProvider>>,

    /// If ICMP or ICMPv6 packet is directed at this address, route it to smoltcp's interface (which will reply to ICMP echo requests) instead of dropping it.
    pub pingable: Option<IpAddr>,

    /// Maximum transfer unit to use for smoltcp stack
    pub mtu: usize,

    /// Receive and send smoltcp TCP socket buffer sizes. Does not affect host socket buffer sizes.
    pub tcp_buffer_size: usize,

    /// Listen these UDP ports and direct content into the internal network.
    
    pub incoming_udp: Vec<PortForward>,

    /// Listen these TCP ports on host and direct connections into the internal network.
    pub incoming_tcp: Vec<PortForward>,
}

/// Instructions how to forward a port from host to our internal IP network.
pub struct PortForward {
    /// Which socket address to use to bind the host socket to.
    pub host: SocketAddr,

    /// Typically incoming connection or datagram address is also used as a source address within internal network.
    /// Setting `src` allows you to override this and just use source fixed address.
    /// 
    /// If port part is `0` and this port forward is used for `incoming_tcp`, the port number is filled in with nonzero value is some way.
    pub src: Option<SocketAddr>,

    /// Where within our internal network to forward connections or datagrams to.
    pub dst: SocketAddr,
}

mod serve_dns;

/// Median of the successful DNS lookup durations from the last two minutes.
/// None when fewer than three samples exist. Reads a small in-process ring.
pub fn dns_answer_median_ms() -> Option<u32> {
    serve_dns::dns_answer_median_ms()
}
mod serve_pingable;
mod serve_stats;

/// A fixed one-second window that admits two stats requests.
#[derive(Default)]
struct StatsWindow {
    start_ms: u64,
    count: u32,
}

impl StatsWindow {
    fn allow(&mut self, now_ms: u64) -> bool {
        if now_ms.saturating_sub(self.start_ms) >= 1_000 {
            self.start_ms = now_ms;
            self.count = 0;
        }
        if self.count >= 2 {
            return false;
        }
        self.count += 1;
        true
    }
}

/// Whether this destination is the configured stats socket.
fn is_stats_target(dst: IpAddress, dst_port: u16, configured: Option<SocketAddr>) -> bool {
    configured
        .map(|addr| addr.port() == dst_port && addr.ip() == IpAddr::from(dst))
        .unwrap_or(false)
}

/// Packets queued from the router to one NAT socket. The router never waits on
/// this queue (it drops when full), so it only needs to absorb a burst.
const PER_SOCKET_QUEUE: usize = 64;
mod serve_tcp;
mod serve_udp;

/// Start the router using given options and a part of channels that represent internal network.
/// 
/// Dropping the resulting `Future` should abort the entire router.
/// Whether a tunnel client may reach `dst` through this device. The internet is
/// allowed; this device and its private neighbours are not: loopback, link-local,
/// unspecified, RFC 1918, carrier-grade NAT (where the phone's own cellular
/// address usually sits), multicast and broadcast.
/// Whether an IPv4 address is a destination a tunnel client may reach.
fn ipv4_outbound_allowed(v4: Ipv4Addr) -> bool {
    let o = v4.octets();
    let carrier_nat = o[0] == 100 && (o[1] & 0xc0) == 0x40;
    !(v4.is_loopback()
        || v4.is_link_local()
        || v4.is_private()
        || v4.is_unspecified()
        || v4.is_multicast()
        || v4.is_broadcast()
        || o[0] == 0
        || carrier_nat)
}

/// The IPv4 address embedded in an IPv4-mapped (`::ffff:a.b.c.d`) or the
/// deprecated IPv4-compatible (`::a.b.c.d`) IPv6 address.
fn embedded_ipv4(v6: Ipv6Addr) -> Option<Ipv4Addr> {
    if let Some(v4) = v6.to_ipv4_mapped() {
        return Some(v4);
    }
    let segments = v6.segments();
    if segments[..6].iter().all(|segment| *segment == 0) {
        let value = (u32::from(segments[6]) << 16) | u32::from(segments[7]);
        return Some(Ipv4Addr::from(value));
    }
    None
}

/// Whether a tunnel client may reach `dst` through this device. The internet is
/// allowed; this device and its private neighbours are not: loopback,
/// link-local, unspecified, RFC 1918, carrier-grade NAT, multicast, broadcast,
/// IPv6 documentation and embedded IPv4 addresses that resolve to any of them.
pub fn outbound_allowed(dst: IpAddr) -> bool {
    match dst {
        IpAddr::V4(v4) => ipv4_outbound_allowed(v4),
        IpAddr::V6(v6) => {
            if let Some(v4) = embedded_ipv4(v6) {
                return ipv4_outbound_allowed(v4);
            }
            let first = v6.segments()[0];
            let unique_local = (first & 0xfe00) == 0xfc00;
            let link_local = (first & 0xffc0) == 0xfe80;
            let documentation = first == 0x2001 && v6.segments()[1] == 0x0db8;
            !(v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                || unique_local
                || link_local
                || documentation)
        }
    }
}

/// Run the userspace router: packets from WireGuard go to the host network
/// and replies go back, until either channel closes.
pub async fn run(
    mut rx_from_wg: Receiver<BytesMut>,
    tx_to_wg: Sender<BytesMut>,
    opts: Opts,
) -> anyhow::Result<()> {
    let mtu = opts.mtu;
    let tcp_buffer_size = opts.tcp_buffer_size;
    let stats_addr = opts.stats_addr;
    let stats_provider = opts.stats_provider.clone();
    let mut stats_window = StatsWindow::default();
    let start = Instant::now();
    let mut table = HashMap::<NatKey, NatEntry>::new();

    for PortForward { host, src, dst } in opts.incoming_udp {
        let tx_to_wg2 = tx_to_wg.clone();
        let (tx_persocket_fromwg, rx_persocket_fromwg) = channel(PER_SOCKET_QUEUE);
        let last_active = Arc::new(AtomicU64::new(0));
        let worker_active = last_active.clone();
        tokio::spawn(async move {
            if let Some(src) = src {
                info!(
                    "Permanent UDP forward from host {}: would send {} -> {}",
                    host, src, dst
                );
            } else {
                info!(
                    "Permanent UDP forward from host {}: would send * -> {}",
                    host, dst
                );
            }
            let ret = serve_udp::serve_udp(
                tx_to_wg2,
                rx_persocket_fromwg,
                dst.into(),
                Some(host),
                src.map(IpEndpoint::from),
                worker_active,
                start,
            )
            .await;
            if let Err(e) = ret {
                warn!("Permanent UDP forward exited with error: {e}");
            }
        });
        let k = NatKey::Udp {
            client_side: dst.into(),
        };
        table.insert(
            k,
            NatEntry {
                sender: tx_persocket_fromwg,
                last_active_ms: last_active,
                tcp_state: Arc::new(AtomicU8::new(TCP_STATE_OTHER)),
                permanent: true,
            },
        );
    }

    let (tx_closes, mut rx_closes): (Sender<NatKey>, Receiver<NatKey>) = channel(4);
    let (tx_opens, mut rx_opens): (
        Sender<(NatKey, Sender<BytesMut>, Arc<AtomicU64>, Arc<AtomicU8>)>,
        Receiver<(NatKey, Sender<BytesMut>, Arc<AtomicU64>, Arc<AtomicU8>)>,
    ) = channel(4);

    // To make dropping router's Future automatically close listening ports
    let mut tcp_auto_aborter : Vec<ArmedJoinHandle> = vec![];
    for PortForward { host, src, dst } in opts.incoming_tcp {
        let tx_to_wg2 = tx_to_wg.clone();
        let tcps = tokio::net::TcpListener::bind(host).await?;
        let tx_opens2 = tx_opens.clone();
        let tx_closes2 = tx_closes.clone();
        let jh = tokio::spawn(async move {
            if let Some(src) = src {
                info!(
                    "Permanent TCP forward from host {}: would connect {} -> {}",
                    host, src, dst
                );
            } else {
                info!(
                    "Permanent TCP forward from host {}: would connect * -> {}",
                    host, dst
                );
            }
            let mut counter : u16 = 1024;
            while let Ok((tcp, from)) = tcps.accept().await {
                let (tx_persocket_fromwg, rx_persocket_fromwg) = channel(PER_SOCKET_QUEUE);

                let tx_to_wg3 = tx_to_wg2.clone();
                let tx_closes3 = tx_closes2.clone();
                let mut src = src.unwrap_or(from);
                if src.port() == 0 {
                    src.set_port(counter);
                    counter = counter.wrapping_add(1);
                    if counter == 0 { counter = 1024 }
                }
                let k = NatKey::Tcp {
                    client_side: dst.into(),
                    external_side: src.into(),
                };
                let last_active = Arc::new(AtomicU64::new(0));
                let tcp_state = Arc::new(AtomicU8::new(TCP_STATE_OTHER));
                let worker_active = last_active.clone();
                let worker_state = tcp_state.clone();
                tokio::spawn(async move {
                    info!("Incoming TCP connection from {} to {} on host side. Connecting from {} to {} within Wireguard.", from, host, src, dst);

                    let ret = serve_tcp::serve_tcp(
                        tx_to_wg3,
                        rx_persocket_fromwg,
                        IpEndpoint::from(src),
                        serve_tcp::ServeTcpMode::Incoming { tcp, client_addr: IpEndpoint::from(dst) },
                        mtu,
                        tcp_buffer_size,
                        worker_active,
                        worker_state,
                        start,
                    )
                    .await;

                    if let Err(e) = ret {
                        warn!("TCP back connection from {from} exited with error: {e}");
                    }

                    info!("  Finished serving {from}");
                    let _ = tx_closes3.send(k).await;
                });
                if tx_opens2
                    .send((k, tx_persocket_fromwg, last_active, tcp_state))
                    .await
                    .is_err()
                {
                    break;
                }
            }
            warn!("Finished listening TCP {host}");
        });
        tcp_auto_aborter.push(ArmedJoinHandle(jh));
    }

    type OpenFlow = (NatKey, Sender<BytesMut>, Arc<AtomicU64>, Arc<AtomicU8>);

    enum SelectOutcome {
        PacketFromWg(Option<BytesMut>),
        ConnectionFinish(Option<NatKey>),
        IncomingTcpStart(Option<OpenFlow>),
    }

    loop {
        let now_ms = start.elapsed().as_millis() as u64;
        let ret = tokio::select! {
            x = rx_from_wg.recv() => SelectOutcome::PacketFromWg(x),
            x = rx_closes.recv() => SelectOutcome::ConnectionFinish(x),
            x = rx_opens.recv() => SelectOutcome::IncomingTcpStart(x),
        };

        let buf = match ret {
            SelectOutcome::PacketFromWg(Some(x)) => x,
            SelectOutcome::ConnectionFinish(None)
            | SelectOutcome::PacketFromWg(None)
            | SelectOutcome::IncomingTcpStart(None) => break,
            SelectOutcome::ConnectionFinish(Some(k)) => {
                table.remove(&k);
                continue;
            }
            SelectOutcome::IncomingTcpStart(Some((k, sender, last_active, tcp_state))) => {
                make_room(&mut table, MAX_NAT_ENTRIES, now_ms);
                table.insert(
                    k,
                    NatEntry {
                        sender,
                        last_active_ms: last_active,
                        tcp_state,
                        permanent: false,
                    },
                );
                continue;
            }
        };

        let (src_addr, dst_addr, nextproto, payload): (IpAddress, IpAddress, IpProtocol, &[u8]) =
            match IpVersion::of_packet(&buf[..]) {
                Err(_e) => {
                    warn!("Malformed packet");
                    continue;
                }
                Ok(IpVersion::Ipv4) => {
                    let Ok(p) = Ipv4Packet::new_checked(&buf[..]) else {
                    error!("Dwarf packet");
                    continue;
                };
                    (
                        p.src_addr().into(),
                        p.dst_addr().into(),
                        p.next_header(),
                        p.payload(),
                    )
                }
                Ok(IpVersion::Ipv6) => {
                    let Ok(p) = Ipv6Packet::new_checked(&buf[..]) else {
                    error!("Dwarf packet");
                    continue;
                };
                    (
                        p.src_addr().into(),
                        p.dst_addr().into(),
                        p.next_header(),
                        p.payload(),
                    )
                }
            };

        let key = match nextproto {
            IpProtocol::Udp => match UdpPacket::new_checked(payload) {
                Ok(u) => {
                    if let Some(ref dns) = opts.dns_addr {
                        if dns.port() == u.dst_port() && dns.ip() == IpAddr::from(dst_addr) {
                            let tx_to_wg2 = tx_to_wg.clone();
                            tokio::spawn(async move {
                                if let Ok(reply) = serve_dns::dns(buf).await {
                                    debug!("Sending DNS reply");
                                    let _ = tx_to_wg2.send(reply).await;
                                } else {
                                    warn!("Failed to calculate DNS reply");
                                }
                            });

                            continue;
                        }
                    }
                    if is_stats_target(dst_addr, u.dst_port(), stats_addr) {
                        if stats_window.allow(now_ms) {
                            if let Some(provider) = stats_provider.clone() {
                                let request = buf.clone();
                                let payload = u.payload().to_vec();
                                let tx_to_wg2 = tx_to_wg.clone();
                                tokio::spawn(async move {
                                    if let Some(body) = provider.respond(&payload) {
                                        if let Ok(reply) = serve_stats::reply(&request, &body) {
                                            debug!("Sending stats reply");
                                            let _ = tx_to_wg2.send(reply).await;
                                        } else {
                                            warn!("Failed to build a stats reply");
                                        }
                                    }
                                });
                            }
                        }
                        continue;
                    }
                    if !outbound_allowed(IpAddr::from(dst_addr)) {
                        debug!("Dropped UDP to a local address {dst_addr}");
                        continue;
                    }
                    NatKey::Udp {
                        client_side: IpEndpoint {
                            addr: src_addr,
                            port: u.src_port(),
                        },
                    }
                }
                Err(_e) => {
                    warn!("Failed to parse UDP packet");
                    continue;
                }
            },
            IpProtocol::Tcp => match TcpPacket::new_checked(payload) {
                Ok(_) if !outbound_allowed(IpAddr::from(dst_addr)) => {
                    debug!("Dropped TCP to a local address {dst_addr}");
                    continue;
                }
                Ok(u) => NatKey::Tcp {
                    client_side: IpEndpoint {
                        addr: src_addr,
                        port: u.src_port(),
                    },
                    external_side: IpEndpoint {
                        addr: dst_addr,
                        port: u.dst_port(),
                    },
                },
                Err(_e) => {
                    warn!("Failed to parse TCP packet");
                    continue;
                }
            },
            IpProtocol::Icmp => {
                debug!("Icmp");
                if Some(IpAddr::from(dst_addr)) == opts.pingable {
                    NatKey::Pingable {
                        client_side: src_addr,
                        external_side: dst_addr,
                    }
                } else {
                    continue;
                }
            }
            IpProtocol::Icmpv6 => {
                debug!("Icmpv6");
                if Some(IpAddr::from(dst_addr)) == opts.pingable {
                    NatKey::Pingable {
                        client_side: src_addr,
                        external_side: dst_addr,
                    }
                } else {
                    continue;
                }
            }
            x => {
                warn!("Uknown protocol in IP packet: {}", x);
                continue;
            }
        };
        if !table.contains_key(&key) {
            make_room(&mut table, MAX_NAT_ENTRIES, now_ms);
        }
        let entry = match table.entry(key) {
            hashbrown::hash_map::Entry::Occupied(mut entry) => {
                entry.get_mut().touch(now_ms);
                entry.into_mut()
            }
            hashbrown::hash_map::Entry::Vacant(entry) => {
                info!("Serving {}", key);
                let now = start.elapsed().as_millis() as u64;
                let tx_to_wg2 = tx_to_wg.clone();
                let (tx_persocket_fromwg, rx_persocket_fromwg) = channel(PER_SOCKET_QUEUE);
                let k = *entry.key();
                let tx_closes = tx_closes.clone();
                let nat_entry = NatEntry::new(tx_persocket_fromwg, now, false);
                let worker_active = nat_entry.last_active_ms.clone();
                let worker_state = nat_entry.tcp_state.clone();
                tokio::spawn(async move {
                    let ret = match k {
                        NatKey::Tcp {
                            external_side,
                            client_side: _,
                        } => {
                            serve_tcp::serve_tcp(
                                tx_to_wg2,
                                rx_persocket_fromwg,
                                external_side,
                                serve_tcp::ServeTcpMode::Outgoing,
                                mtu,
                                tcp_buffer_size,
                                worker_active,
                                worker_state,
                                start,
                            )
                            .await
                        }
                        NatKey::Udp { client_side } => {
                            serve_udp::serve_udp(
                                tx_to_wg2,
                                rx_persocket_fromwg,
                                client_side,
                                None,
                                None,
                                worker_active,
                                start,
                            )
                            .await
                        }
                        NatKey::Pingable {
                            client_side,
                            external_side,
                        } => {
                            serve_pingable::pingable(
                                tx_to_wg2,
                                rx_persocket_fromwg,
                                external_side,
                                client_side,
                                mtu,
                                worker_active,
                                start,
                            )
                            .await
                        }
                    };
                    if let Err(e) = ret {
                        error!("  finished serving {k}: {e}");
                    } else {
                        info!("  Finished serving {k}");
                    }
                    let _ = tx_closes.send(k).await;
                });

                entry.insert(nat_entry)
            }
        };
        let per_socket_sender = &mut entry.sender;

        // Never block the router on one slow socket: waiting here stops the router
        // draining packets from WireGuard, which can close a wait cycle with that
        // socket's own sends to WireGuard and freeze the tunnel. Drop the packet;
        // TCP retransmits and UDP tolerates loss.
        match per_socket_sender.try_send(buf) {
            Ok(()) => {}
            Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                debug!("per-socket queue full, dropped a packet");
            }
            Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                error!("Failed to send to a per-socket interface");
            }
        }
    }
    warn!("Routed stopped: no more incoming packets possible");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn allowed(s: &str) -> bool {
        outbound_allowed(s.parse().unwrap())
    }

    // The lab's "internet" host lives in TEST-NET-3 (203.0.113.0/24), so IPv4
    // documentation ranges stay reachable. The IPv6 documentation range is
    // blocked explicitly below because it is never a real destination.
    #[test]
    fn a_public_address_is_allowed() {
        assert!(allowed("203.0.113.1"), "TEST-NET-3 is the lab's internet host");
        assert!(allowed("198.51.100.1"), "TEST-NET-2");
        assert!(allowed("192.0.2.1"), "TEST-NET-1");
    }

    #[test]
    fn ipv4_mapped_and_compatible_ipv6_addresses_are_checked_as_ipv4() {
        for denied in [
            "::ffff:127.0.0.1",
            "::ffff:192.168.43.1",
            "::ffff:100.64.0.1",
            "::ffff:10.0.0.1",
            "::127.0.0.1",
            "::192.168.43.1",
            "::100.64.0.1",
            "::10.0.0.1",
        ] {
            assert!(!allowed(denied), "{denied} must not be reachable");
        }
        assert!(allowed("::ffff:1.1.1.1"));
        assert!(allowed("::1.1.1.1"));
    }

    #[test]
    fn the_ipv6_documentation_range_and_unspecified_address_are_blocked() {
        assert!(!allowed("2001:db8::1"));
        assert!(!allowed("2001:db8:ffff::1"));
        assert!(!allowed("::"));
    }

    // The tunnel client reaches the internet, not this device or its private
    // neighbours.
    #[test]
    fn a_tunnel_client_reaches_the_internet_and_not_local_or_private_addresses() {
        assert!(allowed("8.8.8.8"));
        assert!(allowed("1.1.1.1"));
        assert!(allowed("100.128.0.1"), "just outside carrier NAT");
        for denied in [
            "127.0.0.1",
            "169.254.10.1",
            "0.0.0.0",
            "10.0.2.1",
            "172.16.0.5",
            "192.168.43.1",
            "100.100.1.1",
            "224.0.0.1",
            "255.255.255.255",
            "fe80::1",
            "fd00::1",
            "::1",
        ] {
            assert!(!allowed(denied), "{denied} must not be reachable");
        }
    }

    fn udp_key(port: u16) -> NatKey {
        NatKey::Udp {
            client_side: IpEndpoint::new(IpAddress::v4(10, 0, 0, 2), port),
        }
    }

    fn tcp_key(port: u16) -> NatKey {
        NatKey::Tcp {
            client_side: IpEndpoint::new(IpAddress::v4(10, 0, 0, 2), port),
            external_side: IpEndpoint::new(IpAddress::v4(1, 1, 1, 1), 443),
        }
    }

    fn entry(last_active_ms: u64, established: bool) -> NatEntry {
        let (tx, _rx) = channel(1);
        NatEntry {
            sender: tx,
            last_active_ms: Arc::new(AtomicU64::new(last_active_ms)),
            tcp_state: Arc::new(AtomicU8::new(if established {
                TCP_STATE_ESTABLISHED
            } else {
                TCP_STATE_OTHER
            })),
            permanent: false,
        }
    }

    #[test]
    fn an_active_old_flow_survives_while_a_newer_idle_flow_is_evicted() {
        let mut table: HashMap<NatKey, NatEntry> = HashMap::new();
        // Created first, but active until just now.
        table.insert(udp_key(1), entry(9_000, false));
        // Created later, but idle for most of the window.
        table.insert(udp_key(2), entry(1_000, false));

        assert_eq!(evict_one(&mut table, 10_000), Some(udp_key(2)));
        assert!(table.contains_key(&udp_key(1)));
    }

    #[test]
    fn eviction_prefers_udp_over_an_established_tcp_flow() {
        let mut table: HashMap<NatKey, NatEntry> = HashMap::new();
        table.insert(udp_key(1), entry(9_000, false));
        table.insert(tcp_key(2), entry(1_000, true));

        assert_eq!(evict_one(&mut table, 10_000), Some(udp_key(1)));
        assert!(table.contains_key(&tcp_key(2)));
    }

    #[test]
    fn eviction_prefers_a_closing_tcp_flow_over_an_established_one() {
        let mut table: HashMap<NatKey, NatEntry> = HashMap::new();
        table.insert(tcp_key(1), entry(9_000, false));
        table.insert(tcp_key(2), entry(1_000, true));

        assert_eq!(evict_one(&mut table, 10_000), Some(tcp_key(1)));
        assert!(table.contains_key(&tcp_key(2)));
    }

    #[test]
    fn only_the_configured_stats_socket_is_the_target() {
        let configured = Some("10.233.0.1:9798".parse().unwrap());
        assert!(is_stats_target(
            Ipv4Addr::new(10, 233, 0, 1).into(),
            9798,
            configured
        ));
        assert!(!is_stats_target(
            Ipv4Addr::new(10, 233, 0, 1).into(),
            9797,
            configured
        ));
        assert!(!is_stats_target(
            Ipv4Addr::new(10, 233, 0, 2).into(),
            9798,
            configured
        ));
        assert!(!is_stats_target(
            Ipv4Addr::new(10, 233, 0, 1).into(),
            9798,
            None
        ));
    }

    #[test]
    fn the_stats_window_allows_two_requests_per_second() {
        let mut window = StatsWindow::default();
        assert!(window.allow(0));
        assert!(window.allow(0));
        assert!(!window.allow(0), "the third immediate request is dropped");
        assert!(window.allow(1_000));
        assert!(window.allow(1_000));
        assert!(!window.allow(1_000));
    }

    fn udp_request() -> BytesMut {
        use smoltcp::phy::{Checksum, ChecksumCapabilities};
        use smoltcp::wire::{IpRepr, UdpRepr};

        let mut checksummer = ChecksumCapabilities::ignored();
        checksummer.udp = Checksum::Tx;
        checksummer.ipv4 = Checksum::Tx;
        let src: IpAddress = Ipv4Addr::new(10, 9, 9, 2).into();
        let dst: IpAddress = Ipv4Addr::new(10, 233, 0, 1).into();
        let body = b"JFS1 1 summary";
        let ip = IpRepr::new(src, dst, IpProtocol::Udp, body.len() + 8, 64);
        let mut buf = BytesMut::new();
        buf.resize(ip.buffer_len(), 0);
        match ip {
            IpRepr::Ipv4(r) => {
                let mut packet = Ipv4Packet::new_unchecked(&mut buf[..]);
                r.emit(&mut packet, &checksummer);
                let mut udp = UdpPacket::new_unchecked(packet.payload_mut());
                UdpRepr {
                    src_port: 5000,
                    dst_port: 9798,
                }
                .emit(
                    &mut udp,
                    &src,
                    &dst,
                    body.len(),
                    |payload| payload.copy_from_slice(body),
                    &checksummer,
                );
            }
            IpRepr::Ipv6(_) => unreachable!("this test builds IPv4"),
        }
        buf
    }

    #[test]
    fn a_stats_reply_swaps_the_udp_endpoints_and_keeps_the_body() {
        let request = udp_request();
        let body = b"JFS1 1 summary v=1 net=wifi";
        let reply = serve_stats::reply(&request, body).unwrap();
        let packet = Ipv4Packet::new_checked(&reply[..]).unwrap();
        assert_eq!(packet.src_addr(), Ipv4Addr::new(10, 233, 0, 1).into());
        assert_eq!(packet.dst_addr(), Ipv4Addr::new(10, 9, 9, 2).into());
        let udp = UdpPacket::new_checked(packet.payload()).unwrap();
        assert_eq!(udp.src_port(), 9798);
        assert_eq!(udp.dst_port(), 5000);
        assert_eq!(udp.payload(), body);
        let src: IpAddress = packet.src_addr().into();
        let dst: IpAddress = packet.dst_addr().into();
        assert!(udp.verify_checksum(&src, &dst));
    }

    #[test]
    fn a_stats_reply_is_capped() {
        let request = udp_request();
        assert!(serve_stats::reply(&request, &[b'x'; 2049]).is_err());
    }
}
