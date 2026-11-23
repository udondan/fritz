//! Why a name does or does not resolve: the view of the router (its local
//! domain, the DNS servers it hands out and its host table), the answers of
//! the DNS server of the router and of the resolver of this machine, the
//! resolver configuration of this machine, and a verdict that names the
//! likely cause.
//!
//! Only the given name is asked for, plus the same name with the local
//! domain of the router if it has no dot. Nothing is changed, neither on the
//! device nor on this machine; the verdict only suggests what to change.
//! The queries and the reading of the configuration go through [`Probes`],
//! so tests can replace them.

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

use serde::Serialize;

use crate::generic::Error;
use crate::read::Reader;
use crate::status;

mod client;
mod wire;

pub use client::{ClientConfig, Resolver, parse_resolv_conf, parse_scutil};
pub use wire::{Failure, Record, query};

/// How long one query to the DNS server of the router may take.
pub const QUERY_TIMEOUT: Duration = Duration::from_secs(2);

/// How long the resolver of this machine may take.
pub const RESOLVE_TIMEOUT: Duration = Duration::from_secs(4);

/// How long all queries together may take. They run at the same time.
pub const TOTAL_TIMEOUT: Duration = Duration::from_secs(6);

/// The DNS queries and the resolver configuration of this machine.
pub trait Probes: Sync {
    /// Asks `server` for the records of `kind` of `name`.
    fn query(
        &self,
        server: SocketAddr,
        name: &str,
        kind: Record,
    ) -> impl Future<Output = Result<Vec<IpAddr>, Failure>> + Send;

    /// Resolves `name` like the programs of this machine do.
    fn resolve(&self, name: &str) -> impl Future<Output = Result<Vec<IpAddr>, Failure>> + Send;

    /// The resolver configuration of this machine, `None` if unknown.
    fn client_config(&self) -> impl Future<Output = Option<ClientConfig>> + Send;
}

/// The probes of this machine: UDP queries, the resolver of the operating
/// system and its configuration files.
#[derive(Debug, Clone, Copy, Default)]
pub struct System;

impl Probes for System {
    async fn query(
        &self,
        server: SocketAddr,
        name: &str,
        kind: Record,
    ) -> Result<Vec<IpAddr>, Failure> {
        wire::query(server, name, kind, QUERY_TIMEOUT).await
    }

    async fn resolve(&self, name: &str) -> Result<Vec<IpAddr>, Failure> {
        let lookup = async {
            let addresses = tokio::net::lookup_host((name, 0)).await?;
            Ok(addresses.collect())
        };
        resolved(tokio::time::timeout(RESOLVE_TIMEOUT, lookup).await.ok())
    }

    async fn client_config(&self) -> Option<ClientConfig> {
        client::read().await
    }
}

/// The addresses the resolver found; `None` if it took too long.
fn resolved(result: Option<std::io::Result<Vec<SocketAddr>>>) -> Result<Vec<IpAddr>, Failure> {
    match result {
        None => Err(Failure::Timeout),
        Some(Err(error)) => Err(Failure::Network(error.to_string())),
        Some(Ok(addresses)) => Ok(addresses.iter().map(SocketAddr::ip).collect()),
    }
}

/// A name that is not valid, refused before anything is asked.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error(
    "{0:?} is not a host name: letters, digits, `-` and `_` in labels of up to 63 \
     characters, separated by dots"
)]
pub struct InvalidName(pub String);

/// Checks a host name, without a dot at the end.
///
/// # Errors
///
/// [`InvalidName`] for anything but a host name, also for addresses.
pub fn check_name(text: &str) -> Result<String, InvalidName> {
    let name = text.strip_suffix('.').unwrap_or(text);
    // An empty name has one empty label.
    let valid = name.len() <= 253 && name.split('.').all(is_label);
    if valid && name.parse::<IpAddr>().is_err() {
        Ok(name.to_owned())
    } else {
        Err(InvalidName(text.to_owned()))
    }
}

