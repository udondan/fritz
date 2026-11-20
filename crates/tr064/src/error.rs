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
}
