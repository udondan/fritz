//! The list documents of a device, fetched at the paths actions like
//! `X_AVM-DE_GetHostListPath` return: the hosts as XML and the mesh as
//! JSON.

use std::collections::{HashMap, HashSet};
use std::str::FromStr;

use serde::Serialize;
use serde_json::Value;

use crate::generic::Error;
use crate::status::Host;

mod tree;

pub use tree::{Branch, Uplink};

/// The mesh: the devices and how they are connected.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Mesh {
    pub nodes: Vec<Node>,
    pub links: Vec<Link>,
}

/// A device in the mesh: a FRITZ! device or a host.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Node {
    /// The id of the node in the mesh list, which links refer to.
    #[serde(skip)]
    pub uid: String,
    pub name: Option<String>,
    pub model: Option<String>,
    pub mac: Option<String>,
    /// Like `master` for the device that leads the mesh, or `slave`.
    pub role: Option<String>,
    /// Whether the node is part of the mesh, rather than a host connected to
    /// it.
    pub meshed: bool,
    /// The IPv4 address of the node, from the host list, or for the device
    /// that leads the mesh from its LAN settings. `None` if unknown.
    pub ip: Option<String>,
    /// Whether the host the address is from is active. An inactive host
    /// keeps its last address, which the device may give to another host.
    /// `None` if the address is not from the host list.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active: Option<bool>,
    /// The MAC addresses of the node and of all its interfaces.
    #[serde(skip)]
    pub macs: Vec<String>,
}

/// A connection between two nodes.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Link {
    /// The name of a node, or its id if it has no name.
    pub from: String,
    pub to: String,
    /// The ids of the nodes `from` and `to`.
    #[serde(skip)]
    pub from_uid: String,
    #[serde(skip)]
    pub to_uid: String,
    /// Like `LAN` or `WLAN`.
    pub kind: Option<String>,
    /// The interface of `from`, like `AP:5G:0`.
    pub interface: Option<String>,
    /// Like `CONNECTED`.
    pub state: Option<String>,
    /// The current rates, in kbit/s.
    pub rate_rx: Option<u64>,
    pub rate_tx: Option<u64>,
    /// The highest rates, in kbit/s.
    pub max_rx: Option<u64>,
    pub max_tx: Option<u64>,
}

impl Mesh {
    /// Adds the IP addresses of the `hosts` to the nodes, joined by the MAC
    /// addresses of the nodes and their interfaces, in any case and with
    /// any separator. An active host is preferred over an inactive one. The
    /// device that leads the mesh is rarely a host of its own; without a
    /// host it gets the address `router`.
    pub fn add_addresses(&mut self, hosts: &[Host], router: Option<&str>) {
        for node in &mut self.nodes {
            let macs: HashSet<String> = node.macs.iter().map(|m| normal_mac(m)).collect();
            let host = hosts
                .iter()
                .filter(|h| h.ip.is_some())
                .filter(|h| {
                    h.mac
                        .as_deref()
                        .is_some_and(|m| macs.contains(&normal_mac(m)))
                })
                .reduce(|found, h| if !found.active && h.active { h } else { found });
            if let Some(host) = host {
                node.ip.clone_from(&host.ip);
                node.active = Some(host.active);
            } else if node.role.as_deref() == Some("master") {
                node.ip = router.map(str::to_owned);
            }
        }
    }
}

/// A MAC address in upper case, without separators.
pub(crate) fn normal_mac(mac: &str) -> String {
    mac.chars()
        .filter(char::is_ascii_hexdigit)
        .map(|c| c.to_ascii_uppercase())
        .collect()
}

/// The fields of an item of an XML list, by tag name, trimmed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Item(pub HashMap<String, String>);

impl Item {
    /// The text of a field, unless it is empty.
    pub fn text(&self, name: &str) -> Option<String> {
        self.0.get(name).filter(|v| !v.is_empty()).cloned()
    }

    /// A field as a number, if it is one.
    pub fn number<T: FromStr>(&self, name: &str) -> Option<T> {
        self.0.get(name).and_then(|v| v.parse().ok())
    }

    /// Whether a field is `1`, if it is there.
    pub fn flag(&self, name: &str) -> Option<bool> {
        self.0.get(name).map(|v| v == "1")
    }
}

