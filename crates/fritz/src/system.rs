//! The terminal and the keychain of the operating system.
//!
//! Excluded from coverage (see `codecov.yml`): CI has neither, and tests must
//! never touch the keychain of a developer.

use std::io::{self, IsTerminal as _};

use fritz_core::secrets::Keychain;
use tr064::SecretString;

use crate::context::{ENV_NO_KEYCHAIN, Keychains, read_line};

/// The keychain of the operating system, unless [`ENV_NO_KEYCHAIN`] is set.
pub(crate) fn keychains() -> Keychains {
    if std::env::var_os(ENV_NO_KEYCHAIN).is_some_and(|value| !value.is_empty()) {
        Keychains::Disabled
    } else {
        Keychains::Open(Keychain::platform)
    }
}

/// Asks for a password on the terminal without showing it, or reads a line
/// when the input is no terminal.
pub(crate) fn read_password(prompt: &str) -> io::Result<SecretString> {
    let password = if io::stdin().is_terminal() {
        rpassword::prompt_password(prompt)?
    } else {
        read_line(&mut io::stdin().lock())?
    };
    Ok(password.into())
}
