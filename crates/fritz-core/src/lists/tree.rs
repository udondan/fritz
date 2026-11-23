//! The mesh as a tree: which device is connected to which router or
//! repeater.

use std::collections::{HashMap, HashSet, VecDeque};

use serde::Serialize;

use super::{Link, Mesh, Node};

/// A node of the mesh as a tree, with the devices connected through it
/// below it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Branch<'a> {
    #[serde(flatten)]
    pub node: &'a Node,
    /// The links to the node above. A repeater often has two, one for each
    /// band.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub uplinks: Vec<Uplink>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<Branch<'a>>,
}

/// A link of a node to the node above it in the tree.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Uplink {
    /// Like `LAN` or `WLAN`.
    pub kind: Option<String>,
    /// The interface the mesh list names for the link, like `AP:5G:0` of the
    /// node above or `UPLINK:5G:0` of the node.
    pub interface: Option<String>,
    /// The current rates towards the node and back, in kbit/s.
    pub rate_down: Option<u64>,
    pub rate_up: Option<u64>,
}

/// The connected links of every node to the nodes below it.
type Below<'a> = HashMap<&'a str, Vec<(&'a str, Uplink)>>;

/// The nodes placed below every node, with their links to it.
type Children<'a> = HashMap<&'a str, Vec<(&'a str, Vec<Uplink>)>>;

impl Mesh {
    /// The mesh as a tree: the node that leads the mesh, the nodes connected
    /// to it below, and so on. A node linked to several others hangs below
    /// the one closest to the top. Links that are not connected are left
    /// out, and so are hosts that only have such links. A mesh node that
    /// cannot be reached starts a tree of its own.
    #[must_use]
    pub fn tree(&self) -> Vec<Branch<'_>> {
        let nodes: HashMap<&str, &Node> = self.nodes.iter().map(|n| (n.uid.as_str(), n)).collect();
        let below = self.below();
        let starts = self
            .nodes
            .iter()
            .filter(|n| n.role.as_deref() == Some("master"))
            .chain(self.nodes.iter().filter(|n| n.meshed));
        let mut placed = HashSet::new();
        let mut children = Children::new();
        let mut roots = Vec::new();
        for start in starts {
            let uid = start.uid.as_str();
            if placed.insert(uid) {
                roots.push(uid);
                place(&below, &nodes, uid, &mut placed, &mut children);
            }
        }
        roots
            .into_iter()
            .map(|uid| branch(&nodes, &mut children, uid, Vec::new()))
            .collect()
    }

    fn below(&self) -> Below<'_> {
        let mut below = Below::new();
        let connected = self
            .links
            .iter()
            .filter(|l| l.state.as_deref().is_none_or(|s| s == "CONNECTED"));
        for link in connected {
            let (parent, child, uplink) = orient(link);
            below.entry(parent).or_default().push((child, uplink));
        }
        below
    }
}

/// The node above, the node below, and the link between them. A repeater
/// lists its uplink with its own interface first.
fn orient(link: &Link) -> (&str, &str, Uplink) {
    let reversed = link
        .interface
        .as_deref()
        .is_some_and(|i| i.starts_with("UPLINK"));
    let (parent, child, down, up) = if reversed {
        (&link.to_uid, &link.from_uid, link.rate_rx, link.rate_tx)
    } else {
        (&link.from_uid, &link.to_uid, link.rate_tx, link.rate_rx)
    };
    let uplink = Uplink {
        kind: link.kind.clone(),
        interface: link.interface.clone(),
        rate_down: down,
        rate_up: up,
    };
    (parent, child, uplink)
}

/// Places the nodes that are not placed yet below `start`, breadth first,
/// so that every node hangs below the node closest to the top.
fn place<'a>(
    below: &Below<'a>,
    nodes: &HashMap<&str, &Node>,
    start: &'a str,
    placed: &mut HashSet<&'a str>,
    children: &mut Children<'a>,
) {
    let mut queue = VecDeque::from([start]);
    while let Some(parent) = queue.pop_front() {
        let links = below.get(parent).map_or(&[][..], Vec::as_slice);
        let found = group(links, |child| {
            nodes.contains_key(child) && !placed.contains(child)
        });
        for (child, _) in &found {
            placed.insert(child);
            queue.push_back(child);
        }
        children.insert(parent, found);
    }
}

