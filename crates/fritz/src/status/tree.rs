//! `fritz mesh --tree`: the mesh drawn like the command `tree`.

use std::io::Write;

use fritz_core::lists::Branch;

use super::{address, rates, table};

/// The band of a wireless interface like `AP:5G:0`.
fn band(interface: &str) -> Option<&'static str> {
    interface.split(':').find_map(|part| match part {
        "2G" => Some("2.4 GHz"),
        "5G" => Some("5 GHz"),
        "6G" => Some("6 GHz"),
        _ => None,
    })
}

/// Draws the tree like the command `tree`, as a table: a row for every
/// device, with its address, its role and model if it is a mesh node, and
/// how it is connected to the device above it.
pub(super) fn mesh_tree(roots: &[Branch], out: &mut impl Write) -> anyhow::Result<()> {
    let mut rows = Vec::new();
    for root in roots {
        rows.push(tree_row(root, ""));
        tree_children(&root.children, "", &mut rows);
    }
    table(
        &["NODE", "IP", "ROLE", "MODEL", "CONNECTION"],
        rows.into_iter(),
        out,
    )
}

fn tree_children(branches: &[Branch], indent: &str, rows: &mut Vec<Vec<String>>) {
    for (i, branch) in branches.iter().enumerate() {
        let last = i + 1 == branches.len();
        let (fork, more) = if last {
            ("└── ", "    ")
        } else {
            ("├── ", "│   ")
        };
        rows.push(tree_row(branch, &format!("{indent}{fork}")));
        tree_children(&branch.children, &format!("{indent}{more}"), rows);
    }
}

/// The cells of one device: the name after the lines of the tree, the
/// address, the role and model of a mesh node, and the links to the device
/// above, like `WLAN 5 GHz ↓ 866.7 Mbit/s  ↑ 780.0 Mbit/s`.
fn tree_row(branch: &Branch, prefix: &str) -> Vec<String> {
    let node = branch.node;
    let name = node.name.as_deref().unwrap_or("?");
    let about = |value: &Option<String>| {
        value
            .as_deref()
            .filter(|_| node.meshed)
            .unwrap_or_default()
            .to_owned()
    };
    let links: Vec<String> = branch
        .uplinks
        .iter()
        .map(|u| {
            let band = u.interface.as_deref().and_then(band);
            // A LAN link reports no rates, as 0.
            let rates = rates(u.rate_down, u.rate_up)
                .filter(|_| u.rate_down.unwrap_or_default() + u.rate_up.unwrap_or_default() > 0);
            [u.kind.clone(), band.map(str::to_owned), rates]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect();
    vec![
        format!("{prefix}{name}"),
        address(node),
        about(&node.role),
        about(&node.model),
        links.join(", "),
    ]
}

#[cfg(test)]
mod tests {
    use fritz_core::lists::{Node, Uplink};

    use super::*;

    #[test]
    fn names_the_bands_of_the_mesh_links() {
        assert_eq!(band("AP:2G:0"), Some("2.4 GHz"));
        assert_eq!(band("UPLINK:5G:0"), Some("5 GHz"));
        assert_eq!(band("AP:6G:1"), Some("6 GHz"));
        assert_eq!(band("LAN:1"), None);
        let node = Node::default();
        let branch = Branch {
            node: &node,
            uplinks: vec![Uplink {
                kind: Some("WLAN".to_owned()),
                interface: Some("AP:2G:0".to_owned()),
                rate_down: Some(0),
                rate_up: None,
            }],
            children: Vec::new(),
        };
        assert_eq!(tree_row(&branch, ""), ["?", "-", "", "", "WLAN 2.4 GHz"]);
    }
}
