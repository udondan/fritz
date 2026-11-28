use crate::Fingerprint;

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
        /// The expected root element.
        expected: &'static str,
        /// The root element of the document.
        found: String,
    },

    /// A required element is missing or empty.
    #[error("<{parent}> has no <{element}>")]
    MissingElement {
        /// The element that lacks the child.
        parent: &'static str,
        /// The missing child.
        element: &'static str,
    },

    /// An element has a value outside the allowed set.
    #[error("invalid value {value:?} in <{element}>")]
    InvalidValue {
        /// The element with the value.
        element: &'static str,
        /// The value.
        value: String,
    },

    /// No service of the device matches the given reference.
    #[error("no service matches {0:?}")]
    UnknownService(String),

    /// More than one service of the device matches the given reference.
    #[error("{reference:?} matches several services: {}", candidates.join(", "))]
    AmbiguousService {
        /// The reference.
        reference: String,
        /// The service ids of the matching services.
        candidates: Vec<String>,
    },

    /// An action or argument name is not a valid XML element name.
    #[error("invalid {element} name {name:?}")]
    InvalidName {
        /// `action` or `argument`.
        element: &'static str,
        /// The name.
        name: String,
    },

    /// The request failed before a response arrived, for example because the
    /// device was unreachable or did not answer in time.
    #[error("HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),

    /// The device answered with an unexpected HTTP status.
    #[error("HTTP status {status} from {url}")]
    Status {
        /// The HTTP status code.
        status: u16,
        /// The requested URL.
        url: String,
    },

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
    Upnp {
        /// The UPnP error code, for example 402 (invalid arguments). Vendors
        /// define their own codes from 800 up.
        code: u16,
        /// The description the device sent with the code, possibly empty.
        description: String,
    },

    /// A response is well-formed XML but not what the call expected.
    #[error("invalid response: {0}")]
    InvalidResponse(String),

    /// A URL cannot be used, for example because it points to another host
    /// than the device.
    #[error("invalid URL: {0}")]
    InvalidUrl(String),

    /// A certificate fingerprint is not 64 hex digits.
    #[error("invalid certificate fingerprint {0:?}")]
    InvalidFingerprint(String),

    /// The device presented another certificate than the pinned one. Either
    /// the device got a new certificate, or someone else is answering.
    #[error("the device presented the certificate {found}, expected {expected}")]
    CertificateMismatch {
        /// The pinned fingerprint.
        expected: Fingerprint,
        /// The fingerprint of the presented certificate.
        found: Fingerprint,
    },
}

impl Error {
    /// The UPnP error code if the device answered an action call with one,
    /// see [`Error::Upnp`].
    pub fn upnp_code(&self) -> Option<u16> {
        match self {
            Self::Upnp { code, .. } => Some(*code),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upnp_code() {
        let upnp = Error::Upnp {
            code: 713,
            description: "SpecifiedArrayIndexInvalid".to_owned(),
        };
        assert_eq!(upnp.upnp_code(), Some(713));
        assert_eq!(Error::AuthenticationFailed.upnp_code(), None);
    }
}
