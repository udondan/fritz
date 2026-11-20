//! The tokens of planned changes, in a file: a token for each plan, valid
//! once and for a few minutes, for exactly the change that was planned.
//!
//! The file holds hashes only: of the tokens, and of the changes they are
//! for, so it reveals neither the tokens nor the values of the changes.

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use super::Error;
use crate::session::to_hex;

/// The file of the tokens.
#[derive(Debug, Clone)]
pub(crate) struct Tokens {
    path: PathBuf,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Entries(BTreeMap<String, Entry>);

#[derive(Debug, Serialize, Deserialize)]
struct Entry {
    /// The hash of the change.
    change: String,
    /// When the token expires, in seconds since the Unix epoch.
    expires: u64,
}

/// The SHA-256 hash of a text as hex digits.
pub(crate) fn hash(text: &str) -> String {
    to_hex(&Sha256::digest(text))
}

impl Tokens {
    pub(crate) fn new(path: PathBuf) -> Self {
        Self { path }
    }

    /// A new token for the change with the hash `change`, valid until
    /// `expires`. Forgets the expired tokens at `now`.
    pub(crate) fn issue(&self, change: &str, now: u64, expires: u64) -> Result<String, Error> {
        let mut bytes = [0; 16];
        getrandom::fill(&mut bytes).map_err(|e| self.error(io::Error::other(e.to_string())))?;
        let token = to_hex(&bytes);
        let mut entries = self.load(now)?;
        entries.0.insert(
            hash(&token),
            Entry {
                change: change.to_owned(),
                expires,
            },
        );
        self.save(&entries)?;
        Ok(token)
    }

    /// Uses up a token for the change with the hash `change`.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidToken`] if the token is unknown, used, expired at
    /// `now` or for another change. A token for another change stays valid
    /// for its own.
    pub(crate) fn redeem(&self, token: &str, change: &str, now: u64) -> Result<(), Error> {
        let mut entries = self.load(now)?;
        let key = hash(token);
        let valid = entries.0.get(&key).is_some_and(|e| e.change == change);
        if valid {
            entries.0.remove(&key);
        }
        self.save(&entries)?;
        if valid {
            Ok(())
        } else {
            Err(Error::InvalidToken)
        }
    }

    /// The tokens that are valid at `now`.
    fn load(&self, now: u64) -> Result<Entries, Error> {
        let mut entries: Entries = match std::fs::read_to_string(&self.path) {
            Ok(text) => serde_json::from_str(&text).unwrap_or_default(),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Entries::default(),
            Err(error) => return Err(self.error(error)),
        };
        entries.0.retain(|_, e| e.expires > now);
        Ok(entries)
    }

    /// Replaces the file at once.
    fn save(&self, entries: &Entries) -> Result<(), Error> {
        let text = serde_json::to_string(entries).map_err(|e| self.error(e.into()))?;
        std::fs::create_dir_all(self.path.parent().unwrap_or(Path::new("")))
            .map_err(|e| self.error(e))?;
        let temporary = self.path.with_extension("tmp");
        std::fs::write(&temporary, text).map_err(|e| self.error(e))?;
        std::fs::rename(&temporary, &self.path).map_err(|e| self.error(e))
    }

    fn error(&self, source: io::Error) -> Error {
        Error::State {
            path: self.path.clone(),
            source,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tokens() -> (tempfile::TempDir, Tokens) {
        let dir = tempfile::tempdir().unwrap();
        let tokens = Tokens::new(dir.path().join("state/tokens.json"));
        (dir, tokens)
    }

    #[test]
    fn redeems_a_token_once() {
        let (_dir, tokens) = tokens();
        let token = tokens.issue("a", 100, 400).unwrap();
        assert_eq!(token.len(), 32);
        assert!(
            !std::fs::read_to_string(&tokens.path)
                .unwrap()
                .contains(&token)
        );
        tokens.redeem(&token, "a", 399).unwrap();
        assert!(matches!(
            tokens.redeem(&token, "a", 399),
            Err(Error::InvalidToken)
        ));
    }

    #[test]
    fn refuses_tokens_for_other_changes() {
        let (_dir, tokens) = tokens();
        let token = tokens.issue("a", 100, 400).unwrap();
        assert!(matches!(
            tokens.redeem(&token, "b", 200),
            Err(Error::InvalidToken)
        ));
        assert!(matches!(
            tokens.redeem("unknown", "a", 200),
            Err(Error::InvalidToken)
        ));
        tokens.redeem(&token, "a", 200).unwrap();
    }

    #[test]
    fn forgets_expired_tokens() {
        let (_dir, tokens) = tokens();
        let old = tokens.issue("a", 100, 400).unwrap();
        let new = tokens.issue("a", 400, 700).unwrap();
        assert!(matches!(
            tokens.redeem(&old, "a", 400),
            Err(Error::InvalidToken)
        ));
        let text = std::fs::read_to_string(&tokens.path).unwrap();
        assert!(!text.contains(&hash(&old)), "{text}");
        tokens.redeem(&new, "a", 400).unwrap();
    }

    #[test]
    fn starts_over_after_a_broken_file() {
        let (_dir, tokens) = tokens();
        std::fs::create_dir_all(tokens.path.parent().unwrap()).unwrap();
        std::fs::write(&tokens.path, "not json").unwrap();
        let token = tokens.issue("a", 100, 400).unwrap();
        tokens.redeem(&token, "a", 200).unwrap();
    }

    #[test]
    fn reports_unusable_files() {
        let dir = tempfile::tempdir().unwrap();
        let tokens = Tokens::new(dir.path().to_owned());
        let error = tokens.issue("a", 100, 400).unwrap_err();
        assert_eq!(
            error.to_string(),
            format!("cannot use {}", dir.path().display())
        );
        let file = dir.path().join("file");
        std::fs::write(&file, "").unwrap();
        let tokens = Tokens::new(file.join("tokens.json"));
        assert!(matches!(
            tokens.issue("a", 100, 400),
            Err(Error::State { .. })
        ));
    }
}
