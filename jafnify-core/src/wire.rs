//! The 9799 socket shared by the discovery, handshake, known-peer and echo
//! endpoints of one process.
//!
//! Each endpoint used to bind its own socket with SO_REUSEPORT. Multicast is
//! copied to all of them, but the kernel delivers a UNICAST datagram to only
//! one (chosen by a hash of the source address): a unicast 0x02 could land on
//! the 0x01 socket and be lost there. So one socket per (port, interfaces) is
//! kept per process, and every datagram it reads is handed to each endpoint
//! that takes its frame type. Within a process that is what multicast already
//! did; it now also holds for unicast.
//!
//! Limit: two processes on one host with the same port are still two sockets,
//! so a unicast frame reaches only one of them.

use std::collections::VecDeque;
use std::io;
use std::mem;
use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
use std::ops::Deref;
#[cfg(unix)]
use std::os::fd::AsRawFd;
#[cfg(windows)]
use std::os::windows::io::AsRawSocket;
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::{Duration, Instant};

#[cfg(windows)]
use std::sync::OnceLock;
#[cfg(windows)]
use windows_sys::core::GUID;
#[cfg(windows)]
use windows_sys::Win32::Networking::WinSock::{
    setsockopt, WSAGetLastError, WSAIoctl, IN_PKTINFO, IPPROTO_IP, IP_PKTINFO,
    LPWSAOVERLAPPED_COMPLETION_ROUTINE, SIO_GET_EXTENSION_FUNCTION_POINTER, SOCKADDR_IN, SOCKET,
    WSABUF, WSAID_WSARECVMSG, WSAMSG,
};
#[cfg(windows)]
use windows_sys::Win32::System::IO::OVERLAPPED;

use crate::discovery::{bind_multicast, send_multicast, DiscoveryConfig, DiscoveryError};

/// Additive counters of one shared control socket. They are diagnostics only:
/// updating them never changes a send/recv result, timeout or error.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct HubStats {
    pub tx_packets: u64,
    pub tx_bytes: u64,
    pub rx_packets: u64,
    pub rx_bytes: u64,
    pub send_errors: u64,
    pub recv_errors: u64,
    /// The last `errno` seen by a send/recv on this socket; 0 = no error yet.
    pub last_errno: i32,
    pub last_rx: Option<Instant>,
    pub last_tx: Option<Instant>,
}

/// How a datagram reached this device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Path {
    /// Sent to the discovery group.
    Multicast,
    /// Sent to this device's address directly.
    Unicast,
}

/// Which frames an endpoint takes. Frame type is the first byte.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Accept {
    /// 0x01: 49 bytes.
    Discovery,
    /// 0x04: 67 bytes (key, port, MAC).
    Handshake,
    /// 0x02: 65 bytes, the version-1 handshake (only to report an old peer).
    LegacyHandshake,
    /// 0x03: any length (the consumer decodes and checks).
    Known,
    /// 0x05 (52 bytes) or 0x06 (101 bytes): PAKE pairing.
    Pake,
    /// Everything (the echo watch looks at the sender only).
    Any,
}

