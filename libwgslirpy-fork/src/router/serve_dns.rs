use std::collections::VecDeque;
use std::net::SocketAddr;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use bytes::BytesMut;
use simple_dns::ResourceRecord;
use smoltcp::{
    phy::{Checksum, ChecksumCapabilities},
    wire::{
        IpAddress, IpProtocol, IpRepr, IpVersion, Ipv4Packet, Ipv6Packet, UdpPacket,
    },
};

use tracing::{warn, info};


pub async fn dns(
    from_wg: BytesMut,
) -> anyhow::Result<BytesMut> {
    let mut checksummer = ChecksumCapabilities::ignored();
    checksummer.udp = Checksum::Tx;
    checksummer.ipv4 = Checksum::Tx;
    checksummer.tcp = Checksum::Tx;

    let buf = &from_wg[..];
    let (src_addr, dst_addr, payload): (IpAddress, IpAddress, &[u8]) =
        match IpVersion::of_packet(&buf[..]) {
            Err(_e) => {
                anyhow::bail!("DNSERR1");
            }
            Ok(IpVersion::Ipv4) => {
                let Ok(p) = Ipv4Packet::new_checked(&buf[..]) else { anyhow::bail!("DNSERR2") };
                (p.src_addr().into(), p.dst_addr().into(), p.payload())
            }
            Ok(IpVersion::Ipv6) => {
                let Ok(p) = Ipv6Packet::new_checked(&buf[..]) else { anyhow::bail!("DNSERR3") };
                (p.src_addr().into(), p.dst_addr().into(), p.payload())
            }
        };

    let (payload, srcport, dstport) = match UdpPacket::new_checked(payload) {
        Ok(u) => {
            if !u.verify_checksum(&src_addr, &dst_addr) {
                warn!("Failed UDP checksum");
                anyhow::bail!("DNSERR5");
            }
            (u.payload(), u.src_port(), u.dst_port())
        }
        Err(_e) => anyhow::bail!("DNSERR4"),
    };

    let Ok(dns) = simple_dns::Packet::parse(payload) else {
        warn!("Non-DNS packet to DNS service");
        anyhow::bail!("DNSERR6")
    };

    if dns.questions.is_empty() {
        warn!("DNS query with no questions");
        anyhow::bail!("DNSERR7")
    }

    if dns.questions.len() > 1 {
        warn!("DNS query with more than one question");
    }

    let q = &dns.questions[0];

    let mut reply = dns.clone().into_reply();
    
    let nam = format!("{}:0", q.qname);
    info!("DNS query {nam} from {src_addr} {srcport}");

    let started = Instant::now();
    if let Ok(ret) = tokio::net::lookup_host(nam).await {
        record_dns_answer(started.elapsed());
        for x in ret {
            match x {
                SocketAddr::V4(t) => reply.answers.push(ResourceRecord {
                    name: q.qname.clone(),
                    class: simple_dns::CLASS::IN,
                    ttl: 60,
                    rdata: simple_dns::rdata::RData::A(simple_dns::rdata::A::from(*t.ip())),
                    cache_flush: false,
                }),
                SocketAddr::V6(t) => reply.answers.push(ResourceRecord {
                    name: q.qname.clone(),
                    class: simple_dns::CLASS::IN,
                    ttl: 60,
                    rdata: simple_dns::rdata::RData::AAAA(simple_dns::rdata::AAAA::from(*t.ip())),
                    cache_flush: false,
                }),
            }
        }
    } else {
        *reply.rcode_mut() = simple_dns::RCODE::ServerFailure;
    }


    let Ok(data) = reply.build_bytes_vec_compressed() else {
        anyhow::bail!("Failed to build DNS reply");
    };

    let mut buf = BytesMut::new();
    buf.resize(data.len() + 64, 0);

    let r = IpRepr::new(
        dst_addr,
        src_addr,
        IpProtocol::Udp,
        data.len() + 8,
        64,
    );

    let len = r.buffer_len();

    let r2 = smoltcp::wire::UdpRepr {
        src_port: dstport,
        dst_port: srcport,
    };

    match r {
        IpRepr::Ipv4(r) => {
            let mut ippkt4 = Ipv4Packet::new_unchecked(buf);
            r.emit(&mut ippkt4, &checksummer);
            let mut udppkt = UdpPacket::new_unchecked(ippkt4.payload_mut());
            r2.emit(
                &mut udppkt,
                &dst_addr,
                &src_addr,
                data.len(),
                |p| p.copy_from_slice(&data[..]),
                &checksummer,
            );
            buf = ippkt4.into_inner();
        }
        IpRepr::Ipv6(r) => {
            let mut ippkt6 = Ipv6Packet::new_unchecked(buf);
            r.emit(&mut ippkt6);
            let mut udppkt = UdpPacket::new_unchecked(ippkt6.payload_mut());
            r2.emit(
                &mut udppkt,
                &dst_addr,
                &src_addr,
                data.len(),
                |p| p.copy_from_slice(&data[..]),
                &checksummer,
            );
            buf = ippkt6.into_inner();
        }
    }
    buf.resize(len, 0);
    

    Ok::<_, anyhow::Error>(buf)
}

