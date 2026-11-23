//! What a diagnosis does from this machine, outside the device: resolve a
//! name with the resolver of the operating system and connect to a TCP
//! port. Behind a trait, so that tests use a fake resolver and loopback.

use std::collections::BTreeSet;
use std::io;
use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

use serde::Serialize;

/// How long a TCP connect may take before the port counts as filtered.
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(1);

/// How long the resolver may take.
pub const RESOLVE_TIMEOUT: Duration = Duration::from_secs(5);

/// The state of a TCP port.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Port {
    /// The connection was made.
    Open,
    /// The host refused it.
    Closed,
    /// No answer in time, or the host cannot be reached.
    Filtered,
}

/// The network as this machine sees it.
pub trait Network {
    /// The addresses of `name`, by the resolver of the operating system.
    fn resolve(&self, name: &str) -> impl Future<Output = io::Result<Vec<IpAddr>>> + Send;

    /// Connects to `address` and closes the connection right away.
    fn connect(&self, address: SocketAddr) -> impl Future<Output = Port> + Send;
}

/// The network of the operating system.
#[derive(Debug, Clone, Copy)]
pub struct System {
    /// How long a TCP connect may take.
    pub timeout: Duration,
}

impl Default for System {
    fn default() -> Self {
        Self {
            timeout: PROBE_TIMEOUT,
        }
    }
}

impl Network for System {
    async fn resolve(&self, name: &str) -> io::Result<Vec<IpAddr>> {
        let lookup = tokio::net::lookup_host((name, 0));
        let found = tokio::time::timeout(RESOLVE_TIMEOUT, lookup).await;
        let found = found.unwrap_or_else(|_| Err(io::ErrorKind::TimedOut.into()))?;
        let addresses: BTreeSet<IpAddr> = found.map(|a| a.ip()).collect();
        Ok(addresses.into_iter().collect())
    }

    async fn connect(&self, address: SocketAddr) -> Port {
        let connect = tokio::net::TcpStream::connect(address);
        let result = tokio::time::timeout(self.timeout, connect).await.ok();
        state(result.map(|r| r.map(drop)))
    }
}

/// The state of a port after a connect, `None` if it timed out.
fn state(result: Option<io::Result<()>>) -> Port {
    match result {
        Some(Ok(())) => Port::Open,
        Some(Err(e)) if e.kind() == io::ErrorKind::ConnectionRefused => Port::Closed,
        _ => Port::Filtered,
    }
}

/// A network with fixed answers, for tests: the names resolve to their
/// addresses, others not at all; the ports have their states, others are
/// filtered.
#[derive(Debug, Clone, Default)]
pub struct Fixed {
    pub names: Vec<(String, Vec<IpAddr>)>,
    pub ports: Vec<(SocketAddr, Port)>,
}

impl Network for Fixed {
    fn resolve(&self, name: &str) -> impl Future<Output = io::Result<Vec<IpAddr>>> + Send {
        let found = self
            .names
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, addresses)| addresses.clone())
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "unknown name"));
        std::future::ready(found)
    }

    fn connect(&self, address: SocketAddr) -> impl Future<Output = Port> + Send {
        let port = self
            .ports
            .iter()
            .find(|(a, _)| *a == address)
            .map_or(Port::Filtered, |(_, port)| *port);
        std::future::ready(port)
    }
}

/// Whether an address is only used inside a home network or on this
/// machine: private, link-local or loopback, and unique local for IPv6.
#[must_use]
pub fn is_private(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(v4) => v4.is_private() || v4.is_link_local() || v4.is_loopback(),
        IpAddr::V6(v6) => {
            let first = v6.segments()[0];
            v6.is_loopback() || (first & 0xFE00) == 0xFC00 || (first & 0xFFC0) == 0xFE80
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn knows_private_addresses() {
        for private in [
            "192.168.178.1",
            "10.0.0.1",
            "169.254.1.1",
            "127.0.0.1",
            "fd00::1",
            "fe80::1",
            "::1",
        ] {
            assert!(is_private(private.parse().unwrap()), "{private}");
        }
        for public in ["192.0.2.1", "8.8.8.8", "2001:db8::1"] {
            assert!(!is_private(public.parse().unwrap()), "{public}");
        }
    }

    #[tokio::test]
    async fn probes_ports_on_loopback() {
        let system = System::default();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let open = listener.local_addr().unwrap();
        assert_eq!(system.connect(open).await, Port::Open);
        drop(listener);
        assert_eq!(system.connect(open).await, Port::Closed);
        assert_eq!(state(None), Port::Filtered);
        let unreachable = io::Error::from(io::ErrorKind::HostUnreachable);
        assert_eq!(state(Some(Err(unreachable))), Port::Filtered);
    }

    #[tokio::test]
    async fn resolves_names() {
        let system = System::default();
        let addresses = system.resolve("localhost").await.unwrap();
        assert!(addresses.iter().all(IpAddr::is_loopback), "{addresses:?}");
        assert_ne!(addresses, Vec::<IpAddr>::new());
    }
}
