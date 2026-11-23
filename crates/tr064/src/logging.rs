//! Diagnostics through [`tracing`], and the hook that decides which values
//! may appear in them.
//!
//! The client logs one event per request at `info`: method, URL, action,
//! status and duration. At `debug` it adds the steps of the Digest
//! authentication and the check of the certificate, at `trace` the
//! arguments of action calls and the size of other bodies. Headers,
//! passwords, nonces and Digest answers are never logged. Values of query
//! parameters and arguments pass a [`Redact`] hook first.

use std::fmt::{self, Write as _};

use reqwest::Url;

use crate::{Error, Service};

/// What a hidden value is replaced with.
pub const HIDDEN: &str = "***";

/// Decides how the values of requests and responses appear in the log.
///
/// The default, [`HideValues`], hides every value. Users that know which
/// names hold secrets on their devices can show the others.
pub trait Redact: fmt::Debug + Send + Sync {
    /// An input or output argument of an action, as it may be logged.
    fn argument(&self, service: &Service, action: &str, name: &str, value: &str) -> String;

    /// A parameter of the query of a URL, as it may be logged.
    fn parameter(&self, name: &str, value: &str) -> String;
}

/// Hides every value: the default [`Redact`] hook.
#[derive(Debug, Clone, Copy, Default)]
pub struct HideValues;

impl Redact for HideValues {
    fn argument(&self, _: &Service, _: &str, _: &str, _: &str) -> String {
        HIDDEN.to_owned()
    }

    fn parameter(&self, _: &str, _: &str) -> String {
        HIDDEN.to_owned()
    }
}

/// A URL as it may be logged: without user information and fragment, and
/// with the values of the query passed through `redact`.
pub(crate) fn url(url: &Url, redact: &dyn Redact) -> String {
    let mut text = format!("{}://{}", url.scheme(), url.host_str().unwrap_or_default());
    if let Some(port) = url.port() {
        let _ = write!(text, ":{port}");
    }
    text.push_str(url.path());
    if url.query().is_some() {
        let pairs: Vec<String> = url
            .query_pairs()
            .map(|(name, value)| format!("{name}={}", redact.parameter(&name, &value)))
            .collect();
        let _ = write!(text, "?{}", pairs.join("&"));
    }
    text
}

/// The arguments of an action as they may be logged, like
/// `NewIndex="0", NewName="***"`.
pub(crate) fn arguments<'a>(
    redact: &dyn Redact,
    service: &Service,
    action: &str,
    arguments: impl IntoIterator<Item = (&'a str, &'a str)>,
) -> String {
    arguments
        .into_iter()
        .map(|(name, value)| {
            let value = redact.argument(service, action, name, value);
            format!("{name}={value:?}")
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// An error as it may be logged: without URLs, which can hold secrets in
/// their query, and without the challenge of the device.
pub(crate) fn error(error: &Error) -> String {
    match error {
        Error::Http(error) => {
            // The causes, without the URL of the request.
            let causes = std::iter::successors(std::error::Error::source(error), |e| e.source());
            causes.fold("HTTP request failed".to_owned(), |text, cause| {
                format!("{text}: {cause}")
            })
        }
        Error::Status { status, .. } => format!("HTTP status {status}"),
        Error::UnsupportedAuth(_) => "unsupported authentication challenge".to_owned(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn service() -> Service {
        Service {
            service_type: "urn:example:service:Thing:1".to_owned(),
            service_id: "urn:example:serviceId:Thing1".to_owned(),
            control_url: "/control".to_owned(),
            event_sub_url: None,
            scpd_url: "/scpd.xml".to_owned(),
        }
    }

    #[test]
    fn hides_values_by_default() {
        let url = Url::parse("https://user:pw@device:8443/list?id=1&token=abc#top").unwrap();
        assert_eq!(
            super::url(&url, &HideValues),
            "https://device:8443/list?id=***&token=***"
        );
        let url = Url::parse("http://device/desc.xml").unwrap();
        assert_eq!(super::url(&url, &HideValues), "http://device/desc.xml");
        assert_eq!(
            arguments(&HideValues, &service(), "Get", [("A", "1"), ("B", "x")]),
            r#"A="***", B="***""#
        );
    }

    #[test]
    fn describes_errors_without_secrets() {
        let status = Error::Status {
            status: 404,
            url: "http://device/list?token=abc".to_owned(),
        };
        assert_eq!(error(&status), "HTTP status 404");
        let auth = Error::UnsupportedAuth("Digest nonce=\"abc\"".to_owned());
        assert_eq!(error(&auth), "unsupported authentication challenge");
        assert_eq!(
            error(&Error::AuthenticationFailed),
            "the device rejected the credentials"
        );
    }
}
