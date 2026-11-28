//! A small DNS server on the loopback interface, for tests of DNS clients:
//! it answers A and AAAA queries from a table of names and addresses, and
//! every other name with NXDOMAIN.
//!
//! ```no_run
//! # async fn example() -> std::io::Result<()> {
//! let dns = fritz_sim::dns::DnsServer::start(&[("nas.fritz.box", "192.0.2.10")]).await?;
//! println!("ask {}", dns.address());
//! # Ok(())
//! # }
//! ```

use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex, PoisonError};

use tokio::net::UdpSocket;
use tokio::task::JoinHandle;

/// A DNS server that answers until it is dropped.
#[derive(Debug)]
pub struct DnsServer {
    address: SocketAddr,
    queries: Arc<Mutex<Vec<String>>>,
    task: JoinHandle<()>,
}

impl DnsServer {
    /// Serves `records`, pairs of a name and an address, on a free UDP port
    /// of the loopback interface. Names are compared ignoring case.
    ///
    /// # Errors
    ///
    /// The errors of binding the port, and [`io::ErrorKind::InvalidInput`]
    /// for an address that is not one.
    pub async fn start(records: &[(&str, &str)]) -> io::Result<Self> {
        let table = records
            .iter()
            .map(|(name, address)| {
                let address = address
                    .parse::<IpAddr>()
                    .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
                Ok((name.trim_end_matches('.').to_ascii_lowercase(), address))
            })
            .collect::<io::Result<Vec<_>>>()?;
        let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await?;
        let address = socket.local_addr()?;
        let queries = Arc::new(Mutex::new(Vec::new()));
        let seen = Arc::clone(&queries);
        let task = tokio::spawn(async move {
            let mut buffer = [0; 512];
            loop {
                if let Ok((length, peer)) = socket.recv_from(&mut buffer).await
                    && let Some((name, reply)) = reply(&buffer[..length], &table)
                {
                    seen.lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .push(name);
                    let _ = socket.send_to(&reply, peer).await;
                }
            }
        });
        Ok(Self {
            address,
            queries,
            task,
        })
    }

    /// The address to send queries to.
    pub fn address(&self) -> SocketAddr {
        self.address
    }

    /// The names asked for so far, in lower case, one per query.
    pub fn queries(&self) -> Vec<String> {
        self.queries
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

impl Drop for DnsServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// The name asked for in `query` and the answer from `table`, `None` for
/// anything but a query with one question.
fn reply(query: &[u8], table: &[(String, IpAddr)]) -> Option<(String, Vec<u8>)> {
    let header = query.get(..12)?;
    if header[2] & 0x80 != 0 || header[4..6] != [0, 1] {
        return None;
    }
    let mut at = 12;
    let mut labels = Vec::new();
    loop {
        let length = usize::from(*query.get(at)?);
        at += 1;
        if length == 0 {
            break;
        }
        labels.push(String::from_utf8_lossy(query.get(at..at + length)?).to_ascii_lowercase());
        at += length;
    }
    // The question ends with its type and class.
    let question = query.get(12..at + 4)?;
    let kind = &query[at..at + 2];
    let name = labels.join(".");
    let known = table.iter().any(|(n, _)| *n == name);
    let answers: Vec<&IpAddr> = table
        .iter()
        .filter(|(n, a)| *n == name && (a.is_ipv4() == (kind == [0, 1])))
        .map(|(_, a)| a)
        .collect();
    let mut packet = Vec::with_capacity(64);
    packet.extend_from_slice(&header[..2]);
    // An answer, recursion available, and NXDOMAIN for unknown names.
    packet.extend_from_slice(&[0x81, if known { 0x80 } else { 0x83 }, 0, 1, 0]);
    packet.push(u8::try_from(answers.len()).ok()?);
    packet.extend_from_slice(&[0, 0, 0, 0]);
    packet.extend_from_slice(question);
    for address in answers {
        let data = match address {
            IpAddr::V4(v4) => v4.octets().to_vec(),
            IpAddr::V6(v6) => v6.octets().to_vec(),
        };
        // The name of the question, class IN, 60 seconds.
        packet.extend_from_slice(&[0xc0, 12]);
        packet.extend_from_slice(kind);
        packet.extend_from_slice(&[0, 1, 0, 0, 0, 60, 0]);
        packet.push(u8::try_from(data.len()).ok()?);
        packet.extend_from_slice(&data);
    }
    Some((name, packet))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ignores_what_is_not_a_query() {
        assert_eq!(reply(&[0; 4], &[]), None);
        // An answer.
        let mut answer = [0; 17];
        answer[2] = 0x80;
        answer[5] = 1;
        assert_eq!(reply(&answer, &[]), None);
        // Cut off in the name.
        let mut cut = [0; 14];
        cut[5] = 1;
        cut[12] = 5;
        assert_eq!(reply(&cut, &[]), None);
    }
}
