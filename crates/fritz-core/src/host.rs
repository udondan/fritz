//! One host, found by its MAC or IP address or by its name, with all the
//! device knows about it: the entry of the host, the host list and the mesh.
//!
//! Addresses are checked before they are sent. Host names are untrusted
//! strings.

use std::fmt;
use std::net::Ipv4Addr;

use serde::Serialize;

use crate::generic::Error;
use crate::lists::{self, Branch};
use crate::read::{Reader, Values};
use crate::status::{self, Host};

/// What a host is looked up by: an address the device can look up itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Lookup {
    /// A MAC address in capitals, separated by colons.
    Mac(String),
    Ip(Ipv4Addr),
}

/// An address that is not valid, refused before anything is sent.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InvalidAddress {
    #[error("{0:?} is not a MAC address like AA:BB:CC:DD:EE:FF")]
    Mac(String),
    #[error("{0:?} is not an IPv4 address like 192.168.178.23")]
    Ip(String),
}

impl Lookup {
    /// A lookup by MAC address: six pairs of hex digits, separated by
    /// colons or dashes, in any case.
    ///
    /// # Errors
    ///
    /// [`InvalidAddress::Mac`] for anything else.
    pub fn mac(text: &str) -> Result<Self, InvalidAddress> {
        let parts: Vec<&str> = text.split([':', '-']).collect();
        let valid = parts.len() == 6
            && parts
                .iter()
                .all(|p| p.len() == 2 && p.chars().all(|c| c.is_ascii_hexdigit()));
        if valid {
            Ok(Self::Mac(parts.join(":").to_ascii_uppercase()))
        } else {
            Err(InvalidAddress::Mac(text.to_owned()))
        }
    }

    /// A lookup by IPv4 address, the only kind the device looks up.
    ///
    /// # Errors
    ///
    /// [`InvalidAddress::Ip`] for anything else.
    pub fn ip(text: &str) -> Result<Self, InvalidAddress> {
        text.parse()
            .map(Self::Ip)
            .map_err(|_| InvalidAddress::Ip(text.to_owned()))
    }

    /// Whether `host` has this address.
    fn matches(&self, host: &Host) -> bool {
        match self {
            Self::Mac(mac) => host
                .mac
                .as_deref()
                .is_some_and(|m| m.eq_ignore_ascii_case(mac)),
            Self::Ip(ip) => host.ip.as_deref() == Some(ip.to_string().as_str()),
        }
    }
}

impl fmt::Display for Lookup {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Mac(mac) => write!(f, "the MAC address {mac}"),
            Self::Ip(ip) => write!(f, "the IP address {ip}"),
        }
    }
}

/// A host with all the device knows about it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Detail {
    #[serde(flatten)]
    pub host: Host,
    /// The seconds until the lease of the address ends. Left out if the
    /// device reports none.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lease_remaining: Option<u64>,
    /// The mesh node the host is connected to, if the mesh list names one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attached_to: Option<Attachment>,
}

/// How a host is connected to a node of the mesh.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Attachment {
    /// The name of the node.
    pub node: Option<String>,
    /// Like `LAN` or `WLAN`.
    pub kind: Option<String>,
    /// The interface the mesh list names for the link, like `AP:5G:0`.
    pub interface: Option<String>,
    /// The band of a wireless link, like `5 GHz`.
    pub band: Option<String>,
    /// The current rates towards the host and back, in kbit/s.
    pub rate_down: Option<u64>,
    pub rate_up: Option<u64>,
}

/// Looks up the host with an address: its item in the host list, its entry,
/// which the device looks up by the MAC address, and its link in the mesh.
/// For an IP address, the device first names the MAC address of the host.
/// `None` if neither the entries nor the host list have it.
///
/// # Errors
///
/// The errors of reading other than missing parts, see
/// [`Reader::try_read`], and [`Error::InvalidDocument`] for a list that
/// cannot be parsed.
pub async fn lookup(reader: &mut Reader, lookup: &Lookup) -> Result<Option<Detail>, Error> {
    let by_ip = match lookup {
        Lookup::Mac(_) => None,
        Lookup::Ip(ip) => {
            let ip = ip.to_string();
            let input = [("NewIPAddress", ip.as_str())];
            reader
                .try_read("Hosts1", "X_AVM-DE_GetSpecificHostEntryByIP", &input)
                .await?
        }
    };
    let listed = host_list(reader)
        .await?
        .into_iter()
        .find(|h| lookup.matches(h));
    let found = listed.is_some() || by_ip.is_some();
    let mut host = listed.unwrap_or_default();
    match lookup {
        Lookup::Mac(mac) => host.mac = host.mac.or_else(|| Some(mac.clone())),
        Lookup::Ip(ip) => host.ip = host.ip.or_else(|| Some(ip.to_string())),
    }
    let host = match &by_ip {
        Some(by_ip) => overlay(host, by_ip),
        None => host,
    };
    let entry = entry(reader, host.mac.as_deref()).await?;
    if !found && entry.is_none() {
        return Ok(None);
    }
    Ok(Some(complete(reader, host, entry.as_ref()).await?))
}

