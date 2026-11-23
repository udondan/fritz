//! `fritz mesh --tree`: the mesh drawn like the command `tree`.

use std::io::Write;

use fritz_core::lists::Branch;

use super::rates;

/// Draws the tree like the command `tree`: a line for every device, with
/// its role and model if it is a mesh node, then how it is connected to the
/// device above it.
/// The band of a wireless interface like `AP:5G:0`.
fn band(interface: &str) -> Option<&'static str> {
    interface.split(':').find_map(|part| match part {
        "2G" => Some("2.4 GHz"),
        "5G" => Some("5 GHz"),
        "6G" => Some("6 GHz"),
        _ => None,
    })
}

pub(super) fn mesh_tree(roots: &[Branch], out: &mut impl Write) -> anyhow::Result<()> {
    for root in roots {
        writeln!(out, "{}", tree_line(root))?;
        tree_children(&root.children, "", out)?;
    }
    Ok(())
}

fn tree_children(branches: &[Branch], indent: &str, out: &mut impl Write) -> anyhow::Result<()> {
    for (i, branch) in branches.iter().enumerate() {
        let last = i + 1 == branches.len();
        let (fork, more) = if last {
            ("└── ", "    ")
        } else {
            ("├── ", "│   ")
        };
        writeln!(out, "{indent}{fork}{}", tree_line(branch))?;
        tree_children(&branch.children, &format!("{indent}{more}"), out)?;
    }
    Ok(())
}

/// Like `repeater (slave, FRITZ!Repeater 2400)  WLAN 5 GHz ↓ 866.7 Mbit/s  ↑ 780.0 Mbit/s`.
fn tree_line(branch: &Branch) -> String {
    let node = branch.node;
    let mut line = node.name.clone().unwrap_or_else(|| "?".to_owned());
    let about: Vec<&str> = [&node.role, &node.model]
        .into_iter()
        .filter(|_| node.meshed)
        .filter_map(Option::as_deref)
        .collect();
    if !about.is_empty() {
        line = format!("{line} ({})", about.join(", "));
    }
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
    if !links.is_empty() {
        line.push_str("  ");
        line.push_str(&links.join(", "));
    }
    line
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
        assert_eq!(tree_line(&branch), "?  WLAN 2.4 GHz");
    }
}
