//! Which device is which: an identity that stays the same across addresses
//! and firmware updates, and the lists of devices that may and may never be
//! changed.
//!
//! The identity is the SHA-256 hash of the unique device name (UDN) in the
//! device description and the serial number `DeviceInfo:GetInfo` returns.
//! The hash names a device without revealing either of them.
//!
//! ```toml
//! [write]
//! allow = ["<identity of a test device>"]
//! deny = ["<identity of the device at home>"]
//! ```

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use crate::session::to_hex;

/// The identity of a device.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Identity(String);

/// A text is not an identity.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0} is not a device identity of 64 hex digits")]
pub struct InvalidIdentity(String);

impl Identity {
    /// The identity of the device with the unique device name `udn` and the
    /// serial number `serial`.
    pub fn of(udn: &str, serial: &str) -> Self {
        let hash = Sha256::digest(format!("{udn}\n{serial}"));
        Self(to_hex(&hash))
    }
}

impl fmt::Display for Identity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for Identity {
    type Err = InvalidIdentity;

    /// Parses 64 hex digits, in any case.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit()) {
            Ok(Self(s.to_ascii_lowercase()))
        } else {
            Err(InvalidIdentity(s.to_owned()))
        }
    }
}

impl TryFrom<String> for Identity {
    type Error = InvalidIdentity;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        value.parse()
    }
}

impl From<Identity> for String {
    fn from(identity: Identity) -> Self {
        identity.0
    }
}

/// The devices that may be changed, and those that may never be.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Targets {
    /// The devices that may be changed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allow: Vec<Identity>,
    /// The devices that may never be changed, even if they are allowed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deny: Vec<Identity>,
}

/// Whether a device may be changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// The device is allowed and not denied.
    Allowed,
    /// The device is denied.
    Denied,
    /// The device is not allowed.
    Unknown,
}

impl Targets {
    /// Whether there are no devices in either list.
    pub fn is_empty(&self) -> bool {
        self.allow.is_empty() && self.deny.is_empty()
    }

    /// Whether the device may be changed. The deny list wins.
    pub fn verdict(&self, identity: &Identity) -> Verdict {
        if self.deny.contains(identity) {
            Verdict::Denied
        } else if self.allow.contains(identity) {
            Verdict::Allowed
        } else {
            Verdict::Unknown
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashes_the_udn_and_the_serial_number() {
        let identity = Identity::of("uuid:1", "AABBCC");
        assert_eq!(identity.to_string().len(), 64);
        assert_eq!(identity, Identity::of("uuid:1", "AABBCC"));
        assert_ne!(identity, Identity::of("uuid:1", "AABBCD"));
        assert_ne!(identity, Identity::of("uuid:1A", "ABBCC"));
    }

    #[test]
    fn parses_hex_digits() {
        let identity = Identity::of("uuid:1", "AABBCC");
        let upper = identity.to_string().to_ascii_uppercase();
        assert_eq!(upper.parse::<Identity>(), Ok(identity.clone()));
        assert_eq!(
            "xyz".parse::<Identity>().unwrap_err().to_string(),
            "xyz is not a device identity of 64 hex digits"
        );
        assert!("g".repeat(64).parse::<Identity>().is_err());
        assert_eq!(String::from(identity.clone()), identity.to_string());
    }

    #[test]
    fn denies_before_it_allows() {
        let home = Identity::of("uuid:home", "1");
        let test = Identity::of("uuid:test", "2");
        let targets = Targets {
            allow: vec![home.clone(), test.clone()],
            deny: vec![home.clone()],
        };
        assert_eq!(targets.verdict(&home), Verdict::Denied);
        assert_eq!(targets.verdict(&test), Verdict::Allowed);
        assert_eq!(
            targets.verdict(&Identity::of("uuid:other", "3")),
            Verdict::Unknown
        );
        assert!(!targets.is_empty());
        assert!(Targets::default().is_empty());
    }
}