/// The details of a host from the host list: its entry, which the device
/// looks up by its MAC address, and its link in the mesh.
///
/// # Errors
///
/// Like [`lookup`].
pub async fn detail(reader: &mut Reader, host: Host) -> Result<Detail, Error> {
    let entry = entry(reader, host.mac.as_deref()).await?;
    complete(reader, host, entry.as_ref()).await
}

/// The entry of the host with the MAC address `mac`, if the device has one.
async fn entry(reader: &mut Reader, mac: Option<&str>) -> Result<Option<Values>, Error> {
    let Some(mac) = mac else { return Ok(None) };
    let input = [("NewMACAddress", mac)];
    reader
        .try_read("Hosts1", "GetSpecificHostEntry", &input)
        .await
}

/// The host with the values of its entry, and its link in the mesh.
async fn complete(
    reader: &mut Reader,
    host: Host,
    entry: Option<&Values>,
) -> Result<Detail, Error> {
    let host = match entry {
        Some(entry) => overlay(host, entry),
        None => host,
    };
    let lease_remaining = entry
        .and_then(|e| e.number("NewLeaseTimeRemaining"))
        .filter(|seconds| *seconds > 0);
    let attached_to = match &host.mac {
        Some(mac) => status::mesh(reader)
            .await?
            .and_then(|mesh| attachment(&mesh.tree(), mac)),
        None => None,
    };
    Ok(Detail {
        host,
        lease_remaining,
        attached_to,
    })
}

/// The hosts of the host list, or none if the device has no host list.
async fn host_list(reader: &mut Reader) -> Result<Vec<Host>, Error> {
    let xml = reader
        .document(
            "Hosts1",
            "X_AVM-DE_GetHostListPath",
            "NewX_AVM-DE_HostListPath",
        )
        .await?;
    xml.map_or_else(|| Ok(Vec::new()), |xml| lists::hosts(&xml))
}

/// The host with the values the entry has.
fn overlay(host: Host, entry: &Values) -> Host {
    let text = |name: &str| entry.text(name);
    Host {
        name: text("NewHostName").or(host.name),
        ip: text("NewIPAddress").or(host.ip),
        mac: text("NewMACAddress").or(host.mac),
        active: entry.flag("NewActive").unwrap_or(host.active),
        interface: text("NewInterfaceType").or(host.interface),
        address_source: text("NewAddressSource").or(host.address_source),
        guest: entry.flag("NewX_AVM-DE_Guest").or(host.guest),
        internet_access: text("NewX_AVM-DE_WANAccess").or(host.internet_access),
        speed: entry
            .number("NewX_AVM-DE_Speed")
            .filter(|speed| *speed > 0)
            .or(host.speed),
    }
}

/// The node above the device with the MAC address `mac` in the tree of the
/// mesh, and the link between them.
fn attachment(branches: &[Branch], mac: &str) -> Option<Attachment> {
    branches.iter().find_map(|parent| {
        parent
            .children
            .iter()
            .find(|child| (child.node.mac.as_deref()).is_some_and(|m| m.eq_ignore_ascii_case(mac)))
            .map(|child| {
                let uplink = child.uplinks.first().cloned().unwrap_or_default();
                Attachment {
                    node: parent.node.name.clone(),
                    band: uplink.interface.as_deref().and_then(band),
                    kind: uplink.kind,
                    interface: uplink.interface,
                    rate_down: uplink.rate_down,
                    rate_up: uplink.rate_up,
                }
            })
            .or_else(|| attachment(&parent.children, mac))
    })
}

