//! Diagnoses a host end to end: whether the device knows it and sees it
//! connected, its address, its link and the mesh node it is attached to,
//! whether its name resolves to that address on this machine, and whether
//! its TCP ports answer. [`router`] checks the device itself.
//!
//! Towards the device, only getters are called. Name resolution and the TCP
//! probes run on this machine, behind [`Network`]. Host names are untrusted
//! strings and only shown as data.

use std::collections::BTreeSet;
use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use serde::Serialize;

use crate::generic::Error;
use crate::host::{self, Detail, Lookup};
use crate::read::Reader;
use crate::status::{self, Host};
use crate::wlan::{self, Views};

mod network;
mod router;

pub use network::{Fixed, Network, PROBE_TIMEOUT, Port, RESOLVE_TIMEOUT, System, is_private};
pub use router::{ROUTER, router};

/// A signal below this, in percent, is weak.
pub const WEAK_SIGNAL: u8 = 30;

/// The most candidates named when a name matches several hosts.
const MAX_CANDIDATES: usize = 10;

/// The outcome of a check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Pass,
    Fail,
    /// Not run, because it cannot make sense, like a link of a host that is
    /// not connected.
    Skip,
    /// Run, but without an answer either way.
    Unknown,
}

/// One check and its outcome.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Check {
    /// Like `active` or `TCP 445`.
    pub name: String,
    pub status: Status,
    /// What the check found.
    pub detail: Option<String>,
    /// On a failure, what is likely wrong.
    pub hint: Option<String>,
}

impl Check {
    pub(crate) fn new(name: impl Into<String>, status: Status, detail: Option<String>) -> Self {
        Self {
            name: name.into(),
            status,
            detail,
            hint: None,
        }
    }

    /// A failed check with its hint.
    pub(crate) fn fail(name: impl Into<String>, detail: Option<String>, hint: String) -> Self {
        Self {
            hint: Some(hint),
            ..Self::new(name, Status::Fail, detail)
        }
    }
}

/// The checks of a host or of the device.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Report {
    /// What was diagnosed: the host as given, or the address of the device.
    pub target: String,
    /// The host the device knows, if exactly one matched.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host: Option<Detail>,
    pub checks: Vec<Check>,
}

impl Report {
    /// The number of failed checks.
    #[must_use]
    pub fn failures(&self) -> usize {
        self.checks
            .iter()
            .filter(|c| c.status == Status::Fail)
            .count()
    }
}

/// What a diagnosis of a host may do.
#[derive(Debug, Clone, Default)]
pub struct Options {
    /// The TCP ports to probe on the host.
    pub ports: Vec<u16>,
    /// Probe only private addresses.
    pub private_only: bool,
}

/// The hosts a name or address matches.
enum Found {
    None,
    One(Box<Detail>),
    Many(Vec<Host>),
}

/// Diagnoses the host that `query` names: a MAC address, an IPv4 address
/// or a name, see [`host::named`].
///
/// # Errors
///
/// The errors of reading the device, see [`host::lookup`]. The outcome of
/// the checks is in the report.
pub async fn host(
    reader: &mut Reader,
    network: &impl Network,
    query: &str,
    options: &Options,
) -> Result<Report, Error> {
    let mut report = Report {
        target: query.to_owned(),
        host: None,
        checks: Vec::new(),
    };
    let detail = match find(reader, query).await? {
        Found::One(detail) => *detail,
        Found::None => {
            let hint = format!(
                "the device knows no host {query:?}; check the name, or give its MAC or IP address"
            );
            report.checks.push(Check::fail(KNOWN, None, hint));
            return Ok(report);
        }
        Found::Many(hosts) => {
            report.checks.push(ambiguous(&hosts));
            return Ok(report);
        }
    };
    let signal = signal(reader, &detail).await?;
    report.checks = checks(&detail, signal, network, options).await;
    report.host = Some(detail);
    Ok(report)
}

const KNOWN: &str = "known to the device";
const ACTIVE: &str = "active";
const ADDRESS: &str = "address";
const LINK: &str = "link";
const RESOLVES: &str = "name resolves";
const PORTS: &str = "ports";

/// The host that `query` names, with its details.
async fn find(reader: &mut Reader, query: &str) -> Result<Found, Error> {
    if let Ok(lookup) = Lookup::mac(query).or_else(|_| Lookup::ip(query)) {
        let detail = host::lookup(reader, &lookup).await?;
        return Ok(detail.map_or(Found::None, |d| Found::One(Box::new(d))));
    }
    let hosts = status::hosts(reader).await?.hosts;
    let mut named = host::named(hosts, query);
    match named.len() {
        0 => Ok(Found::None),
        1 => {
            let found = named.remove(0);
            Ok(Found::One(Box::new(host::detail(reader, found).await?)))
        }
        _ => Ok(Found::Many(named)),
    }
}