/// Parses an XML list whose root is named `root` into its items: the
/// children named `item`, like `<List><Item><Name>…</Name></Item></List>`.
/// Other children of the root are skipped.
///
/// # Errors
///
/// [`Error::InvalidDocument`], naming the `document`, if it is not such a
/// list.
pub fn items(
    xml: &str,
    document: &'static str,
    root: &str,
    item: &str,
) -> Result<Vec<Item>, Error> {
    let invalid = |reason: String| Error::InvalidDocument { document, reason };
    let parsed = roxmltree::Document::parse(xml).map_err(|e| invalid(e.to_string()))?;
    let element = parsed.root_element();
    if element.tag_name().name() != root {
        return Err(invalid(format!(
            "the root is {}, not {root}",
            element.tag_name().name()
        )));
    }
    Ok(element
        .children()
        .filter(|n| n.has_tag_name(item))
        .map(|node| {
            Item(
                node.children()
                    .filter(roxmltree::Node::is_element)
                    .map(|f| {
                        let text = f.text().unwrap_or_default().trim();
                        (f.tag_name().name().to_owned(), text.to_owned())
                    })
                    .collect(),
            )
        })
        .collect())
}

/// Parses the host list: `<List><Item><HostName>…</HostName>…</Item></List>`.
///
/// # Errors
///
/// [`Error::InvalidDocument`] if it is not such a list.
pub fn hosts(xml: &str) -> Result<Vec<Host>, Error> {
    Ok(items(xml, "host list", "List", "Item")?
        .into_iter()
        .map(|item| Host {
            name: item.text("HostName"),
            ip: item.text("IPAddress"),
            mac: item.text("MACAddress"),
            active: item.flag("Active").unwrap_or_default(),
            interface: item.text("InterfaceType"),
            address_source: item.text("AddressSource"),
            guest: item.flag("X_AVM-DE_Guest"),
            internet_access: item.text("X_AVM-DE_WANAccess"),
            speed: item.number("X_AVM-DE_Speed"),
            permanent_address: None,
        })
        .collect())
}

