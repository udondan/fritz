//! The configuration file: named profiles with the address of a device, the
//! user, the trusted certificate and optionally a command that prints the
//! password. Passwords never go here, see [`secrets`](crate::secrets).
//!
//! ```toml
//! default_profile = "home"
//!
//! [profile.home]
//! host = "192.168.178.1"
//! username = "agent"
//! fingerprint = "AB:CD:…"
//! password_command = ["op", "read", "op://Private/FRITZ!Box/password"]
//!
//! [write]
//! deny = ["<identity of the device>"]
//! ```
//!
//! The `write` section lists the devices that may and may never be changed,
//! see [`identity`](crate::identity).

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use tr064::Fingerprint;

use crate::identity::Targets;
use crate::secrets::command::PasswordCommand;

/// The configuration file could not be read or written.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The file exists but cannot be read.
    #[error("cannot read {}", path.display())]
    Read {
        /// The file.
        path: PathBuf,
        /// The cause.
        source: io::Error,
    },
    /// The file is not a valid configuration.
    #[error("{} is not a valid configuration", path.display())]
    Parse {
        /// The file.
        path: PathBuf,
        /// The cause.
        source: toml::de::Error,
    },
    /// The file cannot be written.
    #[error("cannot write {}", path.display())]
    Write {
        /// The file.
        path: PathBuf,
        /// The cause.
        source: io::Error,
    },
    /// No profile has the given name.
    #[error("there is no profile named {0}")]
    UnknownProfile(String),
}

/// The contents of the configuration file.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// The profile to use when none is given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_profile: Option<String>,
    /// The profiles by name.
    #[serde(
        default,
        rename = "profile",
        skip_serializing_if = "BTreeMap::is_empty"
    )]
    pub profiles: BTreeMap<String, Profile>,
    /// The devices that may and may never be changed.
    #[serde(default, skip_serializing_if = "Targets::is_empty")]
    pub write: Targets,
}

/// How to reach one device.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    /// The host name, `host:port`, or the full TR-064 address. See
    /// [`connection::parse_host`](crate::connection::parse_host).
    pub host: String,
    /// The user on the device.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    /// The fingerprint of the trusted certificate.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        serialize_with = "serialize_fingerprint",
        deserialize_with = "deserialize_fingerprint"
    )]
    pub fingerprint: Option<Fingerprint>,
    /// The command that prints the password, see
    /// [`command`](crate::secrets::command).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password_command: Option<PasswordCommand>,
}

#[expect(clippy::ref_option, reason = "the signature serde expects")]
fn serialize_fingerprint<S: Serializer>(
    fingerprint: &Option<Fingerprint>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    fingerprint.map(|f| f.to_string()).serialize(serializer)
}

fn deserialize_fingerprint<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Fingerprint>, D::Error> {
    Option::<String>::deserialize(deserializer)?
        .map(|s| s.parse().map_err(serde::de::Error::custom))
        .transpose()
}

impl Config {
    /// The usual place of the configuration file, like
    /// `~/.config/fritz/config.toml` on Linux and macOS, see
    /// [`dirs`](crate::dirs). `None` if the home directory is unknown.
    pub fn default_path() -> Option<PathBuf> {
        crate::dirs::config().map(|dir| dir.join("config.toml"))
    }

