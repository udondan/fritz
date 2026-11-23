//! Passwords: from the environment, from a command like the one of a
//! password manager, or from the keychain of the operating system (macOS
//! Keychain, Windows Credential Manager, Secret Service on Linux). Never from
//! files or the command line.

pub mod command;
mod platform;

use std::fmt;
use std::sync::Arc;

use keyring_core::{CredentialStore, Entry};
use secrecy::ExposeSecret as _;
use tr064::SecretString;

/// The service name of the passwords in the keychain.
pub const SERVICE: &str = "fritz";

/// The environment variable with the password.
pub const ENV_PASSWORD: &str = "FRITZ_PASSWORD";

/// A password could not be found, read or stored.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The keychain failed.
    #[error("the keychain failed")]
    Keychain(#[source] keyring_core::Error),
    /// The password command is invalid or failed.
    #[error(transparent)]
    Command(#[from] command::Error),
    /// Neither the environment nor the keychain has the password.
    #[error("no password for {0}, run `fritz auth login` or set {ENV_PASSWORD}")]
    Missing(String),
}

/// Passwords in a credential store, by account (`user@host`).
#[derive(Clone)]
pub struct Keychain {
    store: Arc<CredentialStore>,
}

impl fmt::Debug for Keychain {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Keychain")
            .field("store", &self.store.vendor())
            .finish()
    }
}

impl Keychain {
    /// Passwords in the given store, like a mock store in tests.
    pub fn with_store(store: Arc<CredentialStore>) -> Self {
        Self { store }
    }

    fn entry(&self, account: &str) -> Result<Entry, Error> {
        self.store
            .build(SERVICE, account, None)
            .map_err(Error::Keychain)
    }

    /// The password of the account, if one is stored.
    pub fn get(&self, account: &str) -> Result<Option<SecretString>, Error> {
        match self.entry(account)?.get_password() {
            Ok(password) => Ok(Some(password.into())),
            Err(keyring_core::Error::NoEntry) => Ok(None),
            Err(error) => Err(Error::Keychain(error)),
        }
    }

    /// Stores the password of the account, replacing an existing one.
    pub fn set(&self, account: &str, password: &SecretString) -> Result<(), Error> {
        self.entry(account)?
            .set_password(password.expose_secret())
            .map_err(Error::Keychain)
    }

    /// Removes the password of the account. `false` if none was stored.
    pub fn delete(&self, account: &str) -> Result<bool, Error> {
        match self.entry(account)?.delete_credential() {
            Ok(()) => Ok(true),
            Err(keyring_core::Error::NoEntry) => Ok(false),
            Err(error) => Err(Error::Keychain(error)),
        }
    }
}

/// Where a password came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// The environment variable [`ENV_PASSWORD`].
    Environment,
    /// The password command with the given program.
    Command(String),
    /// The keychain.
    Keychain,
}

impl fmt::Display for Source {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Environment => write!(f, "the environment variable {ENV_PASSWORD}"),
            Self::Command(name) => write!(f, "the command `{name}`"),
            Self::Keychain => f.write_str("the keychain"),
        }
    }
}

/// Finds the password of the account: the value of [`ENV_PASSWORD`] if
/// given, else the one in the keychain, if there is a keychain.
pub fn password(
    environment: Option<SecretString>,
    keychain: Option<&Keychain>,
    account: &str,
) -> Result<(SecretString, Source), Error> {
    let stored = match (environment, keychain) {
        (Some(password), _) => return Ok((password, Source::Environment)),
        (None, Some(keychain)) => keychain.get(account)?,
        (None, None) => None,
    };
    stored
        .map(|password| (password, Source::Keychain))
        .ok_or_else(|| Error::Missing(account.to_owned()))
}

#[cfg(test)]
mod tests {
    use keyring_core::mock;

    use super::*;

    const ACCOUNT: &str = "agent@fritz.box";

    fn keychain() -> Keychain {
        Keychain::with_store(mock::Store::new().unwrap())
    }

    /// Makes the next operation on the account fail.
    fn fail_next(keychain: &Keychain) {
        let entry = keychain.entry(ACCOUNT).unwrap();
        entry
            .as_any()
            .downcast_ref::<mock::Cred>()
            .unwrap()
            .set_error(keyring_core::Error::PlatformFailure("broken".into()));
    }

    #[test]
    fn stores_passwords() {
        let keychain = keychain();
        assert!(format!("{keychain:?}").contains("Mock store"));
        assert!(keychain.get(ACCOUNT).unwrap().is_none());
        assert!(!keychain.delete(ACCOUNT).unwrap());

        keychain.set(ACCOUNT, &"secret".into()).unwrap();
        let password = keychain.get(ACCOUNT).unwrap().unwrap();
        assert_eq!(password.expose_secret(), "secret");
        assert!(keychain.delete(ACCOUNT).unwrap());
        assert!(keychain.get(ACCOUNT).unwrap().is_none());
    }

    #[test]
    fn reports_keychain_failures() {
        let keychain = keychain();
        fail_next(&keychain);
        assert!(matches!(keychain.get(ACCOUNT), Err(Error::Keychain(_))));
        fail_next(&keychain);
        assert!(matches!(
            keychain.set(ACCOUNT, &"secret".into()),
            Err(Error::Keychain(_))
        ));
        fail_next(&keychain);
        assert!(matches!(keychain.delete(ACCOUNT), Err(Error::Keychain(_))));
        fail_next(&keychain);
        let error = password(None, Some(&keychain), ACCOUNT).unwrap_err();
        assert_eq!(error.to_string(), "the keychain failed");
    }

    #[test]
    fn prefers_the_environment() {
        let keychain = keychain();
        keychain.set(ACCOUNT, &"stored".into()).unwrap();

        let (found, source) = password(Some("given".into()), Some(&keychain), ACCOUNT).unwrap();
        assert_eq!(found.expose_secret(), "given");
        assert_eq!(source, Source::Environment);
        assert_eq!(
            source.to_string(),
            "the environment variable FRITZ_PASSWORD"
        );

        let (found, source) = password(None, Some(&keychain), ACCOUNT).unwrap();
        assert_eq!(found.expose_secret(), "stored");
        assert_eq!(source.to_string(), "the keychain");
        assert_eq!(
            Source::Command("op".to_owned()).to_string(),
            "the command `op`"
        );
    }

    #[test]
    fn reports_a_missing_password() {
        for keychain in [None, Some(keychain())] {
            let error = password(None, keychain.as_ref(), ACCOUNT).unwrap_err();
            assert_eq!(
                error.to_string(),
                "no password for agent@fritz.box, run `fritz auth login` or set FRITZ_PASSWORD"
            );
        }
    }
}
