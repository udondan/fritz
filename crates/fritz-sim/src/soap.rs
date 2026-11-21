//! Reading SOAP requests and writing SOAP responses.

use std::fmt::Write as _;

use roxmltree::{Document, Node};

use crate::device::Fault;

/// An action call from a request body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Call {
    /// The namespace of the action element, which is the service type.
    pub(crate) service_type: String,
    pub(crate) action: String,
    pub(crate) arguments: Vec<(String, String)>,
}

/// The action call in a request body, if it is one.
pub(crate) fn parse(body: &str) -> Option<Call> {
    let doc = Document::parse(body).ok()?;
    let body = doc
        .descendants()
        .find(|n| n.is_element() && n.tag_name().name() == "Body")?;
    let call = body.children().find(Node::is_element)?;
    Some(Call {
        service_type: call.tag_name().namespace().unwrap_or_default().to_owned(),
        action: call.tag_name().name().to_owned(),
        arguments: call
            .children()
            .filter(Node::is_element)
            .map(|a| {
                let value = a.text().unwrap_or_default();
                (a.tag_name().name().to_owned(), value.to_owned())
            })
            .collect(),
    })
}

/// The response to a successful call.
pub(crate) fn response(service_type: &str, action: &str, out: &[(String, String)]) -> String {
    let mut arguments = String::new();
    for (name, value) in out {
        let _ = write!(arguments, "<{name}>{}</{name}>", escape(value));
    }
    envelope(&format!(
        r#"<u:{action}Response xmlns:u="{}">{arguments}</u:{action}Response>"#,
        escape(service_type)
    ))
}

/// The response to a failed call.
pub(crate) fn fault(fault: &Fault) -> String {
    envelope(&format!(
        concat!(
            "<s:Fault><faultcode>s:Client</faultcode><faultstring>UPnPError</faultstring>",
            r#"<detail><UPnPError xmlns="urn:schemas-upnp-org:control-1-0">"#,
            "<errorCode>{}</errorCode><errorDescription>{}</errorDescription>",
            "</UPnPError></detail></s:Fault>",
        ),
        fault.code,
        escape(&fault.description)
    ))
}

fn envelope(body: &str) -> String {
    format!(
        concat!(
            r#"<?xml version="1.0"?>"#,
            r#"<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/" "#,
            r#"s:encodingStyle="http://schemas.xmlsoap.org/soap/encoding/">"#,
            "<s:Body>{}</s:Body></s:Envelope>",
        ),
        body
    )
}

fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_calls() {
        let body = r#"<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/"><s:Body>
            <u:SetEnable xmlns:u="urn:x"><NewEnable>1</NewEnable><NewName/></u:SetEnable>
            </s:Body></s:Envelope>"#;
        assert_eq!(
            parse(body),
            Some(Call {
                service_type: "urn:x".to_owned(),
                action: "SetEnable".to_owned(),
                arguments: vec![
                    ("NewEnable".to_owned(), "1".to_owned()),
                    ("NewName".to_owned(), String::new())
                ],
            })
        );
        assert_eq!(
            parse("<Envelope><Body><Call/></Body></Envelope>")
                .unwrap()
                .service_type,
            ""
        );
        assert_eq!(parse("not xml"), None);
        assert_eq!(parse("<Envelope/>"), None);
        assert_eq!(parse("<Envelope><Body>text</Body></Envelope>"), None);
    }

    #[test]
    fn escapes_values() {
        let xml = response(
            "urn:x",
            "Get",
            &[("New".to_owned(), "<a & \"b\">".to_owned())],
        );
        assert!(
            xml.contains("<New>&lt;a &amp; &quot;b&quot;&gt;</New>"),
            "{xml}"
        );
    }
}
