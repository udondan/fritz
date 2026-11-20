/// Errors of this crate.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// A document is not well-formed XML.
    #[error("invalid XML: {0}")]
    Xml(#[from] roxmltree::Error),

    /// A document has a different root element than expected, for example an
    /// SCPD where a device description was expected.
    #[error("unexpected root element <{found}>, expected <{expected}>")]
    UnexpectedRoot {
        expected: &'static str,
        found: String,
    },

    /// A required element is missing or empty.
    #[error("<{parent}> has no <{element}>")]
    MissingElement {
        parent: &'static str,
        element: &'static str,
    },

    /// An element has a value outside the allowed set.
    #[error("invalid value {value:?} in <{element}>")]
    InvalidValue {
        element: &'static str,
        value: String,
    },

    /// No service of the device matches the given reference.
    #[error("no service matches {0:?}")]
    UnknownService(String),

    /// More than one service of the device matches the given reference.
    #[error("{reference:?} matches several services: {}", candidates.join(", "))]
    AmbiguousService {
        reference: String,
        candidates: Vec<String>,
    },

    /// An action or argument name is not a valid XML element name.
    #[error("invalid {element} name {name:?}")]
    InvalidName { element: &'static str, name: String },

    /// The request failed before a response arrived, for example because the
    /// device was unreachable or did not answer in time.
    #[error("HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),

    /// The device answered with an unexpected HTTP status.
    #[error("HTTP status {status} from {url}")]
    Status { status: u16, url: String },

    /// The device requires authentication, but the client has no credentials.
    #[error("the device requires credentials")]
    CredentialsRequired,

    /// The device rejected the credentials. The client does not retry, so
    /// that a wrong password does not lock the account.
    #[error("the device rejected the credentials")]
    AuthenticationFailed,

    /// The device asked for an authentication scheme this crate does not
    /// support.
    #[error("unsupported authentication challenge: {0}")]
    UnsupportedAuth(String),

    /// The device answered an action call with a UPnP error.
    #[error("UPnP error {code}: {description}")]
    Upnp { code: u16, description: String },

    /// A response is well-formed XML but not what the call expected.
    #[error("invalid response: {0}")]
    InvalidResponse(String),

    /// A URL cannot be used, for example because it points to another host
    /// than the device.
    #[error("invalid URL: {0}")]
    InvalidUrl(String),
}