/// The failed check of a name that matches several hosts.
fn ambiguous(hosts: &[Host]) -> Check {
    let names: Vec<String> = hosts
        .iter()
        .take(MAX_CANDIDATES)
        .map(|h| {
            let name = h.name.as_deref().unwrap_or("?");
            let address = h.ip.as_deref().or(h.mac.as_deref()).unwrap_or("?");
            format!("{name} ({address})")
        })
        .collect();
    let more = hosts.len().saturating_sub(MAX_CANDIDATES);
    let more = if more > 0 {
        format!(" and {more} more")
    } else {
        String::new()
    };
    Check::fail(
        KNOWN,
        Some(format!("{} hosts match", hosts.len())),
        format!(
            "give one by its MAC or IP address: {}{more}",
            names.join(", ")
        ),
    )
}

/// The signal of a host connected to a wireless network of the device, in
/// percent.
async fn signal(reader: &mut Reader, detail: &Detail) -> Result<Option<u8>, Error> {
    let host = &detail.host;
    let (true, Some(mac)) = (host.active && is_wireless(detail), host.mac.as_deref()) else {
        return Ok(None);
    };
    let views = Views {
        clients: true,
        details: false,
    };
    let networks = wlan::wlans(reader, views).await?;
    Ok(networks
        .iter()
        .flat_map(|n| n.clients.iter().flatten())
        .find(|c| {
            c.mac
                .as_deref()
                .is_some_and(|m| m.eq_ignore_ascii_case(mac))
        })
        .and_then(|c| c.signal))
}

/// Whether the host is connected over WLAN.
fn is_wireless(detail: &Detail) -> bool {
    link_kind(detail).as_deref() == Some("WLAN")
}

/// `LAN` or `WLAN`, from the mesh or else from the interface of the host.
fn link_kind(detail: &Detail) -> Option<String> {
    let attached = detail.attached_to.as_ref().and_then(|a| a.kind.clone());
    attached.or_else(|| {
        detail
            .host
            .interface
            .as_deref()
            .map(|interface| match interface {
                "Ethernet" => "LAN".to_owned(),
                "802.11" => "WLAN".to_owned(),
                other => other.to_owned(),
            })
    })
}

/// All checks of a host the device knows.
async fn checks(
    detail: &Detail,
    signal: Option<u8>,
    network: &impl Network,
    options: &Options,
) -> Vec<Check> {
    let host = &detail.host;
    let ip = host
        .ip
        .as_deref()
        .and_then(|ip| ip.parse::<Ipv4Addr>().ok());
    let mut checks = vec![
        known(host),
        active(host),
        address(host, ip),
        link(detail, signal),
    ];
    let name = host.name.as_deref().filter(|n| is_dns_name(n));
    let resolved = match name {
        Some(name) => Some(network.resolve(name).await),
        None => None,
    };
    checks.push(resolution(host.name.as_deref(), ip, resolved));
    checks.extend(ports(network, host.active, ip, options).await);
    checks
}

fn known(host: &Host) -> Check {
    let name = host.name.as_deref().unwrap_or("(no name)");
    let detail = match &host.address_source {
        Some(source) => format!("{name} ({source})"),
        None => name.to_owned(),
    };
    Check::new(KNOWN, Status::Pass, Some(detail))
}

fn active(host: &Host) -> Check {
    if host.active {
        return Check::new(ACTIVE, Status::Pass, None);
    }
    Check::fail(
        ACTIVE,
        Some("not connected".to_owned()),
        "the device does not see the host connected now: it is off, asleep, out of range or \
         connected to another network"
            .to_owned(),
    )
}

fn address(host: &Host, ip: Option<Ipv4Addr>) -> Check {
    let Some(ip) = ip else {
        return match &host.ip {
            Some(other) => Check::new(ADDRESS, Status::Unknown, Some(other.clone())),
            None => Check::fail(
                ADDRESS,
                None,
                "the device knows no IPv4 address of the host; it may not have asked for one by \
                 DHCP"
                    .to_owned(),
            ),
        };
    };
    let parts = [
        Some(ip.to_string()),
        host.address_source.clone(),
        (!host.active).then(|| "last known".to_owned()),
    ];
    let detail = parts.into_iter().flatten().collect::<Vec<_>>().join(", ");
    Check::new(ADDRESS, Status::Pass, Some(detail))
}