/// The band of a wireless interface like `AP:5G:0`.
fn band(interface: &str) -> Option<String> {
    interface.split(':').find_map(|part| match part {
        "2G" => Some("2.4 GHz".to_owned()),
        "5G" => Some("5 GHz".to_owned()),
        "6G" => Some("6 GHz".to_owned()),
        _ => None,
    })
}

/// The hosts named `name`, ignoring case: those with exactly this name, or
/// if there are none, those whose name contains it.
#[must_use]
pub fn named(hosts: Vec<Host>, name: &str) -> Vec<Host> {
    let name = name.to_lowercase();
    let lower = |host: &Host| host.name.as_deref().map(str::to_lowercase);
    let (exact, rest): (Vec<Host>, Vec<Host>) = hosts
        .into_iter()
        .partition(|h| lower(h).as_deref() == Some(name.as_str()));
    if exact.is_empty() {
        rest.into_iter()
            .filter(|h| lower(h).is_some_and(|n| n.contains(&name)))
            .collect()
    } else {
        exact
    }
}

/// Whether the name, IP or MAC address of `host` contains `text`, ignoring
/// case.
#[must_use]
pub fn contains(host: &Host, text: &str) -> bool {
    let text = text.to_lowercase();
    [&host.name, &host.ip, &host.mac]
        .into_iter()
        .flatten()
        .any(|v| v.to_lowercase().contains(&text))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host(name: &str) -> Host {
        Host {
            name: Some(name.to_owned()),
            ..Host::default()
        }
    }

    #[test]
    fn checks_addresses() {
        assert_eq!(
            Lookup::mac("aa-bb-cc-00-00-0f"),
            Ok(Lookup::Mac("AA:BB:CC:00:00:0F".to_owned()))
        );
        for invalid in [
            "",
            "AA:BB:CC:00:00",
            "AA:BB:CC:00:00:0G",
            "AAB:B:CC:00:00:01",
        ] {
            assert_eq!(
                Lookup::mac(invalid),
                Err(InvalidAddress::Mac(invalid.to_owned()))
            );
        }
        assert_eq!(
            Lookup::ip("192.0.2.1"),
            Ok(Lookup::Ip(Ipv4Addr::new(192, 0, 2, 1)))
        );
        let error = Lookup::ip("2001:db8::1").unwrap_err();
        assert_eq!(
            error.to_string(),
            "\"2001:db8::1\" is not an IPv4 address like 192.168.178.23"
        );
        assert_eq!(
            Lookup::mac("x").unwrap_err().to_string(),
            "\"x\" is not a MAC address like AA:BB:CC:DD:EE:FF"
        );
        assert_eq!(
            Lookup::Ip(Ipv4Addr::new(192, 0, 2, 1)).to_string(),
            "the IP address 192.0.2.1"
        );
        assert_eq!(
            Lookup::Mac("AA:BB:CC:00:00:01".to_owned()).to_string(),
            "the MAC address AA:BB:CC:00:00:01"
        );
    }

    #[test]
    fn finds_hosts_by_name() {
        let hosts = || {
            vec![
                host("device-1"),
                host("Device-11"),
                host("device-11"),
                Host::default(),
            ]
        };
        assert_eq!(named(hosts(), "DEVICE-1"), vec![host("device-1")]);
        assert_eq!(named(hosts(), "device-11").len(), 2);
        assert_eq!(named(hosts(), "ice-1").len(), 3);
        assert_eq!(named(hosts(), "printer"), Vec::<Host>::new());
    }

    #[test]
    fn searches_names_and_addresses() {
        let host = Host {
            name: Some("Laptop".to_owned()),
            ip: Some("192.0.2.10".to_owned()),
            mac: Some("AA:BB:CC:00:00:0A".to_owned()),
            ..Host::default()
        };
        assert!(contains(&host, "lap"));
        assert!(contains(&host, "2.10"));
        assert!(contains(&host, "00:0a"));
        assert!(!contains(&host, "phone"));
    }

    #[test]
    fn names_bands() {
        assert_eq!(band("AP:2G:0").as_deref(), Some("2.4 GHz"));
        assert_eq!(band("UPLINK:6G:0").as_deref(), Some("6 GHz"));
        assert_eq!(band("AP:5G:1").as_deref(), Some("5 GHz"));
        assert_eq!(band("LAN:1"), None);
    }
}