/// Parses the mesh list: `{"nodes": [{"node_interfaces": [{"node_links":
/// […]}]}]}`. A link is listed at both of its ends, but returned once.
///
/// # Errors
///
/// [`Error::InvalidDocument`] if it is not such a list.
pub fn mesh(json: &str) -> Result<Mesh, Error> {
    let invalid = |reason: String| Error::InvalidDocument {
        document: "mesh list",
        reason,
    };
    let value: Value = serde_json::from_str(json).map_err(|e| invalid(e.to_string()))?;
    let nodes = value["nodes"]
        .as_array()
        .ok_or_else(|| invalid("it has no nodes".to_owned()))?;
    let text = |value: &Value| value.as_str().filter(|v| !v.is_empty()).map(str::to_owned);
    let interfaces = |node: &'_ Value| -> Vec<Value> {
        node["node_interfaces"]
            .as_array()
            .cloned()
            .unwrap_or_default()
    };
    let mut names = HashMap::new();
    let mut interface_names = HashMap::new();
    for node in nodes {
        let uid = node["uid"].as_str().unwrap_or_default();
        let name = text(&node["device_name"]).unwrap_or_else(|| uid.to_owned());
        names.insert(uid.to_owned(), name);
        for interface in interfaces(node) {
            if let (Some(uid), Some(name)) = (interface["uid"].as_str(), text(&interface["name"])) {
                interface_names.insert(uid.to_owned(), name);
            }
        }
    }
    let name = |uid: &Value| {
        let uid = uid.as_str().unwrap_or_default();
        names.get(uid).cloned().unwrap_or_else(|| uid.to_owned())
    };
    let mut seen = HashSet::new();
    let mut links = Vec::new();
    for node in nodes {
        for interface in interfaces(node) {
            for link in interface["node_links"].as_array().into_iter().flatten() {
                if !seen.insert(link["uid"].as_str().unwrap_or_default().to_owned()) {
                    continue;
                }
                links.push(Link {
                    from: name(&link["node_1_uid"]),
                    to: name(&link["node_2_uid"]),
                    from_uid: link["node_1_uid"].as_str().unwrap_or_default().to_owned(),
                    to_uid: link["node_2_uid"].as_str().unwrap_or_default().to_owned(),
                    kind: text(&link["type"]),
                    interface: link["node_interface_1_uid"]
                        .as_str()
                        .and_then(|uid| interface_names.get(uid).cloned()),
                    state: text(&link["state"]),
                    rate_rx: link["cur_data_rate_rx"].as_u64(),
                    rate_tx: link["cur_data_rate_tx"].as_u64(),
                    max_rx: link["max_data_rate_rx"].as_u64(),
                    max_tx: link["max_data_rate_tx"].as_u64(),
                });
            }
        }
    }
    Ok(Mesh {
        nodes: nodes
            .iter()
            .map(|node| Node {
                uid: node["uid"].as_str().unwrap_or_default().to_owned(),
                name: text(&node["device_name"]),
                model: text(&node["device_model"]),
                mac: text(&node["device_mac_address"]),
                role: text(&node["mesh_role"]),
                meshed: node["is_meshed"].as_bool().unwrap_or_default(),
                macs: std::iter::once(&node["device_mac_address"])
                    .chain(interfaces(node).iter().map(|i| &i["mac_address"]))
                    .filter_map(text)
                    .collect(),
                ..Node::default()
            })
            .collect(),
        links,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_host_list() {
        let hosts = hosts(
            "<?xml version=\"1.0\"?>\n<List><Item><Index>1</Index>\
             <HostName>device-1</HostName><IPAddress>192.0.2.1</IPAddress>\
             <MACAddress></MACAddress><Active>1</Active>\
             <X_AVM-DE_Guest>0</X_AVM-DE_Guest><X_AVM-DE_Speed>1000</X_AVM-DE_Speed>\
             </Item><Item><X_AVM-DE_Speed>fast</X_AVM-DE_Speed></Item></List>",
        )
        .unwrap();
        assert_eq!(hosts.len(), 2);
        assert_eq!(hosts[0].name.as_deref(), Some("device-1"));
        assert_eq!(hosts[0].ip.as_deref(), Some("192.0.2.1"));
        assert_eq!(hosts[0].mac, None);
        assert!(hosts[0].active);
        assert_eq!(hosts[0].guest, Some(false));
        assert_eq!(hosts[0].speed, Some(1000));
        assert_eq!(hosts[1], Host::default());
    }

    #[test]
    fn rejects_other_documents() {
        let error = hosts("<html/>").unwrap_err();
        assert_eq!(
            error.to_string(),
            "the host list of the device is not valid: the root is html, not List"
        );
        assert!(hosts("not xml").is_err());
        let error = mesh("{}").unwrap_err();
        assert_eq!(
            error.to_string(),
            "the mesh list of the device is not valid: it has no nodes"
        );
        assert!(mesh("not json").is_err());
    }

    #[test]
    fn adds_the_addresses_of_the_hosts() {
        let node = |uid: &str, role: Option<&str>, macs: &[&str]| Node {
            uid: uid.to_owned(),
            role: role.map(str::to_owned),
            macs: macs.iter().map(|m| (*m).to_owned()).collect(),
            ..Node::default()
        };
        let host = |ip: Option<&str>, mac: &str, active: bool| Host {
            ip: ip.map(str::to_owned),
            mac: Some(mac.to_owned()),
            active,
            ..Host::default()
        };
        let mut mesh = Mesh {
            nodes: vec![
                node("box", Some("master"), &["AA:BB:CC:00:00:01"]),
                // A repeater, in the host list with the MAC of its uplink.
                node(
                    "repeater",
                    Some("slave"),
                    &["AA:BB:CC:00:00:02", "AA:BB:CC:00:00:03"],
                ),
                node("inactive", None, &["AA:BB:CC:00:00:04"]),
                node("both", None, &["AA:BB:CC:00:00:05", "AA:BB:CC:00:00:06"]),
                node("unknown", None, &["AA:BB:CC:00:00:07"]),
                node("none", None, &[]),
            ],
            links: Vec::new(),
        };
        let hosts = [
            host(Some("192.0.2.3"), "aa-bb-cc-00-00-03", true),
            host(Some("192.0.2.4"), "AA:BB:CC:00:00:04", false),
            host(Some("192.0.2.5"), "AA:BB:CC:00:00:05", false),
            host(Some("192.0.2.6"), "aa:bb:cc:00:00:06", true),
            host(Some("192.0.2.9"), "AA:BB:CC:00:00:05", true),
            host(None, "AA:BB:CC:00:00:07", true),
            Host::default(),
        ];
        mesh.add_addresses(&hosts, Some("192.0.2.1"));
        let addresses: Vec<_> = mesh
            .nodes
            .iter()
            .map(|n| (n.ip.as_deref(), n.active))
            .collect();
        assert_eq!(
            addresses,
            [
                (Some("192.0.2.1"), None),
                (Some("192.0.2.3"), Some(true)),
                (Some("192.0.2.4"), Some(false)),
                (Some("192.0.2.6"), Some(true)),
                (None, None),
                (None, None),
            ]
        );
        let json = serde_json::to_value(&mesh.nodes[4]).unwrap();
        assert_eq!(json["ip"], Value::Null);
        assert!(json.get("active").is_none() && json.get("macs").is_none());
        // The device itself, as a host of its own.
        mesh.add_addresses(&[host(Some("192.0.2.8"), "AA:BB:CC:00:00:01", true)], None);
        assert_eq!(mesh.nodes[0].ip.as_deref(), Some("192.0.2.8"));
        let mut other = Mesh {
            nodes: vec![node("box", Some("master"), &[])],
            links: Vec::new(),
        };
        other.add_addresses(&[], None);
        assert_eq!(other.nodes[0].ip, None);
    }

    #[test]
    fn parses_the_mesh() {
        let mesh = mesh(
            r#"{"nodes": [
                {"uid": "n-1", "device_name": "fritz.box", "device_model": "FRITZ!Box 7690",
                 "device_mac_address": "AA:BB:CC:00:00:01",
                 "is_meshed": true, "mesh_role": "master",
                 "node_interfaces": [{"uid": "ni-1", "name": "AP:5G:0", "type": "WLAN",
                   "mac_address": "AA:BB:CC:00:00:11",
                   "node_links": [{"uid": "nl-1", "type": "WLAN", "state": "CONNECTED",
                     "node_1_uid": "n-1", "node_2_uid": "n-2",
                     "node_interface_1_uid": "ni-1", "node_interface_2_uid": "ni-2",
                     "cur_data_rate_rx": 100, "cur_data_rate_tx": 200,
                     "max_data_rate_rx": 300, "max_data_rate_tx": 400}]}]},
                {"uid": "n-2", "device_name": "", "is_meshed": false,
                 "node_interfaces": [{"uid": "ni-2", "name": "",
                   "node_links": [{"uid": "nl-1", "node_1_uid": "n-1", "node_2_uid": "n-2"},
                     {"uid": "nl-2", "node_1_uid": "n-2", "node_2_uid": "n-9"}]}]},
                {"uid": "n-3"}
            ]}"#,
        )
        .unwrap();
        assert_eq!(mesh.nodes.len(), 3);
        assert_eq!(mesh.nodes[0].name.as_deref(), Some("fritz.box"));
        assert_eq!(mesh.nodes[0].role.as_deref(), Some("master"));
        assert_eq!(
            mesh.nodes[0].macs,
            ["AA:BB:CC:00:00:01", "AA:BB:CC:00:00:11"]
        );
        assert!(mesh.nodes[0].meshed);
        assert_eq!(
            mesh.nodes[1],
            Node {
                uid: "n-2".to_owned(),
                ..Node::default()
            }
        );
        assert_eq!(
            mesh.links,
            [
                Link {
                    from: "fritz.box".to_owned(),
                    to: "n-2".to_owned(),
                    from_uid: "n-1".to_owned(),
                    to_uid: "n-2".to_owned(),
                    kind: Some("WLAN".to_owned()),
                    interface: Some("AP:5G:0".to_owned()),
                    state: Some("CONNECTED".to_owned()),
                    rate_rx: Some(100),
                    rate_tx: Some(200),
                    max_rx: Some(300),
                    max_tx: Some(400),
                },
                Link {
                    from: "n-2".to_owned(),
                    to: "n-9".to_owned(),
                    from_uid: "n-2".to_owned(),
                    to_uid: "n-9".to_owned(),
                    ..Link::default()
                }
            ]
        );
    }
}
