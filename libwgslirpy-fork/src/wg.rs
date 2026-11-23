//! Simplified interface to `boringtun` based on Tokio's mpsc channels.

use std::{
    net::SocketAddr,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use base64::Engine;
use boringtun::noise::TunnResult;
pub use boringtun::x25519::{PublicKey, StaticSecret};
use bytes::BytesMut;
use tokio::io::unix::AsyncFd;
use tokio::sync::mpsc::{Receiver, Sender};
use tracing::{error, warn};

use crate::TEAR_OF_ALLOCATION_SIZE;

/// Maximum size of an IP packet carried over the TUN device (WireGuard MTU).
pub const TUN_MTU: usize = 1280;

/// Read/write wrapper over a raw TUN file descriptor.
///
/// Enforces the [`TUN_MTU`] bound on both directions; larger buffers or packets
/// are rejected with [`std::io::ErrorKind::InvalidInput`].
pub struct TunDevice {
    fd: std::os::unix::io::RawFd,
}

impl TunDevice {
    /// Wrap an existing raw file descriptor (e.g. a VpnService TUN fd).
    ///
    /// The fd is switched to non-blocking mode so it can be driven by
    /// [`tokio::io::unix::AsyncFd`]; callers must not rely on it staying
    /// blocking afterwards.
    pub fn new(fd: std::os::unix::io::RawFd) -> Self {
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        if flags >= 0 {
            unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) };
        }
        Self { fd }
    }

    /// Read one packet into `buf`; errors if `buf` exceeds [`TUN_MTU`].
    pub fn read(&self, buf: &mut [u8]) -> std::io::Result<usize> {
        if buf.len() > TUN_MTU {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("buffer larger than TUN_MTU {}", TUN_MTU),
            ));
        }
        let n = unsafe { libc::read(self.fd, buf.as_mut_ptr() as *mut libc::c_void, buf.len()) };
        if n < 0 {
            Err(std::io::Error::last_os_error())
        } else {
            Ok(n as usize)
        }
    }

    /// Write one packet; errors if the packet exceeds [`TUN_MTU`].
    pub fn write(&self, buf: &[u8]) -> std::io::Result<usize> {
        if buf.len() > TUN_MTU {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("packet larger than TUN_MTU {}", TUN_MTU),
            ));
        }
        let n = unsafe { libc::write(self.fd, buf.as_ptr() as *const libc::c_void, buf.len()) };
        if n < 0 {
            Err(std::io::Error::last_os_error())
        } else {
            Ok(n as usize)
        }
    }
}

impl std::os::unix::io::AsRawFd for TunDevice {
    fn as_raw_fd(&self) -> std::os::unix::io::RawFd {
        self.fd
    }
}

/// Live transfer counters, UDP diagnostics and the current endpoint facts.
///
/// Diagnostics are additive: the run loop updates this snapshot at the existing
/// UDP send/recv and timer points, without changing a return, a timeout or a
/// packet-processing branch. `-1` means "not known yet".
#[derive(Debug, Clone)]
pub struct WgStats {
    /// Encrypted WireGuard data bytes accepted by boringtun / sent to the peer.
    pub rx_bytes: u64,
    /// Encrypted WireGuard data bytes sent to the peer.
    pub tx_bytes: u64,
    /// Raw UDP datagrams received on the tunnel socket.
    pub udp_rx_packets: u64,
    /// Raw UDP payload bytes received on the tunnel socket.
    pub udp_rx_bytes: u64,
    /// Raw UDP datagrams sent on the tunnel socket.
    pub udp_tx_packets: u64,
    /// Raw UDP payload bytes sent on the tunnel socket.
    pub udp_tx_bytes: u64,
    /// Failed UDP sends.
    pub send_errors: u64,
    /// Failed UDP receives.
    pub recv_errors: u64,
    /// Errno of the most recent UDP error, or -1 when none was seen.
    pub last_errno: i32,
    /// Wall-clock epoch milliseconds of the last successful datagram.
    pub last_rx_ms: i64,
    /// Wall-clock epoch milliseconds of the last successful send.
    pub last_tx_ms: i64,
    /// Local socket address as `ip:port`; empty when unknown.
    pub local_addr: String,
    /// Current peer socket address as `ip:port`; empty when unknown.
    pub peer_addr: String,
    /// Milliseconds since boringtun's last handshake, or -1 before one.
    pub handshake_age_ms: i64,
    /// boringtun's estimate of the peer's packets lost on the way, in
    /// thousandths, or -1 before there is one.
    pub est_loss_permille: i32,
    /// The round trip of the last handshake in milliseconds, or -1.
    pub handshake_rtt_ms: i64,
}