/// Whether `label` is 1 to 63 letters, digits, `-` or `_`.
fn is_label(label: &str) -> bool {
    (1..=63).contains(&label.len())
        && label
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// A DNS server like `192.168.178.1`, `192.168.178.1:53` or `[fd00::1]:53`.
/// Without a port, port 53.
///
/// # Errors
///
/// A message for anything else.
pub fn parse_server(text: &str) -> Result<SocketAddr, String> {
    text.parse::<SocketAddr>()
        .or_else(|_| text.parse::<IpAddr>().map(|ip| SocketAddr::new(ip, 53)))
        .map_err(|_| format!("{text:?} is not an address like 192.168.178.1 or 192.168.178.1:53"))
}

/// What to check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    /// A name checked with [`check_name`].
    pub name: String,
    /// The address the answers should have.
    pub expected: Option<IpAddr>,
    /// The DNS server asked as the router, instead of the LAN address of
    /// the device.
    pub router_dns: Option<SocketAddr>,
}

/// The evidence and the verdict.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Report {
    pub name: String,
    pub router: RouterView,
    /// The answer of the DNS server of the router.
    pub router_answer: Answer,
    /// The answer of the resolver of this machine.
    pub client_answer: Answer,
    /// The resolver configuration of this machine, `None` if unknown.
    pub client_config: Option<ClientConfig>,
    /// Whether the answers have the expected address.
    pub expected: Option<Expected>,
    pub verdict: Verdict,
}

/// What the router knows about the name.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct RouterView {
    /// The local domain, like `fritz.box`.
    pub domain: Option<String>,
    /// The DNS servers it hands out over DHCP.
    pub dns_servers: Vec<String>,
    /// The address of the device in the LAN.
    pub address: Option<String>,
    /// The host of this name in the host table, if there is one.
    pub host: Option<HostEntry>,
}

/// A host of the host table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HostEntry {
    pub name: String,
    pub ip: Option<String>,
    pub active: bool,
}

/// What a query found.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Answered,
    /// The name does not exist.
    NoSuchName,
    /// The name exists, without addresses.
    NoAddress,
    /// No answer: a timeout, an error of the server or of the network.
    Failed,
    /// Not asked.
    Skipped,
}

/// The answer to the A and AAAA queries of a name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Answer {
    /// The DNS server asked; `None` for the resolver of this machine.
    pub server: Option<String>,
    /// The name asked for.
    pub name: String,
    pub status: Status,
    pub addresses: Vec<IpAddr>,
    /// Why there are no addresses.
    pub note: Option<String>,
}

impl Answer {
    /// An answer for a query that was not made.
    fn skipped(name: &str, server: Option<String>, note: &str) -> Self {
        Self {
            server,
            name: name.to_owned(),
            status: Status::Skipped,
            addresses: Vec::new(),
            note: Some(note.to_owned()),
        }
    }

    /// The answer from the results of the queries of `name`.
    fn from_results(
        name: &str,
        server: Option<String>,
        results: Vec<Result<Vec<IpAddr>, Failure>>,
    ) -> Self {
        let mut addresses = Vec::new();
        let mut failures = Vec::new();
        for result in results {
            match result {
                Ok(found) => addresses.extend(found),
                Err(failure) => failures.push(failure),
            }
        }
        let mut seen = BTreeSet::new();
        addresses.retain(|a| seen.insert(*a));
        let (status, failure) = if !addresses.is_empty() {
            (Status::Answered, None)
        } else if let Some(failure) = failures
            .iter()
            .find(|f| !matches!(f, Failure::NoSuchName | Failure::NoRecord))
        {
            (Status::Failed, Some(failure))
        } else if let Some(failure) = failures.iter().find(|f| **f == Failure::NoSuchName) {
            (Status::NoSuchName, Some(failure))
        } else {
            (Status::NoAddress, failures.first())
        };
        Self {
            server,
            name: name.to_owned(),
            status,
            addresses,
            note: failure.map(ToString::to_string),
        }
    }

    fn answered(&self) -> bool {
        self.status == Status::Answered
    }

    /// The addresses, separated by commas.
    fn list(&self) -> String {
        join(&self.addresses)
    }
}

/// Whether the answers have the expected address; `None` for an answer
/// without addresses.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Expected {
    pub address: IpAddr,
    pub router: Option<bool>,
    pub client: Option<bool>,
}