impl Accept {
    fn takes(self, frame: &[u8]) -> bool {
        match self {
            Accept::Discovery => frame.len() == 49 && frame[0] == 0x01,
            Accept::Handshake => frame.len() == 67 && frame[0] == 0x04,
            Accept::LegacyHandshake => frame.len() == 65 && frame[0] == 0x02,
            Accept::Known => frame.first() == Some(&0x03),
            Accept::Pake => {
                (frame.len() == 52 && frame[0] == 0x05) || (frame.len() == 101 && frame[0] == 0x06)
            }
            Accept::Any => true,
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct Datagram {
    pub from: SocketAddr,
    /// The address it was sent to (IP_PKTINFO): tells the interface it came in
    /// on. `Ipv4Addr::UNSPECIFIED` means the platform could not report it; the
    /// ingress rule then treats the datagram as addressed to us.
    pub to: Ipv4Addr,
    pub path: Path,
    pub frame: Vec<u8>,
}

/// Whether a pinned device takes a datagram: the source is private or loopback
/// (a LAN peer, never the cellular network) and it was sent to the group or to
/// one of the pinned interface addresses. Unpinned devices take everything.
fn admits(d: &Datagram, egress: &[Ipv4Addr]) -> bool {
    if egress.is_empty() {
        return true;
    }
    let SocketAddr::V4(src) = d.from else {
        return false;
    };
    let o = src.ip().octets();
    let carrier_nat = o[0] == 100 && (o[1] & 0xc0) == 0x40;
    let from_lan = src.ip().is_private() || src.ip().is_loopback() || carrier_nat;
    // An unknown destination counts as ours only on Windows, whose fallback
    // (see `recv_datagram`) cannot report it; the source rule above still keeps
    // non-LAN senders out. On Unix a missing IP_PKTINFO stays a reject.
    let unknown_ok = cfg!(windows) && d.to.is_unspecified();
    let to_us = unknown_ok || d.to.is_multicast() || egress.contains(&d.to);
    from_lan && to_us
}

/// Frames queued per endpoint before the oldest are dropped. Protocol frames
/// are small and the caller reads them every few milliseconds.
const INBOX_CAP: usize = 256;
/// Verified peer addresses remembered per socket.
const LEARNED_CAP: usize = 16;
/// Poll step while an endpoint waits for a frame.
const POLL: Duration = Duration::from_millis(5);

type Inbox = Arc<Mutex<VecDeque<Datagram>>>;

struct Hub {
    socket: UdpSocket,
    group: SocketAddr,
    egress: Vec<Ipv4Addr>,
    ends: Vec<(Accept, Weak<Mutex<VecDeque<Datagram>>>)>,
    /// Source addresses of verified frames: answered by unicast from now on.
    learned: VecDeque<SocketAddr>,
    /// Diagnostic counters, guarded by the same mutex as the socket itself.
    stats: HubStats,
}

type HubKey = (SocketAddr, u8, Vec<Ipv4Addr>);

/// Live hubs of this process, by key. A hub goes away with its last endpoint.
static HUBS: Mutex<Vec<(HubKey, Weak<Mutex<Hub>>)>> = Mutex::new(Vec::new());

/// Take the live hub for `key`. Pruning by `strong_count` is only a snapshot:
/// another thread can drop the last `Arc` between that check and `upgrade`, so
/// a dead entry is removed here instead of panicking.
fn live_hub<T>(hubs: &mut Vec<(HubKey, Weak<Mutex<T>>)>, key: &HubKey) -> Option<Arc<Mutex<T>>> {
    let index = hubs.iter().position(|(k, _)| k == key)?;
    match hubs[index].1.upgrade() {
        Some(hub) => Some(hub),
        None => {
            // The entry outlived its hub: replace it with a fresh one.
            hubs.remove(index);
            None
        }
    }
}

/// One consumer's view of the shared socket.
pub(crate) struct Endpoint {
    hub: Arc<Mutex<Hub>>,
    inbox: Inbox,
    /// Extra unicast targets of this endpoint (gateways), on the group port.
    unicast: Vec<Ipv4Addr>,
}

impl Endpoint {
    pub(crate) fn open(cfg: &DiscoveryConfig, accept: Accept) -> Result<Self, DiscoveryError> {
        let key: HubKey = (cfg.addr, cfg.ttl, cfg.interfaces());
        let mut hubs = HUBS.lock().unwrap_or_else(|e| e.into_inner());
        hubs.retain(|(_, w)| w.strong_count() > 0);
        let hub = match live_hub(&mut hubs, &key) {
            Some(hub) => hub,
            None => {
                let (socket, egress) = bind_multicast(cfg)?;
                let hub = Arc::new(Mutex::new(Hub {
                    socket,
                    group: cfg.addr,
                    egress,
                    ends: Vec::new(),
                    learned: VecDeque::new(),
                    stats: HubStats::default(),
                }));
                hubs.push((key, Arc::downgrade(&hub)));
                hub
            }
        };
        drop(hubs);
        let inbox: Inbox = Arc::new(Mutex::new(VecDeque::new()));
        {
            let mut h = lock(&hub);
            // Datagrams that arrived before this endpoint existed are not its:
            // drain them to the existing endpoints first.
            h.drain()?;
            h.ends.push((accept, Arc::downgrade(&inbox)));
        }
        Ok(Self {
            hub,
            inbox,
            unicast: cfg.unicast.clone(),
        })
    }

    /// The group (once per egress interface) and every unicast target.
    pub(crate) fn send(&self, frame: &[u8]) -> io::Result<()> {
        let mut guard = lock(&self.hub);
        let Hub {
            socket,
            group,
            egress,
            learned,
            stats,
            ..
        } = &mut *guard;
        // `send_multicast` makes one attempt per egress interface, or one
        // attempt on the kernel's default route when none is pinned.
        let attempts = if egress.is_empty() {
            1
        } else {
            egress.len() as u64
        };
        let mut sent = send_multicast(socket, egress, *group, frame);
        match &sent {
            Ok(()) => {
                stats.tx_packets = stats.tx_packets.saturating_add(attempts);
                stats.tx_bytes = stats.tx_bytes.saturating_add(frame.len() as u64 * attempts);
                stats.last_tx = Some(Instant::now());
            }
            Err(e) => {
                stats.send_errors = stats.send_errors.saturating_add(1);
                stats.last_errno = e.raw_os_error().unwrap_or(-1);
            }
        }
        let port = group.port();
        let targets = self
            .unicast
            .iter()
            .map(|ip| SocketAddr::from((*ip, port)))
            .chain(learned.iter().copied());
        for target in targets {
            match socket.send_to(frame, target) {
                Ok(n) => {
                    stats.tx_packets = stats.tx_packets.saturating_add(1);
                    stats.tx_bytes = stats.tx_bytes.saturating_add(n as u64);
                    stats.last_tx = Some(Instant::now());
                    sent = Ok(());
                }
                Err(e) => {
                    stats.send_errors = stats.send_errors.saturating_add(1);
                    stats.last_errno = e.raw_os_error().unwrap_or(-1);
                    if sent.is_err() {
                        sent = Err(e);
                    }
                }
            }
        }
        sent
    }

    /// Sends one frame to `target` by unicast. Counters match `send`.
    pub(crate) fn send_to(&self, frame: &[u8], target: SocketAddr) -> io::Result<()> {
        let mut h = lock(&self.hub);
        match h.socket.send_to(frame, target) {
            Ok(n) => {
                h.stats.tx_packets = h.stats.tx_packets.saturating_add(1);
                h.stats.tx_bytes = h.stats.tx_bytes.saturating_add(n as u64);
                h.stats.last_tx = Some(Instant::now());
                Ok(())
            }
            Err(e) => {
                h.stats.send_errors = h.stats.send_errors.saturating_add(1);
                h.stats.last_errno = e.raw_os_error().unwrap_or(-1);
                Err(e)
            }
        }
    }

    /// Remembers the source of a verified frame, so later frames go to it too.
    pub(crate) fn learn(&self, from: SocketAddr) {
        let mut h = lock(&self.hub);
        let SocketAddr::V4(v4) = from else { return };
        // Our own socket (this address on our group port) is not a peer.
        let own = v4.port() == h.group.port() && h.egress.contains(v4.ip());
        if own || h.learned.contains(&from) {
            return;
        }
        if h.learned.len() == LEARNED_CAP {
            h.learned.pop_front();
        }
        h.learned.push_back(from);
    }

    /// A snapshot of the shared socket's additive counters. Kept on the
    /// endpoint too so a caller holding one can read it directly.
    #[allow(dead_code)]
    pub fn stats(&self) -> HubStats {
        lock(&self.hub).stats()
    }

    /// Next frame this endpoint takes, waiting up to `timeout`.
    pub(crate) fn recv(&self, timeout: Duration) -> io::Result<Option<Datagram>> {
        let deadline = Instant::now() + timeout;
        loop {
            lock(&self.hub).drain()?;
            if let Some(d) = lock(&self.inbox).pop_front() {
                return Ok(Some(d));
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Ok(None);
            }
            std::thread::sleep(left.min(POLL));
        }
    }
}

impl Hub {
    /// A snapshot of this hub's additive counters.
    fn stats(&self) -> HubStats {
        self.stats
    }

    /// Reads everything queued on the socket and hands each datagram to the
    /// endpoints that take it. Every datagram read and every read error is
    /// counted; the result and the delivery logic are unchanged.
    fn drain(&mut self) -> io::Result<()> {
        loop {
            let d = match recv_datagram(&self.socket) {
                Ok(Some(d)) => d,
                Ok(None) => break,
                Err(e) => {
                    self.stats.recv_errors = self.stats.recv_errors.saturating_add(1);
                    self.stats.last_errno = e.raw_os_error().unwrap_or(-1);
                    return Err(e);
                }
            };
            self.stats.rx_packets = self.stats.rx_packets.saturating_add(1);
            self.stats.rx_bytes = self.stats.rx_bytes.saturating_add(d.frame.len() as u64);
            self.stats.last_rx = Some(Instant::now());
            if !admits(&d, &self.egress) {
                continue;
            }
            self.ends.retain(|(_, w)| w.strong_count() > 0);
            for (accept, weak) in &self.ends {
                let Some(inbox) = weak.upgrade() else {
                    continue;
                };
                if !accept.takes(&d.frame) {
                    continue;
                }
                let mut q = lock(&inbox);
                if q.len() == INBOX_CAP {
                    q.pop_front();
                }
                q.push_back(d.clone());
            }
        }
        Ok(())
    }
}

/// Additive counters of every live control socket in this process, or None
/// when no socket is open. Counters are summed; `last_errno` is the most
/// recently observed non-zero error and the last rx/tx times are the newest.
pub fn stats() -> Option<HubStats> {
    let hubs = HUBS.lock().unwrap_or_else(|e| e.into_inner());
    let mut out: Option<HubStats> = None;
    for (_, weak) in hubs.iter() {
        let Some(hub) = weak.upgrade() else { continue };
        let next = lock(&hub).stats();
        out = Some(match out {
            None => next,
            Some(mut sum) => {
                sum.tx_packets = sum.tx_packets.saturating_add(next.tx_packets);
                sum.tx_bytes = sum.tx_bytes.saturating_add(next.tx_bytes);
                sum.rx_packets = sum.rx_packets.saturating_add(next.rx_packets);
                sum.rx_bytes = sum.rx_bytes.saturating_add(next.rx_bytes);
                sum.send_errors = sum.send_errors.saturating_add(next.send_errors);
                sum.recv_errors = sum.recv_errors.saturating_add(next.recv_errors);
                if next.last_errno != 0 {
                    sum.last_errno = next.last_errno;
                }
                sum.last_rx = newest(sum.last_rx, next.last_rx);
                sum.last_tx = newest(sum.last_tx, next.last_tx);
                sum
            }
        });
    }
    out
}

/// The later of two optional instants.
fn newest(a: Option<Instant>, b: Option<Instant>) -> Option<Instant> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (Some(a), None) => Some(a),
        (None, b) => b,
    }
}

/// One datagram from a non-blocking socket, or None when none is queued. The
/// destination address (IP_PKTINFO, enabled in `bind_multicast`) tells the path.
#[cfg(unix)]
fn recv_datagram(socket: &UdpSocket) -> io::Result<Option<Datagram>> {
    let mut buf = [0u8; 256];
    let mut iov = libc::iovec {
        iov_base: buf.as_mut_ptr().cast(),
        iov_len: buf.len(),
    };
    let mut src: libc::sockaddr_in = unsafe { mem::zeroed() };
    let mut control = [0u8; 128];
    let mut msg: libc::msghdr = unsafe { mem::zeroed() };
    msg.msg_name = (&mut src as *mut libc::sockaddr_in).cast();
    msg.msg_namelen = mem::size_of::<libc::sockaddr_in>() as libc::socklen_t;
    msg.msg_iov = &mut iov;
    msg.msg_iovlen = 1;
    msg.msg_control = control.as_mut_ptr().cast();
    msg.msg_controllen = control.len() as _;
    // SAFETY: every pointer in `msg` points at a live local for the call.
    let n = unsafe { libc::recvmsg(socket.as_raw_fd(), &mut msg, 0) };
    if n < 0 {
        let e = io::Error::last_os_error();
        return if e.kind() == io::ErrorKind::WouldBlock {
            Ok(None)
        } else {
            Err(e)
        };
    }
    let mut to = Ipv4Addr::UNSPECIFIED;
    // SAFETY: the control buffer was filled by the kernel for this msghdr.
    unsafe {
        let mut c = libc::CMSG_FIRSTHDR(&msg);
        while !c.is_null() {
            if (*c).cmsg_level == libc::IPPROTO_IP && (*c).cmsg_type == libc::IP_PKTINFO {
                let info = &*(libc::CMSG_DATA(c) as *const libc::in_pktinfo);
                to = Ipv4Addr::from(info.ipi_addr.s_addr.to_ne_bytes());
            }
            c = libc::CMSG_NXTHDR(&msg, c);
        }
    }
    let to_group = to.is_multicast();
    let from = SocketAddr::from((
        Ipv4Addr::from(src.sin_addr.s_addr.to_ne_bytes()),
        u16::from_be(src.sin_port),
    ));
    Ok(Some(Datagram {
        from,
        to,
        path: if to_group {
            Path::Multicast
        } else {
            Path::Unicast
        },
        frame: buf[..n as usize].to_vec(),
    }))
}

/// Asks the kernel to report each datagram's destination address.
#[cfg(unix)]
pub(crate) fn enable_pktinfo(socket: &UdpSocket) -> io::Result<()> {
    let on: libc::c_int = 1;
    // SAFETY: `on` outlives the call and its size is passed along.
    let r = unsafe {
        libc::setsockopt(
            socket.as_raw_fd(),
            libc::IPPROTO_IP,
            libc::IP_PKTINFO,
            (&on as *const libc::c_int).cast(),
            mem::size_of::<libc::c_int>() as libc::socklen_t,
        )
    };
    if r < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// The `WSARecvMsg` extension function. windows-sys does not declare it: the
/// pointer is fetched once per socket with `WSAIoctl` and cached below.
#[cfg(windows)]
type WsaRecvMsg = unsafe extern "system" fn(
    SOCKET,
    *mut WSAMSG,
    *mut u32,
    *mut OVERLAPPED,
    LPWSAOVERLAPPED_COMPLETION_ROUTINE,
) -> i32;

/// The `WSARecvMsg` pointer. It is a provider property: every socket of the
/// provider returns the same function, so one process-wide slot is enough and
/// no entry is kept per raw socket. Only a success is cached: a failed ioctl
/// is retried on the next datagram, so a transient error never makes the
/// `IP_PKTINFO` fallback permanent.
#[cfg(windows)]
static WSARECVMSG: OnceLock<WsaRecvMsg> = OnceLock::new();

/// The `WSARecvMsg` pointer, or None when the ioctl fails this time (the
/// caller then falls back to plain `recv_from`).
#[cfg(windows)]
fn wsa_recv_msg(socket: &UdpSocket) -> Option<WsaRecvMsg> {
    if let Some(function) = WSARECVMSG.get() {
        return Some(*function);
    }
    let mut function: Option<WsaRecvMsg> = None;
    let mut returned: u32 = 0;
    let id: GUID = WSAID_WSARECVMSG;
    // SAFETY: the input GUID is a process constant, both buffers point at
    // live locals, their sizes are passed, and the overlapped slot is null
    // (a synchronous ioctl).
    let rc = unsafe {
        WSAIoctl(
            socket.as_raw_socket() as SOCKET,
            SIO_GET_EXTENSION_FUNCTION_POINTER,
            (&id as *const GUID).cast(),
            mem::size_of_val(&id) as u32,
            (&mut function as *mut Option<WsaRecvMsg>).cast(),
            mem::size_of::<WsaRecvMsg>() as u32,
            &mut returned,
            std::ptr::null_mut(),
            None,
        )
    };
    if rc != 0 {
        return None;
    }
    let function = function?;
    // Two threads may race here; both get the provider's one pointer, and the
    // loser's set is a no-op.
    let _ = WSARECVMSG.set(function);
    Some(function)
}

/// One datagram read with `WSARecvMsg`: the control buffer carries
/// `IP_PKTINFO`, so `to` (and with it the ingress interface and the path) is
/// known exactly as on Unix. Falls back to plain `recv_from` when the provider
/// has no `WSARecvMsg` (then `to` is left unspecified and the Windows admit
/// rule treats it as ours).
#[cfg(windows)]
fn recv_datagram(socket: &UdpSocket) -> io::Result<Option<Datagram>> {
    let Some(recv_msg) = wsa_recv_msg(socket) else {
        return recv_datagram_fallback(socket);
    };
    let mut buf = [0u8; 256];
    let mut src: SOCKADDR_IN = unsafe { mem::zeroed() };
    let mut control = [0u8; 128];
    let mut data = WSABUF {
        len: buf.len() as u32,
        buf: buf.as_mut_ptr(),
    };
    let mut msg = WSAMSG {
        name: (&mut src as *mut SOCKADDR_IN).cast(),
        namelen: mem::size_of::<SOCKADDR_IN>() as i32,
        lpBuffers: &mut data,
        dwBufferCount: 1,
        Control: WSABUF {
            len: control.len() as u32,
            buf: control.as_mut_ptr(),
        },
        dwFlags: 0,
    };
    let mut received: u32 = 0;
    // SAFETY: every pointer in `msg` points at a live local for the call, the
    // buffer lengths match, and the extension pointer was checked above.
    let rc = unsafe {
        recv_msg(
            socket.as_raw_socket() as SOCKET,
            &mut msg,
            &mut received,
            std::ptr::null_mut(),
            None,
        )
    };
    if rc != 0 {
        // SAFETY: a failed WSA call leaves its error in the Winsock slot.
        let error = io::Error::from_raw_os_error(unsafe { WSAGetLastError() });
        return if error.kind() == io::ErrorKind::WouldBlock {
            Ok(None)
        } else {
            Err(error)
        };
    }
    // SAFETY: WSARecvMsg filled `msg.Control` for this call and `src` holds
    // the sender it wrote.
    let to = unsafe { pktinfo_destination(&msg) };
    // SAFETY: `sin_addr` is the union the call wrote for an IPv4 socket.
    let from = SocketAddr::from((
        Ipv4Addr::from(unsafe { src.sin_addr.S_un.S_addr }.to_ne_bytes()),
        u16::from_be(src.sin_port),
    ));
    let to_group = to.is_multicast();
    Ok(Some(Datagram {
        from,
        to,
        path: if to_group {
            Path::Multicast
        } else {
            Path::Unicast
        },
        frame: buf[..received as usize].to_vec(),
    }))
}

/// The destination address from the `IP_PKTINFO` control message, or
/// `UNSPECIFIED` when the message is absent.
///
/// SAFETY: `msg.Control` must be the buffer a successful `WSARecvMsg` filled;
/// `Control.len` is the number of valid bytes.
#[cfg(windows)]
unsafe fn pktinfo_destination(msg: &WSAMSG) -> Ipv4Addr {
    #[repr(C)]
    struct CmsgHdr {
        len: usize,
        level: i32,
        kind: i32,
    }
    let align = mem::size_of::<usize>();
    let base = msg.Control.buf;
    let end = base.add(msg.Control.len as usize);
    let mut current = base as *const CmsgHdr;
    while (current as *const u8) < end {
        let len = (*current).len;
        if len < mem::size_of::<CmsgHdr>() {
            break;
        }
        if (*current).level == IPPROTO_IP && (*current).kind == IP_PKTINFO {
            let info = current.add(1).cast::<IN_PKTINFO>();
            return Ipv4Addr::from((*info).ipi_addr.S_un.S_addr.to_ne_bytes());
        }
        let next = (current as *const u8).add((len + align - 1) & !(align - 1));
        if next >= end {
            break;
        }
        current = next as *const CmsgHdr;
    }
    Ipv4Addr::UNSPECIFIED
}

/// The W0 path: no destination address is known, so `admits` treats an
/// unspecified destination as ours on Windows.
#[cfg(windows)]
fn recv_datagram_fallback(socket: &UdpSocket) -> io::Result<Option<Datagram>> {
    let mut buf = [0u8; 256];
    match socket.recv_from(&mut buf) {
        Ok((n, from)) => Ok(Some(Datagram {
            from,
            to: Ipv4Addr::UNSPECIFIED,
            path: Path::Unicast,
            frame: buf[..n].to_vec(),
        })),
        Err(e) if e.kind() == io::ErrorKind::WouldBlock => Ok(None),
        Err(e) => Err(e),
    }
}

/// Asks Winsock to report each datagram's destination address.
#[cfg(windows)]
pub(crate) fn enable_pktinfo(socket: &UdpSocket) -> io::Result<()> {
    let on: i32 = 1;
    // SAFETY: `on` outlives the call and its size is passed along.
    let rc = unsafe {
        setsockopt(
            socket.as_raw_socket() as SOCKET,
            IPPROTO_IP,
            IP_PKTINFO,
            (&on as *const i32).cast(),
            mem::size_of::<i32>() as i32,
        )
    };
    if rc != 0 {
        // SAFETY: a failed WSA call leaves its error in the Winsock slot.
        return Err(io::Error::from_raw_os_error(unsafe { WSAGetLastError() }));
    }
    Ok(())
}

/// Poisoned locks are not a reason to stop: the data is plain queues.
fn lock<T, M: Deref<Target = Mutex<T>>>(m: &M) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hub_weak_pointer_that_died_is_replaced_not_upgraded() {
        let key: HubKey = ("239.1.2.3:9799".parse().unwrap(), 1, Vec::new());
        let other: HubKey = ("239.1.2.4:9799".parse().unwrap(), 1, Vec::new());
        let live = Arc::new(Mutex::new(7u8));
        let dead = Arc::new(Mutex::new(9u8));
        let mut hubs: Vec<(HubKey, Weak<Mutex<u8>>)> = vec![
            (key.clone(), Arc::downgrade(&dead)),
            (other.clone(), Arc::downgrade(&live)),
        ];
        drop(dead);
        // The key's hub is dead: no panic, and the slot is dropped so the
        // caller creates a fresh hub.
        assert!(live_hub(&mut hubs, &key).is_none());
        assert_eq!(hubs.len(), 1);
        assert_eq!(hubs[0].0, other);
        // A live hub still upgrades.
        let found = live_hub(&mut hubs, &other).expect("the live hub");
        assert_eq!(*lock(&found), 7);
        // A missing key is just None.
        assert!(live_hub(&mut hubs, &key).is_none());
    }

    fn datagram(from: &str, to: [u8; 4]) -> Datagram {
        Datagram {
            from: from.parse().unwrap(),
            to: Ipv4Addr::from(to),
            path: Path::Unicast,
            frame: Vec::new(),
        }
    }

    // Guard for the source half of the ingress rule: the destination half is
    // covered by tests/ingress_test.rs.
    #[test]
    fn a_pinned_device_takes_frames_from_the_lan_and_not_from_the_internet() {
        let ours = [Ipv4Addr::new(192, 168, 43, 1)];
        assert!(admits(
            &datagram("192.168.43.20:9799", [192, 168, 43, 1]),
            &ours
        ));
        assert!(admits(
            &datagram("192.168.43.20:9799", [239, 255, 42, 99]),
            &ours
        ));
        assert!(!admits(
            &datagram("203.0.113.9:9799", [192, 168, 43, 1]),
            &ours
        ));
        assert!(!admits(
            &datagram("192.168.43.20:9799", [10, 0, 2, 1]),
            &ours
        ));
    }

    #[test]
    fn a_carrier_grade_nat_source_counts_as_the_lan() {
        let ours = [Ipv4Addr::new(192, 168, 43, 1)];
        assert!(admits(
            &datagram("100.64.0.20:9799", [192, 168, 43, 1]),
            &ours
        ));
        assert!(admits(
            &datagram("100.127.255.254:9799", [239, 255, 42, 99]),
            &ours
        ));
        assert!(!admits(
            &datagram("100.128.0.1:9799", [192, 168, 43, 1]),
            &ours
        ));
    }

    #[test]
    fn an_unpinned_device_takes_every_frame() {
        assert!(admits(&datagram("203.0.113.9:9799", [10, 0, 2, 1]), &[]));
    }

    // The Windows fallback cannot report the destination; it must not make a
    // pinned device drop every frame, while the source rule still applies.
    #[test]
    fn an_unknown_destination_counts_as_ours_only_on_windows() {
        let ours = [Ipv4Addr::new(192, 168, 43, 1)];
        assert_eq!(
            admits(&datagram("192.168.43.20:9799", [0, 0, 0, 0]), &ours),
            cfg!(windows)
        );
        assert!(!admits(&datagram("203.0.113.9:9799", [0, 0, 0, 0]), &ours));
    }
}