impl Default for WgStats {
    fn default() -> Self {
        Self {
            rx_bytes: 0,
            tx_bytes: 0,
            udp_rx_packets: 0,
            udp_rx_bytes: 0,
            udp_tx_packets: 0,
            udp_tx_bytes: 0,
            send_errors: 0,
            recv_errors: 0,
            last_errno: -1,
            last_rx_ms: -1,
            last_tx_ms: -1,
            local_addr: String::new(),
            peer_addr: String::new(),
            handshake_age_ms: -1,
            est_loss_permille: -1,
            handshake_rtt_ms: -1,
        }
    }
}

type StatsHandle = Option<std::sync::Arc<std::sync::Mutex<WgStats>>>;

/// Epoch milliseconds, or -1 if the system clock is before the epoch.
fn epoch_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(-1)
}

/// Run `update` on the shared stats, ignoring a poisoned or absent handle.
fn with_stats(stats: &StatsHandle, update: impl FnOnce(&mut WgStats)) {
    if let Some(s) = stats {
        if let Ok(mut s) = s.lock() {
            update(&mut s);
        }
    }
}

fn record_udp_rx(stats: &StatsHandle, len: usize) {
    with_stats(stats, |s| {
        s.udp_rx_packets += 1;
        s.udp_rx_bytes += len as u64;
        s.last_rx_ms = epoch_ms();
    });
}

fn record_udp_tx(stats: &StatsHandle, len: usize) {
    with_stats(stats, |s| {
        s.udp_tx_packets += 1;
        s.udp_tx_bytes += len as u64;
        s.tx_bytes += len as u64;
        s.last_tx_ms = epoch_ms();
    });
}

fn record_send_error(stats: &StatsHandle, err: &std::io::Error) {
    let errno = err.raw_os_error().unwrap_or(-1);
    with_stats(stats, |s| {
        s.send_errors += 1;
        s.last_errno = errno;
    });
}

fn record_recv_error(stats: &StatsHandle, err: &std::io::Error) {
    let errno = err.raw_os_error().unwrap_or(-1);
    with_stats(stats, |s| {
        s.recv_errors += 1;
        s.last_errno = errno;
    });
}

fn record_local_addr(stats: &StatsHandle, addr: SocketAddr) {
    with_stats(stats, |s| s.local_addr = addr.to_string());
}

fn record_peer_addr(stats: &StatsHandle, addr: Option<SocketAddr>) {
    if let Some(addr) = addr {
        with_stats(stats, |s| s.peer_addr = addr.to_string());
    }
}

/// A packet that boringtun accepted authenticates its source, so follow a peer
/// that moved even when the config named an endpoint. Anything else (a forged
/// packet, or a timer/tun event with no source) keeps the current endpoint.
fn roam_peer_addr(
    current: Option<SocketAddr>,
    seen: Option<SocketAddr>,
    authenticated: bool,
) -> Option<SocketAddr> {
    if authenticated {
        seen.or(current)
    } else {
        current
    }
}

/// Record boringtun's handshake age, loss estimate and handshake round trip
/// at the existing loop point (no timer).
fn record_handshake_age(stats: &StatsHandle, wg: &boringtun::noise::Tunn) {
    let (age, _, _, loss, rtt) = wg.stats();
    let age_ms = age.map(|d| d.as_millis() as i64).unwrap_or(-1);
    let loss_permille = if loss.is_finite() {
        (loss.clamp(0.0, 1.0) * 1000.0).round() as i32
    } else {
        -1
    };
    let rtt_ms = rtt.map(i64::from).unwrap_or(-1);
    with_stats(stats, |s| {
        s.handshake_age_ms = age_ms;
        s.est_loss_permille = loss_permille;
        s.handshake_rtt_ms = rtt_ms;
    });
}