/// The `links` grouped by the node below, for the nodes that are `new`.
fn group<'a>(
    links: &[(&'a str, Uplink)],
    new: impl Fn(&str) -> bool,
) -> Vec<(&'a str, Vec<Uplink>)> {
    let mut found: Vec<(&str, Vec<Uplink>)> = Vec::new();
    for (child, uplink) in links {
        if let Some(entry) = found.iter_mut().find(|(c, _)| c == child) {
            entry.1.push(uplink.clone());
        } else if new(child) {
            found.push((child, vec![uplink.clone()]));
        }
    }
    found
}

/// The branch of the node `uid`, with the `children` found for it.
fn branch<'a>(
    nodes: &HashMap<&str, &'a Node>,
    children: &mut Children<'_>,
    uid: &str,
    uplinks: Vec<Uplink>,
) -> Branch<'a> {
    let mut below: Vec<Branch<'a>> = children
        .remove(uid)
        .unwrap_or_default()
        .into_iter()
        .map(|(child, uplinks)| branch(nodes, children, child, uplinks))
        .collect();
    below.sort_by_key(|b| {
        (
            !b.node.meshed,
            b.node.name.clone().map(|n| n.to_lowercase()),
        )
    });
    Branch {
        node: nodes[uid],
        uplinks,
        children: below,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(uid: &str, role: Option<&str>) -> Node {
        Node {
            uid: uid.to_owned(),
            name: Some(uid.to_uppercase()),
            role: role.map(str::to_owned),
            meshed: role.is_some(),
            ..Node::default()
        }
    }

    fn link(from: &str, to: &str, interface: &str, state: &str) -> Link {
        Link {
            from_uid: from.to_owned(),
            to_uid: to.to_owned(),
            interface: Some(interface.to_owned()),
            state: Some(state.to_owned()),
            rate_rx: Some(1),
            rate_tx: Some(2),
            ..Link::default()
        }
    }

    /// The uids of a branch and those below it, like `box(r1(h1))`.
    fn shape(branch: &Branch) -> String {
        let below: Vec<String> = branch.children.iter().map(shape).collect();
        if below.is_empty() {
            branch.node.uid.clone()
        } else {
            format!("{}({})", branch.node.uid, below.join(" "))
        }
    }

    #[test]
    fn builds_the_tree() {
        let mesh = Mesh {
            nodes: vec![
                node("h2", None),
                node("r1", Some("slave")),
                node("box", Some("master")),
                node("r2", Some("slave")),
                node("h1", None),
                node("gone", None),
                node("lost", Some("slave")),
            ],
            links: vec![
                // The repeaters list their uplinks from their own side, r1
                // in two bands, and r2 is connected through r1.
                link("r1", "box", "UPLINK:2G:0", "CONNECTED"),
                link("box", "r1", "AP:5G:0", "CONNECTED"),
                link("r1", "box", "UPLINK:5G:0", "CONNECTED"),
                link("r2", "r1", "UPLINK:5G:0", "CONNECTED"),
                link("r2", "box", "UPLINK:2G:0", "DISCONNECTED"),
                link("r2", "h2", "LAN:1", "CONNECTED"),
                link("box", "h1", "AP:2G:0", "CONNECTED"),
                link("box", "gone", "AP:2G:0", "DISCONNECTED"),
                link("box", "nowhere", "AP:2G:0", "CONNECTED"),
            ],
        };
        let tree = mesh.tree();
        let shapes: Vec<String> = tree.iter().map(shape).collect();
        assert_eq!(shapes, ["box(r1(r2(h2)) h1)", "lost"]);
        let r1 = &tree[0].children[0];
        let interfaces: Vec<_> = r1.uplinks.iter().map(|u| u.interface.as_deref()).collect();
        assert_eq!(
            interfaces,
            [Some("UPLINK:2G:0"), Some("AP:5G:0"), Some("UPLINK:5G:0")]
        );
        // The rates go down towards the repeater, whichever side lists the
        // link.
        assert_eq!(
            (r1.uplinks[0].rate_down, r1.uplinks[0].rate_up),
            (Some(1), Some(2))
        );
        assert_eq!(
            (r1.uplinks[1].rate_down, r1.uplinks[1].rate_up),
            (Some(2), Some(1))
        );
        let json = serde_json::to_value(&tree[1]).unwrap();
        assert_eq!(json["name"], "LOST");
        assert!(json.get("children").is_none() && json.get("uid").is_none());
    }
}