fn link(detail: &Detail, signal: Option<u8>) -> Check {
    if !detail.host.active {
        return Check::new(LINK, Status::Skip, Some("not connected".to_owned()));
    }
    let attached = detail.attached_to.as_ref();
    let band = attached.and_then(|a| a.band.clone());
    let kind = [link_kind(detail), band]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" ");
    let node = attached.and_then(|a| a.node.as_deref());
    let parts: Vec<String> = [
        Some(kind).filter(|k| !k.is_empty()),
        detail.host.speed.map(|s| format!("{s} Mbit/s")),
        signal.map(|s| format!("signal {s}%")),
        node.map(|n| format!("via {n}")),
    ]
    .into_iter()
    .flatten()
    .collect();
    if parts.is_empty() {
        return Check::new(LINK, Status::Unknown, None);
    }
    let text = Some(parts.join(", "));
    match signal.filter(|s| *s < WEAK_SIGNAL) {
        Some(weak) => Check::fail(
            LINK,
            text,
            format!(
                "the host is attached to {} with a weak signal of {weak}%; move it closer or add \
                 a repeater",
                node.unwrap_or("the device")
            ),
        ),
        None => Check::new(LINK, Status::Pass, text),
    }
}

/// Whether `name` can be sent to a resolver: labels of letters, digits and
/// hyphens, separated by dots.
fn is_dns_name(name: &str) -> bool {
    name.len() <= 253
        && name.trim_end_matches('.').split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        })
}

fn resolution(
    name: Option<&str>,
    ip: Option<Ipv4Addr>,
    resolved: Option<io::Result<Vec<IpAddr>>>,
) -> Check {
    let Some(resolved) = resolved else {
        let detail = match name {
            Some(_) => "the name is not a DNS name",
            None => "the host has no name",
        };
        return Check::new(RESOLVES, Status::Skip, Some(detail.to_owned()));
    };
    let addresses = resolved.unwrap_or_default();
    let name = name.unwrap_or_default();
    if addresses.is_empty() {
        return Check::fail(
            RESOLVES,
            Some("does not resolve".to_owned()),
            format!(
                "{name} does not resolve on this machine; use the device as DNS server, or the \
                 address"
            ),
        );
    }
    let list = addresses
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ");
    let Some(ip) = ip else {
        return Check::new(RESOLVES, Status::Unknown, Some(list));
    };
    if addresses.contains(&IpAddr::V4(ip)) {
        return Check::new(RESOLVES, Status::Pass, Some(ip.to_string()));
    }
    Check::fail(
        RESOLVES,
        Some(format!("{list} (device: {ip})")),
        format!("DNS returns {list}, the device knows {ip}"),
    )
}

/// The checks of the TCP ports, or why they are skipped.
async fn ports(
    network: &impl Network,
    active: bool,
    ip: Option<Ipv4Addr>,
    options: &Options,
) -> Vec<Check> {
    let skip = |detail: String| vec![Check::new(PORTS, Status::Skip, Some(detail))];
    if options.ports.is_empty() {
        return skip("no port given".to_owned());
    }
    if !active {
        return skip("not connected".to_owned());
    }
    let Some(ip) = ip else {
        return skip("no IPv4 address".to_owned());
    };
    if options.private_only && !is_private(IpAddr::V4(ip)) {
        return skip(format!("{ip} is not a private address"));
    }
    let mut checks = Vec::new();
    for port in options.ports.iter().copied().collect::<BTreeSet<u16>>() {
        let state = network.connect(SocketAddr::new(IpAddr::V4(ip), port)).await;
        checks.push(port_check(port, state));
    }
    checks
}

fn port_check(port: u16, state: Port) -> Check {
    let name = format!("TCP {port}");
    match state {
        Port::Open => Check::new(name, Status::Pass, Some("open".to_owned())),
        Port::Closed => Check::fail(
            name,
            Some("closed".to_owned()),
            format!(
                "the host refuses port {port}: the service is not running or listens on another port"
            ),
        ),
        Port::Filtered => Check::fail(
            name,
            Some("filtered".to_owned()),
            format!(
                "no answer on port {port} within {} s: a firewall on the host drops it, or the \
                 host cannot be reached from here",
                PROBE_TIMEOUT.as_secs()
            ),
        ),
    }
}

#[cfg(test)]
mod tests;
