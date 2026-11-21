//! SOAP envelopes of TR-064 action calls and their responses.

use std::fmt::Write as _;

use roxmltree::Document;

use crate::{Error, xml};

/// The output arguments of an action call, in the order the device sent them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Arguments(Vec<(String, String)>);

impl Arguments {
    /// The value of the argument with the given name.
    pub fn get(&self, name: &str) -> Option<&str> {
        self.0
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }

    /// All arguments as name and value.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.0.iter().map(|(n, v)| (n.as_str(), v.as_str()))
    }

    /// The number of arguments.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether the action returned no arguments.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl IntoIterator for Arguments {
    type Item = (String, String);
    type IntoIter = std::vec::IntoIter<(String, String)>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

/// The request body of an action call.
pub(crate) fn envelope(
    service_type: &str,
    action: &str,
    arguments: &[(&str, &str)],
) -> Result<String, Error> {
    check_name("action", action)?;
    let mut body = String::new();
    for (name, value) in arguments {
        check_name("argument", name)?;
        let _ = write!(body, "<{name}>{}</{name}>", escape(value));
    }
    Ok(format!(
        concat!(
            r#"<?xml version="1.0" encoding="utf-8"?>"#,
            r#"<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/" "#,
            r#"s:encodingStyle="http://schemas.xmlsoap.org/soap/encoding/">"#,
            r#"<s:Body><u:{action} xmlns:u="{service_type}">{body}</u:{action}></s:Body>"#,
            "</s:Envelope>",
        ),
        action = action,
        service_type = escape(service_type),
        body = body,
    ))
}

/// The output arguments from the response to `action`.
pub(crate) fn parse_response(xml: &str, action: &str) -> Result<Arguments, Error> {
    let doc = Document::parse(xml)?;
    let body = doc
        .descendants()
        .find(|n| n.is_element() && n.tag_name().name() == "Body")
        .ok_or_else(|| Error::InvalidResponse("no SOAP Body".to_owned()))?;
    let expected = format!("{action}Response");
    let response = xml::child(body, &expected)
        .ok_or_else(|| Error::InvalidResponse(format!("no <{expected}> in the SOAP Body")))?;
    Ok(Arguments(
        response
            .children()
            .filter(roxmltree::Node::is_element)
            .map(|c| {
                (
                    c.tag_name().name().to_owned(),
                    c.text().unwrap_or_default().to_owned(),
                )
            })
            .collect(),
    ))
}

/// The UPnP error in a SOAP fault, if the document is one.
pub(crate) fn parse_fault(xml: &str) -> Option<Error> {
    let doc = Document::parse(xml).ok()?;
    let error = doc
        .descendants()
        .find(|n| n.is_element() && n.tag_name().name() == "UPnPError")?;
    let code = xml::optional(error, "errorCode")?.parse().ok()?;
    Some(Error::Upnp {
        code,
        description: xml::optional(error, "errorDescription").unwrap_or_default(),
    })
}

/// Action and argument names become element names, so they must be plain
/// XML names.
fn check_name(element: &'static str, name: &str) -> Result<(), Error> {
    let mut chars = name.chars();
    let valid = chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'));
    if valid {
        Ok(())
    } else {
        Err(Error::InvalidName {
            element,
            name: name.to_owned(),
        })
    }
}

fn escape(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&apos;"),
            _ => escaped.push(c),
        }
    }
    escaped
}