/// Add `len` received bytes to the stats, unless boringtun rejected the
/// datagram. Garbage, forged or replayed datagrams come back as `Err`; anything
/// else (handshake reply, decrypted data, an authenticated keepalive) proves the
/// peer's WireGuard traffic arrived.
fn count_rx(
    stats: &Option<std::sync::Arc<std::sync::Mutex<WgStats>>>,
    len: usize,
    tr: &TunnResult,
) {
    // `Done` must count: an authenticated keepalive decapsulates to `Done`, and
    // on an idle tunnel it is the only traffic (every 25 s).
    if matches!(tr, TunnResult::Err(_)) {
        return;
    }
    if let Some(s) = stats {
        if let Ok(mut s) = s.lock() {
            s.rx_bytes += len as u64;
        }
    }
}

/// Options for being a Wireguard peer
pub struct Opts {
    /// Private key of this Wireguard node.
    /// 
    /// Use `parsebase64_32` and `into()` to specify it from a string.
    pub private_key: StaticSecret,


    /// Public key of single Wireguard peer we are connecting to.
    /// 
    /// Use `parsebase64_32` and `into()` to specify it from a string.
    pub peer_key: PublicKey,

    /// Static socket address of Wireguard peer to connect to.
    /// 
    /// If it is missing, it listens for incoming datagrams and remembers last seen `from` address
    /// (data from which Wireguard implementation recognized) for `sendto` purposes.
    pub peer_endpoint: Option<SocketAddr>,

    /// How often to send keepalive packets to the peer.
    pub keepalive_interval: Option<u16>,

    /// Socket address to bind local UDP port to.
    pub bind_ip_port: SocketAddr,

    /// Optional live transfer counters. When set, the run loop increments
    /// rx/tx byte totals after every datagram; the caller can poll them for
    /// live transfer display without touching the loop's internals.
    pub stats: Option<std::sync::Arc<std::sync::Mutex<WgStats>>>,
}

