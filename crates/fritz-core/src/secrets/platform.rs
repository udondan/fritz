//! The credential store of the operating system.
//!
//! Excluded from coverage (see `codecov.yml`): CI has no keychain, and tests
//! must never touch the keychain of a developer.

use std::sync::Arc;

use keyring_core::CredentialStore;

use super::{Error, Keychain};

impl Keychain {
    /// The keychain of the operating system.
    pub fn platform() -> Result<Self, Error> {
        store().map(Self::with_store).map_err(Error::Keychain)
    }
}

#[cfg(target_os = "macos")]
fn store() -> keyring_core::Result<Arc<CredentialStore>> {
    Ok(apple_native_keyring_store::keychain::Store::new()?)
}

#[cfg(target_os = "windows")]
fn store() -> keyring_core::Result<Arc<CredentialStore>> {
    Ok(windows_native_keyring_store::Store::new()?)
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn store() -> keyring_core::Result<Arc<CredentialStore>> {
    Ok(zbus_secret_service_keyring_store::Store::new()?)
}
