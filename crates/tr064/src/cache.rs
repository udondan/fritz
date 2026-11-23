//! A hook to keep service descriptions between runs.

use std::fmt;

use crate::Scpd;

/// Keeps the service descriptions (SCPDs) of devices, for a
/// [`Client`](crate::Client) with a [cache](crate::ClientBuilder::cache).
///
/// The client asks the cache before it fetches an SCPD, and hands it every
/// SCPD it fetches. Both calls get the device description the SCPD belongs
/// to, as the device sent it, so that the cache can tell devices and their
/// versions apart. The client only uses the cache after it has fetched the
/// device description, so every run checks the description first.
///
/// How the cache stores the documents, and which descriptions it trusts, is
/// up to the implementation. A cache that is unsure returns `None`, and the
/// client fetches the SCPD.
pub trait DescriptionCache: Send + Sync + fmt::Debug {
    /// The SCPD at `url` of the device that `description` describes, if the
    /// cache has it.
    fn scpd(&self, description: &str, url: &str) -> Option<Scpd>;

    /// Keeps the SCPD at `url` of the device that `description` describes:
    /// `xml` as the device sent it, and `scpd` as parsed from it.
    fn store(&self, description: &str, url: &str, xml: &str, scpd: &Scpd);
}