impl Opts {
    /// Start Wireguard implementation using this options set.
    /// 
    /// Received IP packets would appear at `tx_fromwg`'s channel.
    /// IP packets to be sent to Wireguard tunnel is to be written to `rx_towg`'s channel.
    pub async fn start(
        &self,
        tx_fromwg: Sender<BytesMut>,
        mut rx_towg: Receiver<BytesMut>,
    ) -> anyhow::Result<()> {
        let mut wg = boringtun::noise::Tunn::new(
            self.private_key.clone(),
            self.peer_key,
            None,
            self.keepalive_interval,
            0,
            None,
        );

        let udp = tokio::net::UdpSocket::bind(self.bind_ip_port).await?;

        let mut current_peer_addr = self.peer_endpoint;

        let mut each_second = tokio::time::interval(Duration::from_secs(1));
        let mut udp_recv_buf = [0; 4096 - 32];
        let mut wg_scratch_buf = [0; 4096];
        let mut tear_off_buffer = BytesMut::with_capacity(TEAR_OF_ALLOCATION_SIZE);
        let stats = self.stats.clone();
        if let Ok(addr) = udp.local_addr() {
            record_local_addr(&stats, addr);
        }
        record_peer_addr(&stats, current_peer_addr);
        loop {
            let mut last_seen_recv_address = None;
            let mut tr: Option<TunnResult> = tokio::select! {
                _instant = each_second.tick() => {
                    Some(wg.update_timers(&mut wg_scratch_buf))
                }
                ret = udp.recv_from(&mut udp_recv_buf[..]) => {
                    let ret = ret.inspect_err(|e| {
                        record_recv_error(&stats, e);
                    })?;
                    let buf : &[u8] = &udp_recv_buf[0..(ret.0)];
                    let from : SocketAddr = ret.1;
                    last_seen_recv_address = Some(from);
                    record_udp_rx(&stats, ret.0);

                    let tr = wg.decapsulate(None, buf, &mut wg_scratch_buf);
                    count_rx(&stats, ret.0, &tr);
                    Some(tr)
                }
                ret = rx_towg.recv() => {
                    let Some(incoming) : Option<BytesMut> = ret else {
                        warn!("Finished possible packets into wg");
                        break
                    };
                    Some(wg.encapsulate(&incoming[..], &mut wg_scratch_buf))
                }
            };
            loop {
                if let Some(tr_inner) = tr {
                    let next = roam_peer_addr(
                        current_peer_addr,
                        last_seen_recv_address,
                        !matches!(tr_inner, TunnResult::Err(..)),
                    );
                    if next != current_peer_addr {
                        current_peer_addr = next;
                        record_peer_addr(&stats, current_peer_addr);
                    }
                    match tr_inner {
                        TunnResult::Done => (),
                        TunnResult::Err(e) => {
                            error!("boringturn error: {:?}", e);
                        }
                        TunnResult::WriteToNetwork(b) => {
                            if let Some(cpa) = current_peer_addr {
                                match udp.send_to(b, cpa).await {
                                    Ok(n) => record_udp_tx(&stats, n),
                                    Err(e) => {
                                        record_send_error(&stats, &e);
                                        error!("Failed to send wiregaurd packet to peer: {e}")
                                    }
                                }
                                tr = Some(wg.decapsulate(None, b"", &mut wg_scratch_buf));
                                continue;
                            } else {
                                error!("Trying to send a wireguard packet without configured peer address");
                            }
                        }
                        TunnResult::WriteToTunnelV4(b, _) | TunnResult::WriteToTunnelV6(b, _) => {
                            tear_off_buffer.extend_from_slice(b);
                            tx_fromwg.send(tear_off_buffer.split()).await?;
                            if tear_off_buffer.capacity() < 2048 {
                                tear_off_buffer = BytesMut::with_capacity(TEAR_OF_ALLOCATION_SIZE);
                            }
                        }
                    }
                }
                break;
            }
            record_handshake_age(&stats, &wg);
        }
        #[allow(unreachable_code)]
        Ok::<_, anyhow::Error>(())
    }

