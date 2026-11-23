use bytes::BytesMut;
use smoltcp::{
    phy::{Checksum, ChecksumCapabilities},
    wire::{IpAddress, IpProtocol, IpRepr, IpVersion, Ipv4Packet, Ipv6Packet, UdpPacket, UdpRepr},
};

use tracing::warn;

/// A phone summary is at most 512 bytes; the history frame is at most 2048.
/// The provider is in-process, but the router still caps what it will emit.
const MAX_BODY: usize = 2048;

/// Build the reply to an authenticated stats request. The UDP endpoints are
/// swapped exactly as in the DNS responder: the reply leaves the gateway
/// address and returns to the tunnel source.
pub fn reply(from_wg: &[u8], body: &[u8]) -> anyhow::Result<BytesMut> {
    if body.len() > MAX_BODY {
        anyhow::bail!("stats body too large");
    }
    let mut checksummer = ChecksumCapabilities::ignored();
    checksummer.udp = Checksum::Tx;
    checksummer.ipv4 = Checksum::Tx;
    checksummer.tcp = Checksum::Tx;

    let (src_addr, dst_addr, payload): (IpAddress, IpAddress, &[u8]) =
        match IpVersion::of_packet(from_wg) {
            Err(_e) => anyhow::bail!("stats packet is not IP"),
            Ok(IpVersion::Ipv4) => {
                let Ok(packet) = Ipv4Packet::new_checked(from_wg) else {
                    anyhow::bail!("stats packet has a bad IPv4 header");
                };
                (
                    packet.src_addr().into(),
                    packet.dst_addr().into(),
                    packet.payload(),
                )
            }
            Ok(IpVersion::Ipv6) => {
                let Ok(packet) = Ipv6Packet::new_checked(from_wg) else {
                    anyhow::bail!("stats packet has a bad IPv6 header");
                };
                (
                    packet.src_addr().into(),
                    packet.dst_addr().into(),
                    packet.payload(),
                )
            }
        };

    let udp = match UdpPacket::new_checked(payload) {
        Ok(udp) => {
            if !udp.verify_checksum(&src_addr, &dst_addr) {
                warn!("Failed UDP checksum on a stats request");
                anyhow::bail!("stats packet has a bad UDP checksum");
            }
            udp
        }
        Err(_e) => anyhow::bail!("stats packet has a bad UDP header"),
    };
    let (src_port, dst_port) = (udp.src_port(), udp.dst_port());

    let ip = IpRepr::new(dst_addr, src_addr, IpProtocol::Udp, body.len() + 8, 64);
    let len = ip.buffer_len();
    let udp_repr = UdpRepr {
        src_port: dst_port,
        dst_port: src_port,
    };

    let mut buf = BytesMut::new();
    buf.resize(len, 0);
    match ip {
        IpRepr::Ipv4(ipv4) => {
            let mut packet = Ipv4Packet::new_unchecked(buf);
            ipv4.emit(&mut packet, &checksummer);
            let mut udp = UdpPacket::new_unchecked(packet.payload_mut());
            udp_repr.emit(
                &mut udp,
                &dst_addr,
                &src_addr,
                body.len(),
                |payload| payload.copy_from_slice(body),
                &checksummer,
            );
            buf = packet.into_inner();
        }
        IpRepr::Ipv6(ipv6) => {
            let mut packet = Ipv6Packet::new_unchecked(buf);
            ipv6.emit(&mut packet);
            let mut udp = UdpPacket::new_unchecked(packet.payload_mut());
            udp_repr.emit(
                &mut udp,
                &dst_addr,
                &src_addr,
                body.len(),
                |payload| payload.copy_from_slice(body),
                &checksummer,
            );
            buf = packet.into_inner();
        }
    }
    buf.resize(len, 0);
    Ok(buf)
}
