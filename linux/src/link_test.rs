//! Linux transport for the one-shot "Test the link now" measurement.
//!
//! The probe is the same unprivileged ICMP echo socket the liveness check
//! uses, bound to `jafn0` and aimed at the gateway. The runner is generic over
//! [`EchoProbe`] so tests never open a socket.

use std::io;
use std::net::{Ipv4Addr, SocketAddr};
use std::os::unix::io::AsRawFd;
use std::time::{Duration, Instant};

use jafnify_core::link_test::{LinkSample, LinkTestConfig, LinkTestResult};

/// One ICMP echo attempt.
pub trait EchoProbe {
    /// Send `seq` and wait up to `timeout` for its reply.
    fn echo(&mut self, seq: u32, timeout: Duration) -> EchoReply;
}

/// The outcome of one echo attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EchoReply {
    /// The reply arrived after this round trip.
    Reply(Duration),
    /// No matching reply before the deadline.
    Timeout,
    /// The host does not allow an unprivileged ICMP socket.
    NotPermitted,
}

/// Run the configured plan through `probe`. Probes start at least `interval`
/// apart; each waits up to `timeout`. A denial ends the run at once.
pub fn run(probe: &mut dyn EchoProbe, config: LinkTestConfig) -> LinkTestResult {
    let mut samples = Vec::with_capacity(config.probes as usize);
    for seq in 0..config.probes {
        let started = Instant::now();
        match probe.echo(seq, config.timeout) {
            EchoReply::Reply(rtt) => samples.push(LinkSample {
                seq,
                rtt_ms: Some(rtt.as_millis() as u64),
            }),
            EchoReply::Timeout => samples.push(LinkSample { seq, rtt_ms: None }),
            EchoReply::NotPermitted => return LinkTestResult::not_permitted(),
        }
        if let Some(rest) = config.interval.checked_sub(started.elapsed()) {
            std::thread::sleep(rest);
        }
    }
    LinkTestResult::from_samples(&samples)
}

/// Run the fixed plan, or return "Connect first" without opening a socket.
pub fn run_default(connected: bool) -> LinkTestResult {
    if !connected {
        return LinkTestResult::connect_first();
    }
    match IcmpProbe::open() {
        Ok(mut probe) => run(&mut probe, LinkTestConfig::DEFAULT),
        Err(e) if e.kind() == io::ErrorKind::PermissionDenied => LinkTestResult::not_permitted(),
        Err(_) => LinkTestResult::from_samples(&[]),
    }
}

const MAGIC: &[u8] = b"jafnify-link-test";

/// The unprivileged ICMP echo probe, bound to `jafn0`.
struct IcmpProbe {
    socket: socket2::Socket,
    target: SocketAddr,
}

impl IcmpProbe {
    fn open() -> io::Result<Self> {
        let socket = socket2::Socket::new(
            socket2::Domain::IPV4,
            socket2::Type::DGRAM,
            Some(socket2::Protocol::ICMPV4),
        )?;
        socket.bind_device(Some(crate::app::IFACE.as_bytes()))?;
        socket.set_nonblocking(true)?;
        let target = SocketAddr::from((
            jafnify_core::GATEWAY_DNS_IP
                .parse::<Ipv4Addr>()
                .unwrap_or(Ipv4Addr::new(10, 233, 0, 1)),
            0,
        ));
        Ok(Self { socket, target })
    }
}

impl EchoProbe for IcmpProbe {
    fn echo(&mut self, seq: u32, timeout: Duration) -> EchoReply {
        let started = Instant::now();
        let packet = echo_packet(seq);
        if let Err(e) = self.socket.send_to(&packet, &self.target.into()) {
            return if e.kind() == io::ErrorKind::PermissionDenied {
                EchoReply::NotPermitted
            } else {
                EchoReply::Timeout
            };
        }
        let deadline = started + timeout;
        let mut buf = [0u8; 256];
        loop {
            let now = Instant::now();
            if now >= deadline {
                return EchoReply::Timeout;
            }
            let millis = (deadline - now).as_millis().min(i32::MAX as u128) as i32;
            let mut pollfd = libc::pollfd {
                fd: self.socket.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            let ready = unsafe { libc::poll(&mut pollfd, 1, millis) };
            if ready < 0 {
                let err = io::Error::last_os_error();
                if err.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                return EchoReply::Timeout;
            }
            if ready == 0 {
                return EchoReply::Timeout;
            }
            let received = unsafe {
                libc::recvfrom(
                    self.socket.as_raw_fd(),
                    buf.as_mut_ptr() as *mut libc::c_void,
                    buf.len(),
                    0,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                )
            };
            if received < 0 {
                let err = io::Error::last_os_error();
                if err.kind() == io::ErrorKind::WouldBlock {
                    continue;
                }
                return EchoReply::Timeout;
            }
            // A duplicate, late or foreign reply is ignored; keep waiting for
            // this sequence number until its own deadline.
            if parse_reply(&buf[..received as usize]) == Some(seq as u16) {
                return EchoReply::Reply(started.elapsed());
            }
        }
    }
}

/// An echo request with the fixed magic and the sequence number.
fn echo_packet(seq: u32) -> Vec<u8> {
    let mut packet = vec![8, 0, 0, 0, 0, 0, (seq >> 8) as u8, (seq & 0xff) as u8];
    packet.extend_from_slice(MAGIC);
    packet.extend_from_slice(&seq.to_be_bytes());
    let sum = crate::app::icmp_checksum(&packet);
    packet[2..4].copy_from_slice(&sum.to_be_bytes());
    packet
}

/// The reply sequence number when `buf` is one of our echo replies. Linux
/// ping sockets usually start at the ICMP header; skip an IPv4 header if one
/// is present.
fn parse_reply(buf: &[u8]) -> Option<u16> {
    let icmp = if buf.first().is_some_and(|byte| *byte >> 4 == 4) {
        let header_len = usize::from(buf[0] & 0x0f) * 4;
        buf.get(header_len..)?
    } else {
        buf
    };
    if icmp.len() < 8 + MAGIC.len() || icmp[0] != 0 || icmp[1] != 0 {
        return None;
    }
    if &icmp[8..8 + MAGIC.len()] != MAGIC {
        return None;
    }
    Some(u16::from_be_bytes([icmp[6], icmp[7]]))
}