/// The likely cause.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Cause {
    /// Both answer the same.
    Ok,
    /// A `.local` name, resolved by mDNS.
    Mdns,
    /// The router answers, this machine asks another server that does not
    /// forward the local domain to the router.
    NotForwarded,
    /// A name without a dot fails, because the search domain is missing.
    SearchDomainMissing,
    /// The router answers, this machine does not, for an unknown reason.
    ClientFails,
    /// This machine answers, the router does not.
    ClientOnly,
    /// Both answer, with different addresses.
    StaleRecord,
    /// Both answer the same, but not the expected address.
    UnexpectedAddress,
    /// The host is in the host table, but the DNS server of the router does
    /// not answer for it.
    RouterNoRecord,
    /// Nobody knows the name.
    UnknownName,
    /// The DNS server of the router did not answer.
    RouterUnreachable,
    /// The DNS server of the router is unknown.
    RouterUnknown,
}

/// The likely cause and what it means.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Verdict {
    pub cause: Cause,
    pub message: String,
}

impl Verdict {
    fn new(cause: Cause, message: impl Into<String>) -> Self {
        Self {
            cause,
            message: message.into(),
        }
    }
}

/// Checks the name of `request`: reads the router view through `reader`,
/// asks the router and this machine through `probes`, and judges the
/// answers.
///
/// # Errors
///
/// The errors of reading the device other than missing parts, see
/// [`Reader::try_read`].
pub async fn diagnose(
    reader: &mut Reader,
    probes: &impl Probes,
    request: &Request,
) -> Result<Report, Error> {
    let name = request.name.as_str();
    let mut router = router_view(reader).await?;
    router.host = host_entry(&status::hosts(reader).await?.hosts, name, &router);
    let server = request.router_dns.or_else(|| {
        router
            .address
            .as_deref()
            .and_then(|a| a.parse::<IpAddr>().ok())
            .map(|ip| SocketAddr::new(ip, 53))
    });
    let (router_answer, client_answer, client_config) =
        ask(probes, name, router.domain.as_deref(), server).await;
    let expected = request.expected.map(|address| Expected {
        address,
        router: router_answer
            .answered()
            .then(|| router_answer.addresses.contains(&address)),
        client: client_answer
            .answered()
            .then(|| client_answer.addresses.contains(&address)),
    });
    let mut report = Report {
        name: name.to_owned(),
        router,
        router_answer,
        client_answer,
        client_config,
        expected,
        verdict: Verdict::new(Cause::Ok, String::new()),
    };
    report.verdict = verdict(&report);
    Ok(report)
}

/// The local domain, the DNS servers handed out and the address of the
/// device, from `LANHostConfigManagement:GetInfo`.
async fn router_view(reader: &mut Reader) -> Result<RouterView, Error> {
    let info = reader
        .try_read("LANHostConfigManagement1", "GetInfo", &[])
        .await?
        .unwrap_or_default();
    let list = |name| {
        info.get(name).map_or_else(Vec::new, |v| {
            v.split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .collect::<Vec<_>>()
        })
    };
    Ok(RouterView {
        domain: info
            .text("NewDomainName")
            .map(|d| d.trim().trim_matches('.').to_owned())
            .filter(|d| !d.is_empty() && d != "(none)"),
        dns_servers: list("NewDNSServers"),
        address: list("NewIPRouters").into_iter().next(),
        host: None,
    })
}

/// The host of the host table that `name` names: the name itself without a
/// dot, or its first label in the local domain or in `.local`. Active
/// hosts first.
fn host_entry(hosts: &[status::Host], name: &str, router: &RouterView) -> Option<HostEntry> {
    let label = host_label(name, router.domain.as_deref())?;
    let mut matches: Vec<&status::Host> = hosts
        .iter()
        .filter(|h| {
            h.name
                .as_deref()
                .is_some_and(|n| n.eq_ignore_ascii_case(label))
        })
        .collect();
    matches.sort_by_key(|h| !h.active);
    matches.first().map(|host| HostEntry {
        name: host.name.clone().unwrap_or_default(),
        ip: host.ip.clone(),
        active: host.active,
    })
}

/// The name of a host in the host table that `name` stands for.
fn host_label<'a>(name: &'a str, domain: Option<&str>) -> Option<&'a str> {
    let Some((label, rest)) = name.split_once('.') else {
        return Some(name);
    };
    let rest = rest.to_ascii_lowercase();
    (rest == "local" || domain.is_some_and(|d| d.eq_ignore_ascii_case(&rest))).then_some(label)
}