    /// Client-path WireGuard loop over a TUN fd instead of the smoltcp channel:
    /// UDP <-> boringtun <-> fd. No router involvement on this path.
    pub async fn start_tun(&self, tun_fd: std::os::unix::io::RawFd) -> anyhow::Result<()> {
        let mut wg = boringtun::noise::Tunn::new(
            self.private_key.clone(),
            self.peer_key,
            None,
            self.keepalive_interval,
            0,
            None,
        );
        let udp = tokio::net::UdpSocket::bind(self.bind_ip_port).await?;
        // SAFETY: the fd is owned by the caller (VpnService) and stays open
        // for the whole AsyncFd lifetime; TunDevice never closes it and always
        // reports the same raw fd.
        let tun = unsafe { AsyncFd::register(TunDevice::new(tun_fd))? };
        let mut current_peer_addr = self.peer_endpoint;
        let mut each_second = tokio::time::interval(Duration::from_secs(1));
        let mut udp_recv_buf = [0; 4096 - 32];
        let mut wg_scratch_buf = [0; 4096];
        let mut tun_buf = [0; TUN_MTU];
        let stats = self.stats.clone();
        if let Ok(addr) = udp.local_addr() {
            record_local_addr(&stats, addr);
        }
        record_peer_addr(&stats, current_peer_addr);
        loop {
            let mut last_seen_recv_address = None;
            let mut tr: Option<TunnResult> = tokio::select! {
                _instant = each_second.tick() => Some(wg.update_timers(&mut wg_scratch_buf)),
                ret = udp.recv_from(&mut udp_recv_buf[..]) => {
                    let (n, from) = ret.inspect_err(|e| {
                        record_recv_error(&stats, e);
                    })?;
                    last_seen_recv_address = Some(from);
                    record_udp_rx(&stats, n);
                    let tr = wg.decapsulate(None, &udp_recv_buf[..n], &mut wg_scratch_buf);
                    count_rx(&stats, n, &tr);
                    Some(tr)
                }
                ret = tun.readable() => {
                    let mut guard = ret?;
                    let n = match tun.try_io(tokio::io::Interest::READABLE, |tun: &TunDevice| {
                        tun.read(&mut tun_buf)
                    }) {
                        Ok(n) => n,
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            guard.clear_ready();
                            continue;
                        }
                        Err(e) => return Err(e.into()),
                    };
                    Some(wg.encapsulate(&tun_buf[..n], &mut wg_scratch_buf))
                }
            };
            loop {
                if let Some(tr_inner) = tr {
                    let next = roam_peer_addr(
                        current_peer_addr,
                        last_seen_recv_address,
                        !matches!(tr_inner, TunnResult::Err(..)),
                    );
                    if next != current_peer_addr {
                        current_peer_addr = next;
                        record_peer_addr(&stats, current_peer_addr);
                    }
                    match tr_inner {
                        TunnResult::Done => (),
                        TunnResult::Err(e) => error!("wg error: {:?}", e),
                        TunnResult::WriteToNetwork(b) => {
                            if let Some(cpa) = current_peer_addr {
                                match udp.send_to(b, cpa).await {
                                    Ok(n) => record_udp_tx(&stats, n),
                                    Err(e) => {
                                        record_send_error(&stats, &e);
                                        error!("send_to failed: {}", e);
                                    }
                                }
                                tr = Some(wg.decapsulate(None, b"", &mut wg_scratch_buf));
                                continue;
                            }
                            error!("Trying to send a wireguard packet without configured peer address");
                        }
                        TunnResult::WriteToTunnelV4(b, _) | TunnResult::WriteToTunnelV6(b, _) => {
                            let mut attempts = 0;
                            loop {
                                match tun.try_io(tokio::io::Interest::WRITABLE, |tun: &TunDevice| {
                                    tun.write(b)
                                }) {
                                    Ok(_n) => break,
                                    Err(e)
                                        if e.kind() == std::io::ErrorKind::WouldBlock && attempts < 3 =>
                                    {
                                        tokio::time::sleep(Duration::from_millis(1)).await;
                                        attempts += 1;
                                    }
                                    Err(e) => {
                                        error!("tun write failed: {}", e);
                                        break;
                                    }
                                }
                            }
                        }
                    }
                }
                break;
            }
            record_handshake_age(&stats, &wg);
        }
    }
}