const DNS_RING: usize = 32;
const DNS_WINDOW: Duration = Duration::from_secs(120);
const DNS_MIN_SAMPLES: usize = 3;

#[derive(Clone, Copy, Debug)]
struct DnsSample {
    at: Instant,
    ms: u64,
}

static DNS_SAMPLES: OnceLock<Mutex<VecDeque<DnsSample>>> = OnceLock::new();

fn samples() -> &'static Mutex<VecDeque<DnsSample>> {
    DNS_SAMPLES.get_or_init(|| Mutex::new(VecDeque::with_capacity(DNS_RING)))
}

fn push_sample(ring: &mut VecDeque<DnsSample>, sample: DnsSample) {
    if ring.len() == DNS_RING {
        ring.pop_front();
    }
    ring.push_back(sample);
}

fn median_ms<'a>(
    samples: impl IntoIterator<Item = &'a DnsSample>,
    now: Instant,
) -> Option<u32> {
    let mut values: Vec<u64> = samples
        .into_iter()
        .filter(|sample| now.saturating_duration_since(sample.at) <= DNS_WINDOW)
        .map(|sample| sample.ms)
        .collect();
    if values.len() < DNS_MIN_SAMPLES {
        return None;
    }
    values.sort_unstable();
    Some(values[values.len() / 2].min(60_000) as u32)
}

/// Record one successful lookup. Uses `try_lock` so a busy median reader can
/// never delay the DNS path; a skipped sample is harmless.
pub(crate) fn record_dns_answer(duration: Duration) {
    let ms = duration.as_millis().min(u128::from(u64::MAX)) as u64;
    if let Ok(mut ring) = samples().try_lock() {
        push_sample(
            &mut ring,
            DnsSample {
                at: Instant::now(),
                ms,
            },
        );
    }
}

/// The median of the last two minutes, or None with fewer than three samples.
pub(crate) fn dns_answer_median_ms() -> Option<u32> {
    let now = Instant::now();
    let ring = samples().lock().unwrap_or_else(|e| e.into_inner());
    median_ms(ring.iter(), now)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(now: Instant, age_secs: u64, ms: u64) -> DnsSample {
        DnsSample {
            at: now - Duration::from_secs(age_secs),
            ms,
        }
    }

    #[test]
    fn fewer_than_three_recent_samples_is_unknown() {
        let now = Instant::now();
        assert_eq!(median_ms(&[], now), None);
        let two = [sample(now, 1, 10), sample(now, 2, 20)];
        assert_eq!(median_ms(&two, now), None);
    }

    #[test]
    fn three_samples_take_the_middle() {
        let now = Instant::now();
        let three = [
            sample(now, 1, 30),
            sample(now, 2, 10),
            sample(now, 3, 20),
        ];
        assert_eq!(median_ms(&three, now), Some(20));
    }

    #[test]
    fn even_samples_take_the_upper_middle() {
        let now = Instant::now();
        let four = [
            sample(now, 1, 40),
            sample(now, 2, 10),
            sample(now, 3, 30),
            sample(now, 4, 20),
        ];
        assert_eq!(median_ms(&four, now), Some(30));
    }

    #[test]
    fn samples_older_than_two_minutes_are_ignored() {
        let now = Instant::now();
        let mixed = [
            sample(now, 121, 999),
            sample(now, 1, 30),
            sample(now, 2, 10),
            sample(now, 3, 20),
        ];
        assert_eq!(median_ms(&mixed, now), Some(20));

        let old = [
            sample(now, 130, 10),
            sample(now, 140, 20),
            sample(now, 150, 30),
        ];
        assert_eq!(median_ms(&old, now), None);
    }

    #[test]
    fn the_ring_keeps_the_last_32() {
        let now = Instant::now();
        let mut ring = VecDeque::new();
        for ms in 0..40 {
            push_sample(&mut ring, sample(now, 0, ms));
        }
        assert_eq!(ring.len(), DNS_RING);
        assert_eq!(ring.front().unwrap().ms, 8);
        assert_eq!(ring.back().unwrap().ms, 39);
    }
}