/// Whether `name` is resolved by mDNS.
fn is_mdns(name: &str) -> bool {
    name.rsplit('.')
        .next()
        .is_some_and(|label| label.eq_ignore_ascii_case("local"))
}

/// Asks the router and this machine at the same time, and reads the
/// configuration of this machine, within [`TOTAL_TIMEOUT`].
async fn ask(
    probes: &impl Probes,
    name: &str,
    domain: Option<&str>,
    server: Option<SocketAddr>,
) -> (Answer, Answer, Option<ClientConfig>) {
    // A name without a dot gets the local domain on clients that search
    // it; the router is asked for that full name.
    let full = match domain {
        Some(domain) if !name.contains('.') => format!("{name}.{domain}"),
        _ => name.to_owned(),
    };
    let shown = server.map(show_server);
    if is_mdns(name) {
        let note = "a .local name is resolved by mDNS, not asked by DNS";
        return (
            Answer::skipped(&full, shown, note),
            Answer::skipped(name, None, note),
            probes.client_config().await,
        );
    }
    let router = async {
        let Some(server) = server else {
            return Answer::skipped(&full, None, "the address of the router is unknown");
        };
        let (a, aaaa) = tokio::join!(
            probes.query(server, &full, Record::A),
            probes.query(server, &full, Record::Aaaa)
        );
        Answer::from_results(&full, shown.clone(), vec![a, aaaa])
    };
    let client = async { Answer::from_results(name, None, vec![probes.resolve(name).await]) };
    let all = async { tokio::join!(router, client, probes.client_config()) };
    tokio::time::timeout(TOTAL_TIMEOUT, all)
        .await
        .unwrap_or_else(|_| {
            let failed = |name: &str, server| Answer {
                server,
                name: name.to_owned(),
                status: Status::Failed,
                addresses: Vec::new(),
                note: Some(Failure::Timeout.to_string()),
            };
            (failed(&full, shown.clone()), failed(name, None), None)
        })
}

/// A server, without the port if it is 53.
fn show_server(server: SocketAddr) -> String {
    if server.port() == 53 {
        server.ip().to_string()
    } else {
        server.to_string()
    }
}