    /// Reads the configuration, or returns an empty one if the file does not
    /// exist.
    pub fn load(path: &Path) -> Result<Self, Error> {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(source) => {
                return Err(Error::Read {
                    path: path.to_owned(),
                    source,
                });
            }
        };
        toml::from_str(&text).map_err(|source| Error::Parse {
            path: path.to_owned(),
            source,
        })
    }

    /// Writes the configuration, creating the directory if needed. Replaces
    /// the file at once, so a failed write leaves the old one intact.
    pub fn save(&self, path: &Path) -> Result<(), Error> {
        let write = |source| Error::Write {
            path: path.to_owned(),
            source,
        };
        let text = toml::to_string(self)
            .map_err(io::Error::other)
            .map_err(write)?;
        // A relative file name has an empty parent, which needs no creation.
        std::fs::create_dir_all(path.parent().unwrap_or(Path::new(""))).map_err(write)?;
        let temporary = path.with_extension("toml.tmp");
        std::fs::write(&temporary, text).map_err(write)?;
        std::fs::rename(&temporary, path).map_err(write)
    }

    /// Whether others than the owner may change the file, which matters
    /// when it contains a password command. Always `false` on systems
    /// without Unix permissions, and when the file cannot be read.
    pub fn writable_by_others(path: &Path) -> bool {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::metadata(path).is_ok_and(|m| m.permissions().mode() & 0o022 != 0)
        }
        #[cfg(not(unix))]
        {
            let _ = path;
            false
        }
    }

    /// The name of the profile to use: the given one, else the default one.
    /// `None` if neither is set.
    pub fn profile_name<'a>(&'a self, name: Option<&'a str>) -> Option<&'a str> {
        name.or(self.default_profile.as_deref())
    }

    /// The profile with the given name, else the default profile, or `None`
    /// if neither is set.
    pub fn profile(&self, name: Option<&str>) -> Result<Option<(&str, &Profile)>, Error> {
        let Some(name) = self.profile_name(name) else {
            return Ok(None);
        };
        self.profiles
            .get_key_value(name)
            .map(|(name, profile)| Some((name.as_str(), profile)))
            .ok_or_else(|| Error::UnknownProfile(name.to_owned()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::Identity;

    const FINGERPRINT: &str = "00:11:22:33:44:55:66:77:88:99:AA:BB:CC:DD:EE:FF:00:11:22:33:44:55:66:77:88:99:AA:BB:CC:DD:EE:FF";

    fn home() -> Profile {
        Profile {
            host: "192.168.178.1".to_owned(),
            username: Some("agent".to_owned()),
            fingerprint: Some(FINGERPRINT.parse().unwrap()),
            password_command: Some(
                vec!["pass".to_owned(), "fritz".to_owned()]
                    .try_into()
                    .unwrap(),
            ),
        }
    }

    #[test]
    fn round_trips_through_a_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/config.toml");
        let config = Config {
            default_profile: Some("home".to_owned()),
            profiles: BTreeMap::from([
                ("home".to_owned(), home()),
                (
                    "lab".to_owned(),
                    Profile {
                        host: "lab.example".to_owned(),
                        ..Profile::default()
                    },
                ),
            ]),
            write: Targets {
                allow: Vec::new(),
                deny: vec![Identity::of("uuid:home", "1")],
            },
        };
        config.save(&path).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(
            text.contains(&format!("fingerprint = \"{FINGERPRINT}\"")),
            "{text}"
        );
        assert!(text.contains("[write]\ndeny = [\""), "{text}");
        assert!(
            text.contains("password_command = [\"pass\", \"fritz\"]"),
            "{text}"
        );
        assert!(!dir.path().join("nested/config.toml.tmp").exists());
        assert_eq!(Config::load(&path).unwrap(), config);
    }

    #[test]
    fn treats_a_missing_file_as_empty() {
        let dir = tempfile::tempdir().unwrap();
        let config = Config::load(&dir.path().join("config.toml")).unwrap();
        assert_eq!(config, Config::default());
    }

    #[test]
    fn reports_unreadable_and_invalid_files() {
        let dir = tempfile::tempdir().unwrap();
        let error = Config::load(dir.path()).unwrap_err();
        assert!(matches!(error, Error::Read { .. }), "{error:?}");

        let path = dir.path().join("config.toml");
        for text in [
            "[profile.home]\nhost = \"fritz.box\"\nfingerprint = \"00\"\n",
            "unknown = 1\n",
            "[write]\nallow = [\"00\"]\n",
            "[profile.home]\nhost = \"fritz.box\"\npassword_command = []\n",
        ] {
            std::fs::write(&path, text).unwrap();
            let error = Config::load(&path).unwrap_err();
            assert!(matches!(error, Error::Parse { .. }), "{error:?}");
            assert_eq!(
                error.to_string(),
                format!("{} is not a valid configuration", path.display())
            );
        }
    }

    #[test]
    fn reports_unwritable_files() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("file");
        std::fs::write(&file, "").unwrap();
        let error = Config::default()
            .save(&file.join("config.toml"))
            .unwrap_err();
        assert!(matches!(error, Error::Write { .. }), "{error:?}");
    }

    #[test]
    fn selects_profiles() {
        let mut config = Config {
            default_profile: None,
            profiles: BTreeMap::from([("home".to_owned(), home())]),
            write: Targets::default(),
        };
        assert_eq!(config.profile(None).unwrap(), None);
        assert_eq!(
            config.profile(Some("home")).unwrap(),
            Some(("home", &home()))
        );
        let error = config.profile(Some("lab")).unwrap_err();
        assert_eq!(error.to_string(), "there is no profile named lab");

        config.default_profile = Some("home".to_owned());
        assert_eq!(config.profile(None).unwrap(), Some(("home", &home())));
        assert_eq!(config.profile_name(Some("lab")), Some("lab"));
    }

    #[test]
    fn tells_whether_others_may_write() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        assert!(!Config::writable_by_others(&path));
        Config::default().save(&path).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let set = |mode| {
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
            };
            set(0o600);
            assert!(!Config::writable_by_others(&path));
            set(0o620);
            assert!(Config::writable_by_others(&path));
            set(0o602);
            assert!(Config::writable_by_others(&path));
        }
        #[cfg(not(unix))]
        assert!(!Config::writable_by_others(&path));
    }

    #[test]
    fn has_a_default_path() {
        let path = Config::default_path().unwrap();
        assert!(path.ends_with("config.toml"), "{}", path.display());
    }
}
