//! Helpers to read UPnP documents. Elements are matched by their local name,
//! so documents with and without namespace prefixes read the same.

use roxmltree::{Document, Node};

use crate::Error;

pub(crate) fn root<'a, 'i>(
    doc: &'a Document<'i>,
    expected: &'static str,
) -> Result<Node<'a, 'i>, Error> {
    let root = doc.root_element();
    if root.tag_name().name() == expected {
        Ok(root)
    } else {
        Err(Error::UnexpectedRoot {
            expected,
            found: root.tag_name().name().to_owned(),
        })
    }
}

pub(crate) fn child<'a, 'i>(node: Node<'a, 'i>, name: &str) -> Option<Node<'a, 'i>> {
    node.children()
        .find(|c| c.is_element() && c.tag_name().name() == name)
}

/// The trimmed text of the child element `name`, if it exists and isn't empty.
pub(crate) fn optional(node: Node<'_, '_>, name: &str) -> Option<String> {
    child(node, name)
        .and_then(|c| c.text())
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(str::to_owned)
}

pub(crate) fn required(
    node: Node<'_, '_>,
    parent: &'static str,
    element: &'static str,
) -> Result<String, Error> {
    optional(node, element).ok_or(Error::MissingElement { parent, element })
}

/// Parses every `item` element inside the child element `list`. A missing list
/// is an empty list.
pub(crate) fn list<'a, 'i, T>(
    node: Node<'a, 'i>,
    list: &str,
    item: &str,
    parse: impl Fn(Node<'a, 'i>) -> Result<T, Error>,
) -> Result<Vec<T>, Error> {
    child(node, list).map_or_else(
        || Ok(Vec::new()),
        |list| {
            list.children()
                .filter(|c| c.is_element() && c.tag_name().name() == item)
                .map(parse)
                .collect()
        },
    )
}
