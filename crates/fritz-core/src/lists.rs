//! The list documents of a device, fetched at the paths actions like
//! `X_AVM-DE_GetHostListPath` return: the hosts as XML and the mesh as
//! JSON.

use std::collections::{HashMap, HashSet};

use serde::Serialize;
use serde_json::Value;

use crate::generic::Error;
use crate::status::Host;

/// The mesh: the devices and how they are connected.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Mesh {
    pub nodes: Vec<Node>,
    pub links: Vec<Link>,
}

/// A device in the mesh: a FRITZ! device or a host.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Node {
    pub name: Option<String>,
    pub model: Option<String>,
    pub mac: Option<String>,
    /// Like `master` for the device that leads the mesh, or `slave`.
    pub role: Option<String>,
    /// Whether the node is part of the mesh, rather than a host connected to
    /// it.
    pub meshed: bool,
}

/// A connection between two nodes.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Link {
    /// The name of a node, or its id if it has no name.
    pub from: String,
    pub to: String,
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

/// Parses the host list: `<List><Item><HostName>…</HostName>…</Item></List>`.
///
/// # Errors
///
/// [`Error::InvalidDocument`] if it is not such a list.
pub fn hosts(xml: &str) -> Result<Vec<Host>, Error> {
    let invalid = |reason: String| Error::InvalidDocument {
        document: "host list",
        reason,
    };
    let document = roxmltree::Document::parse(xml).map_err(|e| invalid(e.to_string()))?;
    let root = document.root_element();
    if root.tag_name().name() != "List" {
        return Err(invalid(format!(
            "the root is {}, not List",
            root.tag_name().name()
        )));
    }
    Ok(root
        .children()
        .filter(|n| n.has_tag_name("Item"))
        .map(|item| {
            let fields: HashMap<&str, &str> = item
                .children()
                .filter(roxmltree::Node::is_element)
                .map(|f| (f.tag_name().name(), f.text().unwrap_or_default().trim()))
                .collect();
            let text = |name: &str| {
                fields
                    .get(name)
                    .filter(|v| !v.is_empty())
                    .map(|v| (*v).to_owned())
            };
            let flag = |name: &str| fields.get(name).map(|v| *v == "1");
            Host {
                name: text("HostName"),
                ip: text("IPAddress"),
                mac: text("MACAddress"),
                active: flag("Active").unwrap_or_default(),
                interface: text("InterfaceType"),
                address_source: text("AddressSource"),
                guest: flag("X_AVM-DE_Guest"),
                internet_access: text("X_AVM-DE_WANAccess"),
                speed: fields.get("X_AVM-DE_Speed").and_then(|v| v.parse().ok()),
            }
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
                name: text(&node["device_name"]),
                model: text(&node["device_model"]),
                mac: text(&node["device_mac_address"]),
                role: text(&node["mesh_role"]),
                meshed: node["is_meshed"].as_bool().unwrap_or_default(),
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
    fn parses_the_mesh() {
        let mesh = mesh(
            r#"{"nodes": [
                {"uid": "n-1", "device_name": "fritz.box", "device_model": "FRITZ!Box 7690",
                 "is_meshed": true, "mesh_role": "master",
                 "node_interfaces": [{"uid": "ni-1", "name": "AP:5G:0", "type": "WLAN",
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
        assert!(mesh.nodes[0].meshed);
        assert_eq!(mesh.nodes[1], Node::default());
        assert_eq!(
            mesh.links,
            [
                Link {
                    from: "fritz.box".to_owned(),
                    to: "n-2".to_owned(),
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
                    ..Link::default()
                }
            ]
        );
    }
}