/// Addresses, separated by commas.
fn join(addresses: &[IpAddr]) -> String {
    addresses
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

/// Whether two answers differ: in a family of addresses both have, they
/// have different addresses. A family only one of them has is not
/// compared, as clients without IPv6 do not ask for AAAA records.
fn differ(one: &[IpAddr], other: &[IpAddr]) -> bool {
    [true, false].into_iter().any(|v4| {
        let family = |list: &[IpAddr]| -> BTreeSet<IpAddr> {
            list.iter().copied().filter(|a| a.is_ipv4() == v4).collect()
        };
        let (one, other) = (family(one), family(other));
        !one.is_empty() && !other.is_empty() && one != other
    })
}

/// Judges the evidence of `report`.
pub fn verdict(report: &Report) -> Verdict {
    let router = &report.router_answer;
    let client = &report.client_answer;
    if is_mdns(&report.name) {
        return mdns(report);
    }
    if router.status == Status::Skipped {
        return Verdict::new(
            Cause::RouterUnknown,
            "the DNS server of the router is unknown, so it was not asked; give it with \
             --router-dns",
        );
    }
    match (router.answered(), client.answered()) {
        (true, true) => both_answer(report),
        (true, false) => client_fails(report),
        (false, true) => Verdict::new(
            Cause::ClientOnly,
            format!(
                "this machine resolves the name ({}), but the router does not ({}): the \
                 answer comes from elsewhere, like /etc/hosts, another DNS server or a VPN",
                client.list(),
                router.note.as_deref().unwrap_or("no address"),
            ),
        ),
        (false, false) => nobody_answers(report),
    }
}

/// The verdict for a `.local` name.
fn mdns(report: &Report) -> Verdict {
    let mut message = "names in .local are resolved by mDNS (Bonjour, Avahi), not by DNS, \
                       so they were not checked"
        .to_owned();
    if let (Some(host), Some(domain)) = (&report.router.host, &report.router.domain) {
        let _ = write!(
            message,
            "; the router knows the host as {}.{domain}",
            host.name
        );
    }
    Verdict::new(Cause::Mdns, message)
}

/// The verdict when the router and this machine answer.
fn both_answer(report: &Report) -> Verdict {
    let router = &report.router_answer;
    let client = &report.client_answer;
    if differ(&router.addresses, &client.addresses) {
        return Verdict::new(
            Cause::StaleRecord,
            format!(
                "the router answers {} and this machine {}: one of them has a stale record, \
                 or this machine asks another server; clear the DNS cache of this machine",
                router.list(),
                client.list()
            ),
        );
    }
    match &report.expected {
        Some(expected) if expected.router == Some(false) || expected.client == Some(false) => {
            Verdict::new(
                Cause::UnexpectedAddress,
                format!(
                    "both answer {}, not {}: the host has another address now, or the \
                     record is stale",
                    router.list(),
                    expected.address
                ),
            )
        }
        _ => Verdict::new(
            Cause::Ok,
            format!("the router and this machine both answer {}", router.list()),
        ),
    }
}

/// The verdict when the router answers and this machine does not.
fn client_fails(report: &Report) -> Verdict {
    let name = report.name.as_str();
    let domain = report.router.domain.as_deref();
    let config = report.client_config.as_ref();
    if let (Some(domain), Some(config)) = (domain, config) {
        let searched = config
            .search()
            .iter()
            .any(|s| s.eq_ignore_ascii_case(domain));
        if !name.contains('.') && !searched {
            return Verdict::new(
                Cause::SearchDomainMissing,
                format!(
                    "the router answers {name}.{domain}, but this machine does not add the \
                     search domain {domain} to {name}; use the full name, or add {domain} to \
                     the search domains of this machine"
                ),
            );
        }
    }
    let router_address = report
        .router_answer
        .server
        .as_deref()
        .or(report.router.address.as_deref());
    let servers = config
        .and_then(|c| c.resolver_for(&report.client_answer.name))
        .map_or_else(Vec::new, |r| r.nameservers.clone());
    let identified = !servers.is_empty()
        && servers
            .iter()
            .all(|s| s.parse::<IpAddr>().is_ok_and(|ip| ip.is_ipv4()))
        && !servers.iter().any(|s| Some(s.as_str()) == router_address);
    if identified {
        let zone = domain.unwrap_or(name);
        let router = router_address.unwrap_or("the router");
        let asks = if servers.iter().all(|s| client::is_local(s)) {
            format!(
                "a resolver on this machine ({}), whose upstream servers are unknown",
                servers.join(", ")
            )
        } else {
            servers.join(", ")
        };
        return Verdict::new(
            Cause::NotForwarded,
            format!(
                "the router answers, but this machine asks {asks}, which does not forward \
                 {zone} to the router; add conditional forwarding of {zone} to {router} there"
            ),
        );
    }
    Verdict::new(
        Cause::ClientFails,
        format!(
            "the router answers, but this machine does not resolve the name ({}); which \
             server it asks is unknown, a VPN or a local setting may take over name resolution",
            report.client_answer.note.as_deref().unwrap_or("no address")
        ),
    )
}

/// The verdict when neither the router nor this machine answer.
fn nobody_answers(report: &Report) -> Verdict {
    let router = &report.router_answer;
    if router.status == Status::Failed {
        return Verdict::new(
            Cause::RouterUnreachable,
            format!(
                "the DNS server of the router ({}) did not answer ({}); check the address, or \
                 give another with --router-dns",
                router.server.as_deref().unwrap_or("unknown"),
                router.note.as_deref().unwrap_or("no answer"),
            ),
        );
    }
    if let Some(host) = &report.router.host {
        return Verdict::new(
            Cause::RouterNoRecord,
            format!(
                "the router knows the host {} ({}, {}), but its DNS server does not answer \
                 for {}",
                host.name,
                host.ip.as_deref().unwrap_or("no address"),
                if host.active { "active" } else { "inactive" },
                router.name
            ),
        );
    }
    Verdict::new(
        Cause::UnknownName,
        "neither the router nor this machine resolve the name, and the router knows no host \
         of this name; the host may have another name on the router, see fritz hosts",
    )
}

#[cfg(test)]
mod tests;
