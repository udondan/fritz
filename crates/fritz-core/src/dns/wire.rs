//! A minimal DNS client over UDP: one question for the A or AAAA records of
//! one name, one answer, read with bounds checks.

use std::fmt;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::time::Duration;

use serde::Serialize;
use tokio::net::UdpSocket;

/// The largest answer read. Answers for A and AAAA records are small.
const MAX_ANSWER: usize = 4096;

/// The kind of record asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Record {
    A,
    Aaaa,
}

impl Record {
    fn code(self) -> u16 {
        match self {
            Self::A => 1,
            Self::Aaaa => 28,
        }
    }
}

/// Why a query got no addresses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Failure {
    /// The server says the name does not exist (NXDOMAIN).
    NoSuchName,
    /// The name exists, but has no record of this kind.
    NoRecord,
    /// The server did not answer in time.
    Timeout,
    /// The server answered with an error, like SERVFAIL or REFUSED.
    Refused(u8),
    /// The answer was too long for UDP and cut off, so it is incomplete.
    Truncated,
    /// The query could not be sent, or the answer could not be read.
    Network(String),
}

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoSuchName => f.write_str("no such name"),
            Self::NoRecord => f.write_str("no address"),
            Self::Timeout => f.write_str("no answer in time"),
            Self::Refused(2) => f.write_str("server failure (SERVFAIL)"),
            Self::Refused(5) => f.write_str("refused (REFUSED)"),
            Self::Refused(code) => write!(f, "error code {code}"),
            Self::Truncated => f.write_str("answer cut off, too long for UDP"),
            Self::Network(error) => f.write_str(error),
        }
    }
}

/// Asks `server` for the records of `kind` of `name`, and waits at most
/// `timeout` for the answer. Answers to other queries are ignored.
///
/// # Errors
///
/// A [`Failure`] if there are no addresses.
pub async fn query(
    server: SocketAddr,
    name: &str,
    kind: Record,
    timeout: Duration,
) -> Result<Vec<IpAddr>, Failure> {
    let id = query_id();
    let packet = encode(id, name, kind).map_err(|e| Failure::Network(e.to_owned()))?;
    let exchange = async {
        let local: SocketAddr = if server.is_ipv4() {
            (Ipv4Addr::UNSPECIFIED, 0).into()
        } else {
            (Ipv6Addr::UNSPECIFIED, 0).into()
        };
        let socket = UdpSocket::bind(local).await?;
        socket.connect(server).await?;
        socket.send(&packet).await?;
        let mut buffer = vec![0; MAX_ANSWER];
        loop {
            let length = socket.recv(&mut buffer).await?;
            if let Some(answer) = decode(id, kind, &buffer[..length]) {
                return Ok::<_, std::io::Error>(answer);
            }
        }
    };
    match tokio::time::timeout(timeout, exchange).await {
        Err(_) => Err(Failure::Timeout),
        Ok(Err(error)) => Err(Failure::Network(error.to_string())),
        Ok(Ok(answer)) => answer,
    }
}

/// An ID for a query, different for every query of a run.
fn query_id() -> u16 {
    use std::sync::atomic::{AtomicU16, Ordering};
    static NEXT: AtomicU16 = AtomicU16::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    #[expect(clippy::cast_possible_truncation, reason = "any 16 bits will do")]
    let seed = (nanos ^ std::process::id()) as u16;
    seed.wrapping_add(NEXT.fetch_add(1, Ordering::Relaxed))
}

/// The query for the records of `kind` of `name`, with recursion desired.
pub(crate) fn encode(id: u16, name: &str, kind: Record) -> Result<Vec<u8>, &'static str> {
    let mut packet = Vec::with_capacity(32 + name.len());
    packet.extend_from_slice(&id.to_be_bytes());
    // Recursion desired; one question.
    packet.extend_from_slice(&[0x01, 0x00, 0, 1, 0, 0, 0, 0, 0, 0]);
    for label in name.trim_end_matches('.').split('.') {
        let length = u8::try_from(label.len())
            .ok()
            .filter(|l| (1..=63).contains(l))
            .ok_or("invalid label")?;
        packet.push(length);
        packet.extend_from_slice(label.as_bytes());
    }
    packet.push(0);
    packet.extend_from_slice(&kind.code().to_be_bytes());
    // Class IN.
    packet.extend_from_slice(&[0, 1]);
    Ok(packet)
}

/// The addresses in the answer to the query `id`, or `None` if `packet` is
/// not that answer.
pub(crate) fn decode(id: u16, kind: Record, packet: &[u8]) -> Option<Result<Vec<IpAddr>, Failure>> {
    let mut reader = Bytes { packet, at: 0 };
    if reader.u16()? != id {
        return None;
    }
    let flags = reader.u8()?;
    if flags & 0x80 == 0 {
        return None;
    }
    // TC: the answer was cut off and may lack records.
    if flags & 0x02 != 0 {
        return Some(Err(Failure::Truncated));
    }
    let code = reader.u8()? & 0x0f;
    let questions = reader.u16()?;
    let answers = reader.u16()?;
    reader.skip(4)?;
    match code {
        0 => {}
        3 => return Some(Err(Failure::NoSuchName)),
        code => return Some(Err(Failure::Refused(code))),
    }
    for _ in 0..questions {
        reader.name()?;
        reader.skip(4)?;
    }
    let mut addresses = Vec::new();
    for _ in 0..answers {
        reader.name()?;
        let record = reader.u16()?;
        reader.skip(6)?;
        let length = usize::from(reader.u16()?);
        let data = reader.take(length)?;
        if record == kind.code()
            && let Some(address) = address(data)
        {
            addresses.push(address);
        }
    }
    Some(if addresses.is_empty() {
        Err(Failure::NoRecord)
    } else {
        Ok(addresses)
    })
}