/// Helper funtion to simplify creating [`StaticSecret`]s and [`PublicKey`]s.
pub fn parsebase64_32(x: &str) -> anyhow::Result<[u8; 32]> {
    let b = base64::engine::general_purpose::STANDARD.decode(x)?;
    Ok(b[..].try_into()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::io::RawFd;
    use std::sync::{Arc, Mutex};

    fn pipe_pair() -> (RawFd, RawFd) {
        let mut fds = [0 as libc::c_int; 2];
        let rc = unsafe { libc::pipe(fds.as_mut_ptr()) };
        assert_eq!(rc, 0, "pipe() failed: {}", std::io::Error::last_os_error());
        (fds[0], fds[1])
    }

    #[test]
    fn tun_write_reaches_other_pipe_end() {
        let (read_fd, write_fd) = pipe_pair();
        let tun = TunDevice::new(write_fd);
        let payload = b"hello tun";
        let n = tun.write(payload).unwrap();
        assert_eq!(n, payload.len());
        let mut got = [0u8; 16];
        let m = unsafe { libc::read(read_fd, got.as_mut_ptr() as *mut libc::c_void, got.len()) };
        assert_eq!(m as usize, payload.len());
        assert_eq!(&got[..payload.len()], payload);
        unsafe { libc::close(read_fd); libc::close(write_fd); }
    }

    #[test]
    fn tun_read_gets_bytes_written_by_other_end() {
        let (read_fd, write_fd) = pipe_pair();
        let tun = TunDevice::new(read_fd);
        let payload = b"hello tun";
        let n = unsafe { libc::write(write_fd, payload.as_ptr() as *const libc::c_void, payload.len()) };
        assert_eq!(n as usize, payload.len());
        let mut buf = [0u8; 16];
        let m = tun.read(&mut buf).unwrap();
        assert_eq!(m, payload.len());
        assert_eq!(&buf[..payload.len()], payload);
        unsafe { libc::close(read_fd); libc::close(write_fd); }
    }

    #[test]
    fn tun_write_rejects_packets_over_mtu() {
        let (read_fd, write_fd) = pipe_pair();
        let tun = TunDevice::new(write_fd);
        let big = vec![0u8; TUN_MTU + 1];
        let err = tun.write(&big).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
        unsafe { libc::close(read_fd); libc::close(write_fd); }
    }

    #[test]
    fn tun_read_rejects_buffers_over_mtu() {
        let (read_fd, write_fd) = pipe_pair();
        let tun = TunDevice::new(read_fd);
        let mut big = vec![0u8; TUN_MTU + 1];
        let err = tun.read(&mut big).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
        unsafe { libc::close(read_fd); libc::close(write_fd); }
    }

    #[tokio::test]
    async fn start_tun_loop_survives_garbage_tun_input() {
        let mut fds = [0 as libc::c_int; 2];
        assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0, "pipe() failed");

        let opts = Opts {
            private_key: StaticSecret::from([7u8; 32]),
            peer_key: PublicKey::from([9u8; 32]),
            peer_endpoint: Some("127.0.0.1:9797".parse().unwrap()),
            keepalive_interval: Some(25),
            bind_ip_port: "127.0.0.1:0".parse().unwrap(),
            stats: None,
        };
        let handle: tokio::task::JoinHandle<anyhow::Result<()>> =
            tokio::spawn(async move { opts.start_tun(fds[0]).await });

        let garbage = b"not a wireguard packet".to_vec();
        let n = unsafe { libc::write(fds[1], garbage.as_ptr() as *const libc::c_void, garbage.len()) };
        assert_eq!(n as usize, garbage.len());

        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(!handle.is_finished(), "start_tun loop must survive garbage tun input");
        handle.abort();
        let _ = handle.await;
        unsafe { libc::close(fds[0]); libc::close(fds[1]); }
    }

    // Lab: rx_bytes was bumped before decapsulate, so any UDP
    // datagram to the WG port (a stranger, garbage) counted as "peer data" and
    // made the sharer report Connected. Only datagrams boringtun accepted count.
    fn ipv4_packet() -> [u8; 20] {
        [0x45, 0, 0, 20, 0, 0, 0, 0, 64, 17, 0, 0, 10, 0, 0, 1, 10, 0, 0, 2]
    }

    fn rx(stats: &Arc<Mutex<WgStats>>) -> u64 {
        stats.lock().unwrap().rx_bytes
    }

    // Diagnostics only: a fresh counter set is honest about what it does not
    // know yet (no datagram, no handshake, no addresses) instead of a fake 0.
    #[test]
    fn stats_defaults_mark_unavailable_diagnostics_as_minus_one() {
        let s = WgStats::default();
        assert_eq!(s.rx_bytes, 0);
        assert_eq!(s.tx_bytes, 0);
        assert_eq!(s.udp_rx_packets, 0);
        assert_eq!(s.udp_rx_bytes, 0);
        assert_eq!(s.udp_tx_packets, 0);
        assert_eq!(s.udp_tx_bytes, 0);
        assert_eq!(s.send_errors, 0);
        assert_eq!(s.recv_errors, 0);
        assert_eq!(s.last_errno, -1);
        assert_eq!(s.last_rx_ms, -1);
        assert_eq!(s.last_tx_ms, -1);
        assert_eq!(s.handshake_age_ms, -1);
        assert!(s.local_addr.is_empty());
        assert!(s.peer_addr.is_empty());
    }

    #[test]
    fn rx_counts_only_datagrams_boringtun_accepted() {
        let a_sk = StaticSecret::from([1u8; 32]);
        let b_sk = StaticSecret::from([2u8; 32]);
        let (a_pk, b_pk) = (PublicKey::from(&a_sk), PublicKey::from(&b_sk));
        let mut a = boringtun::noise::Tunn::new(a_sk, b_pk, None, None, 0, None);
        let mut b = boringtun::noise::Tunn::new(b_sk, a_pk, None, None, 1, None);
        let stats = Arc::new(Mutex::new(WgStats::default()));
        let some = Some(stats.clone());
        let (mut b1, mut b2) = ([0u8; 2048], [0u8; 2048]);

        // Garbage and a stranger's handshake: never counted.
        let junk = [0xABu8; 148];
        let r = b.decapsulate(None, &junk, &mut b2);
        count_rx(&some, junk.len(), &r);
        let mut forged = [0u8; 148];
        forged[0] = 1; // handshake-initiation type, invalid body/MAC
        let r = b.decapsulate(None, &forged, &mut b2);
        count_rx(&some, forged.len(), &r);
        assert_eq!(rx(&stats), 0, "unauthenticated datagrams were counted");

        // Real handshake: initiation -> response -> keepalive.
        let init = match a.encapsulate(&ipv4_packet(), &mut b1) {
            TunnResult::WriteToNetwork(p) => p.to_vec(),
            other => panic!("no initiation: {other:?}"),
        };
        let resp = match b.decapsulate(None, &init, &mut b2) {
            TunnResult::WriteToNetwork(p) => {
                count_rx(&some, init.len(), &TunnResult::WriteToNetwork(&mut []));
                p.to_vec()
            }
            other => panic!("no response: {other:?}"),
        };
        let after_init = rx(&stats);
        assert_eq!(after_init, init.len() as u64, "valid initiation not counted");

        let keepalive = match a.decapsulate(None, &resp, &mut b1) {
            TunnResult::WriteToNetwork(p) => p.to_vec(),
            other => panic!("no keepalive: {other:?}"),
        };

        // Idle tunnel: the peer's keepalive is the only traffic for ~25 s, so
        // it must count (else the 35 s stall watch kills healthy tunnels).
        let r = b.decapsulate(None, &keepalive, &mut b2);
        count_rx(&some, keepalive.len(), &r);
        assert_eq!(
            rx(&stats),
            after_init + keepalive.len() as u64,
            "authenticated keepalive not counted ({r:?})"
        );

        // Decrypted data counts too.
        let data = match a.encapsulate(&ipv4_packet(), &mut b1) {
            TunnResult::WriteToNetwork(p) => p.to_vec(),
            other => panic!("no data packet: {other:?}"),
        };
        let before = rx(&stats);
        let r = b.decapsulate(None, &data, &mut b2);
        assert!(matches!(r, TunnResult::WriteToTunnelV4(..)), "{r:?}");
        count_rx(&some, data.len(), &r);
        assert_eq!(rx(&stats), before + data.len() as u64);

        // A replay of that very packet is rejected, so it is not counted.
        let r = b.decapsulate(None, &data, &mut b2);
        count_rx(&some, data.len(), &r);
        assert_eq!(rx(&stats), before + data.len() as u64, "replay counted ({r:?})");
    }

    #[test]
    fn an_authenticated_packet_roams_even_a_configured_endpoint() {
        let old: SocketAddr = "192.168.43.2:9797".parse().unwrap();
        let new: SocketAddr = "192.168.44.2:9797".parse().unwrap();
        assert_eq!(roam_peer_addr(Some(old), Some(new), true), Some(new));
        assert_eq!(roam_peer_addr(Some(old), Some(new), false), Some(old));
        assert_eq!(roam_peer_addr(Some(old), None, true), Some(old));
        assert_eq!(roam_peer_addr(None, Some(new), true), Some(new));
    }
}