/// The address in the data of an A or AAAA record.
fn address(data: &[u8]) -> Option<IpAddr> {
    if let Ok(octets) = <[u8; 4]>::try_from(data) {
        return Some(IpAddr::from(octets));
    }
    <[u8; 16]>::try_from(data).ok().map(IpAddr::from)
}

/// Reads a packet front to back, `None` past its end.
struct Bytes<'a> {
    packet: &'a [u8],
    at: usize,
}

impl<'a> Bytes<'a> {
    fn take(&mut self, length: usize) -> Option<&'a [u8]> {
        let end = self.at.checked_add(length)?;
        let bytes = self.packet.get(self.at..end)?;
        self.at = end;
        Some(bytes)
    }

    fn skip(&mut self, length: usize) -> Option<()> {
        self.take(length).map(|_| ())
    }

    fn u8(&mut self) -> Option<u8> {
        self.take(1).map(|b| b[0])
    }

    fn u16(&mut self) -> Option<u16> {
        self.take(2).map(|b| u16::from_be_bytes([b[0], b[1]]))
    }

    /// Skips a name: labels, ended by an empty one or a pointer.
    fn name(&mut self) -> Option<()> {
        loop {
            let length = self.u8()?;
            match length {
                0 => return Some(()),
                l if l & 0xc0 == 0xc0 => return self.skip(1),
                l => self.skip(usize::from(l))?,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An answer to the query `id` with code `code` and the records.
    fn answer(id: u16, code: u8, records: &[(u16, &[u8])]) -> Vec<u8> {
        let mut packet = encode(id, "nas.example", Record::A).unwrap();
        packet[2] = 0x81;
        packet[3] = 0x80 | code;
        packet[7] = u8::try_from(records.len()).unwrap();
        for (kind, data) in records {
            // A pointer to the name of the question.
            packet.extend_from_slice(&[0xc0, 12]);
            packet.extend_from_slice(&kind.to_be_bytes());
            packet.extend_from_slice(&[0, 1, 0, 0, 0, 60]);
            packet.extend_from_slice(&u16::try_from(data.len()).unwrap().to_be_bytes());
            packet.extend_from_slice(data);
        }
        packet
    }

    #[test]
    fn encodes_a_query() {
        let packet = encode(0x1234, "nas.example.", Record::Aaaa).unwrap();
        assert_eq!(
            packet,
            [
                0x12, 0x34, 1, 0, 0, 1, 0, 0, 0, 0, 0, 0, 3, b'n', b'a', b's', 7, b'e', b'x', b'a',
                b'm', b'p', b'l', b'e', 0, 0, 28, 0, 1
            ]
        );
        assert!(encode(1, "a..b", Record::A).is_err());
        assert!(encode(1, &"x".repeat(64), Record::A).is_err());
    }

    #[test]
    fn decodes_answers() {
        let cname: &[u8] = &[3, b'n', b'a', b's', 0];
        let packet = answer(7, 0, &[(5, cname), (1, &[192, 0, 2, 7]), (28, &[0; 16])]);
        assert_eq!(
            decode(7, Record::A, &packet),
            Some(Ok(vec![IpAddr::from([192, 0, 2, 7])]))
        );
        assert_eq!(
            decode(7, Record::Aaaa, &packet),
            Some(Ok(vec![IpAddr::from([0u8; 16])]))
        );
        assert_eq!(
            decode(7, Record::A, &answer(7, 0, &[])),
            Some(Err(Failure::NoRecord))
        );
        assert_eq!(
            decode(7, Record::A, &answer(7, 3, &[])),
            Some(Err(Failure::NoSuchName))
        );
        assert_eq!(
            decode(7, Record::A, &answer(7, 2, &[])),
            Some(Err(Failure::Refused(2)))
        );
        // Another query, a query instead of an answer, cut off.
        assert_eq!(decode(8, Record::A, &packet), None);
        assert_eq!(
            decode(7, Record::A, &encode(7, "nas", Record::A).unwrap()),
            None
        );
        assert_eq!(decode(7, Record::A, &packet[..packet.len() - 2]), None);
        // An answer cut off for UDP.
        let mut truncated = packet.clone();
        truncated[2] |= 0x02;
        assert_eq!(
            decode(7, Record::A, &truncated),
            Some(Err(Failure::Truncated))
        );
        assert_eq!(
            Failure::Truncated.to_string(),
            "answer cut off, too long for UDP"
        );
        // A record with an odd length is skipped.
        assert_eq!(
            decode(7, Record::A, &answer(7, 0, &[(1, &[1, 2, 3])])),
            Some(Err(Failure::NoRecord))
        );
    }

    #[test]
    fn describes_failures() {
        let texts: Vec<String> = [
            Failure::NoSuchName,
            Failure::NoRecord,
            Failure::Timeout,
            Failure::Refused(2),
            Failure::Refused(5),
            Failure::Refused(4),
            Failure::Network("unreachable".to_owned()),
        ]
        .iter()
        .map(ToString::to_string)
        .collect();
        assert_eq!(
            texts,
            [
                "no such name",
                "no address",
                "no answer in time",
                "server failure (SERVFAIL)",
                "refused (REFUSED)",
                "error code 4",
                "unreachable",
            ]
        );
    }
}
