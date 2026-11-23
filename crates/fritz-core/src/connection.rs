//! Where and how to connect: the device address, the user and the trusted
//! certificate, from a profile and the overrides of the command line or the
//! environment.

use std::sync::Arc;

use tr064::{Client, ClientBuilder, Fingerprint, Url};

use crate::config::Profile;
use crate::redact;

/// The host when neither the command line nor a profile names one.
pub const DEFAULT_HOST: &str = "fritz.box";

/// The user name sent to TR-064 when none is set. Devices with only a
/// password, like FRITZ!Repeaters or a FRITZ!Box set to log in with the
/// password alone, use a user of their own instead of the one sent, so any
/// name works. This one is the historic default of TR-064.
pub const PASSWORD_ONLY_USERNAME: &str = "dslf-config";

/// The TR-064 port for HTTPS.
pub const HTTPS_PORT: u16 = 49443;

/// The TR-064 port for HTTP.
pub const HTTP_PORT: u16 = 49000;

/// The connection settings are incomplete or invalid.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The host cannot be turned into a TR-064 address.
    #[error("{host} is not a valid host: {reason}")]
    InvalidHost {
        /// The host as given.
        host: String,
        /// What is wrong with it.
        reason: &'static str,
    },
    /// The address is `https`, but no certificate is trusted for it.
    #[error("no certificate is trusted for {0}, run `fritz tls trust`")]
    NotTrusted(Url),
    /// The fingerprint given as an override is not one.
    #[error("{0} is not a SHA-256 fingerprint of 64 hex digits")]
    InvalidFingerprint(String),
    /// No user is set. No longer returned: without a user, the device is
    /// taken to have only a password.
    #[error("no user is set for {0}, pass --user or run `fritz config init --user <name>`")]
    NoUsername(Url),
}

/// Turns a host into the TR-064 address of the device.
///
/// A host name or IP address, optionally with a port, means HTTPS on port
/// 49443 unless another port is given: `fritz.box` becomes
/// `https://fritz.box:49443`. A full `http` or `https` address is taken as
/// is, like `http://192.168.178.1:49000` for unencrypted access.
pub fn parse_host(host: &str) -> Result<Url, Error> {
    let invalid = |reason| Error::InvalidHost {
        host: host.to_owned(),
        reason,
    };
    let url = if host.contains("://") {
        let url = Url::parse(host).map_err(|_| invalid("not an address"))?;
        if !matches!(url.scheme(), "http" | "https") {
            return Err(invalid("the scheme is neither http nor https"));
        }
        url
    } else {
        let mut url =
            Url::parse(&format!("https://{host}")).map_err(|_| invalid("not a host name"))?;
        let port = url.port().unwrap_or(HTTPS_PORT);
        // Fails only for addresses without a host, which https rules out.
        let _ = url.set_port(Some(port));
        url
    };
    if url.path() != "/" || url.query().is_some() || url.fragment().is_some() {
        return Err(invalid("the address has a path"));
    }
    Ok(url)
}

/// Settings from the command line or the environment, which take precedence
/// over the profile.
#[derive(Debug, Default, Clone)]
pub struct Overrides {
    /// The host, see [`parse_host`].
    pub host: Option<String>,
    /// The user on the device.
    pub username: Option<String>,
    /// The fingerprint of the certificate to trust, as given. It is only
    /// checked when it is used, so that a server can start without one.
    pub fingerprint: Option<String>,
}

impl Overrides {
    /// The fingerprint of the certificate to trust, if one is given.
    pub fn fingerprint(&self) -> Result<Option<Fingerprint>, Error> {
        self.fingerprint
            .as_deref()
            .map(|f| {
                f.parse()
                    .map_err(|_| Error::InvalidFingerprint(f.to_owned()))
            })
            .transpose()
    }
}

/// The device to connect to, with everything but the password.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    /// The TR-064 address.
    pub url: Url,
    /// The user on the device.
    pub username: Option<String>,
    /// The fingerprint of the trusted certificate.
    pub fingerprint: Option<Fingerprint>,
}

impl Target {
    /// Combines a profile with the overrides. Without either, the target is
    /// [`DEFAULT_HOST`].
    ///
    /// The user of the profile applies only to the host of the profile, also
    /// over another scheme or port; its certificate only to the same
    /// address. Neither applies to another host given as an override.
    pub fn resolve(profile: Option<&Profile>, overrides: &Overrides) -> Result<Self, Error> {
        let host = overrides
            .host
            .as_deref()
            .or(profile.map(|p| p.host.as_str()))
            .unwrap_or(DEFAULT_HOST);
        let url = parse_host(host)?;
        let own = profile.and_then(|p| parse_host(&p.host).ok().map(|own| (p, own)));
        let same_host = own
            .as_ref()
            .filter(|(_, own)| own.host_str() == url.host_str());
        let same_address = own.as_ref().filter(|(_, own)| *own == url);
        Ok(Self {
            username: overrides
                .username
                .clone()
                .or_else(|| same_host.and_then(|(p, _)| p.username.clone())),
            fingerprint: overrides
                .fingerprint()?
                .or(same_address.and_then(|(p, _)| p.fingerprint)),
            url,
        })
    }

    /// The user for TR-064, or [`PASSWORD_ONLY_USERNAME`] if none is set.
    ///
    /// # Errors
    ///
    /// None; the result is kept for compatibility.
    pub fn username(&self) -> Result<&str, Error> {
        Ok(self.username.as_deref().unwrap_or(PASSWORD_ONLY_USERNAME))
    }

    /// The user for the login to the web interface, empty if none is set,
    /// as the web interface expects for devices with only a password.
    #[must_use]
    pub fn web_username(&self) -> &str {
        self.username.as_deref().unwrap_or_default()
    }

    /// The name of the password in the keychain: `user@host`, or the host
    /// alone if no user is set.
    ///
    /// # Errors
    ///
    /// None; the result is kept for compatibility.
    pub fn account(&self) -> Result<String, Error> {
        let host = self.url.host_str().unwrap_or_default();
        Ok(match &self.username {
            Some(username) => format!("{username}@{host}"),
            None => host.to_owned(),
        })
    }

    /// The web interface of the device, which serves the session login and
    /// the REST API: the TR-064 ports 49443 and 49000 become the default
    /// ports of `https` and `http`, other addresses stay as they are. The
    /// device uses the same certificate for both.
    #[must_use]
    pub fn web(&self) -> Self {
        let mut url = self.url.clone();
        let port = match (url.scheme(), url.port()) {
            ("https", Some(HTTPS_PORT)) | ("http", Some(HTTP_PORT)) => None,
            (_, port) => port,
        };
        // Fails only for addresses without a host, which parse_host rules out.
        let _ = url.set_port(port);
        Self {
            url,
            ..self.clone()
        }
    }

    /// A client builder for the device, with the certificate pinned for
    /// `https` and the log redacted by [`redact::Log`]. Without credentials,
    /// which only actions need.
    pub fn client(&self) -> Result<ClientBuilder, Error> {
        let builder = Client::builder(self.url.clone()).redact(Arc::new(redact::Log));
        match (self.url.scheme(), self.fingerprint) {
            ("https", Some(fingerprint)) => Ok(builder.pin(fingerprint)),
            ("https", None) => Err(Error::NotTrusted(self.url.clone())),
            _ => Ok(builder),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fingerprint(byte: u8) -> Fingerprint {
        Fingerprint::of(&[byte])
    }

    #[test]
    fn parses_hosts() {
        for (host, url) in [
            ("fritz.box", "https://fritz.box:49443/"),
            ("192.168.178.1:8443", "https://192.168.178.1:8443/"),
            ("[fd00::1]", "https://[fd00::1]:49443/"),
            ("http://fritz.box:49000", "http://fritz.box:49000/"),
            ("https://fritz.box", "https://fritz.box/"),
        ] {
            assert_eq!(parse_host(host).unwrap().as_str(), url, "{host}");
        }
    }

    #[test]
    fn rejects_invalid_hosts() {
        for (host, reason) in [
            ("ftp://fritz.box", "the scheme is neither http nor https"),
            ("http://", "not an address"),
            ("fritz box", "not a host name"),
            ("fritz.box/path", "the address has a path"),
            ("https://fritz.box/?query", "the address has a path"),
            ("fritz.box#top", "the address has a path"),
        ] {
            let error = parse_host(host).unwrap_err();
            assert_eq!(
                error.to_string(),
                format!("{host} is not a valid host: {reason}")
            );
        }
    }

    #[test]
    fn defaults_to_fritz_box() {
        let target = Target::resolve(None, &Overrides::default()).unwrap();
        assert_eq!(target.url.as_str(), "https://fritz.box:49443/");
        assert_eq!(target.username, None);
        assert_eq!(target.fingerprint, None);
        // Without a user, the device is taken to have only a password.
        assert_eq!(target.username().unwrap(), PASSWORD_ONLY_USERNAME);
        assert_eq!(target.web_username(), "");
        assert_eq!(target.account().unwrap(), "fritz.box");
        assert_eq!(
            target.client().unwrap_err().to_string(),
            "no certificate is trusted for https://fritz.box:49443/, run `fritz tls trust`"
        );
    }

    #[test]
    fn takes_the_profile() {
        let profile = Profile {
            host: "192.168.178.1".to_owned(),
            username: Some("agent".to_owned()),
            fingerprint: Some(fingerprint(1)),
        };
        let target = Target::resolve(Some(&profile), &Overrides::default()).unwrap();
        assert_eq!(target.url.as_str(), "https://192.168.178.1:49443/");
        assert_eq!(target.account().unwrap(), "agent@192.168.178.1");
        assert_eq!(target.username().unwrap(), "agent");
        assert_eq!(target.web_username(), "agent");
        assert_eq!(target.fingerprint, Some(fingerprint(1)));
        assert!(target.client().unwrap().build().is_ok());

        // The same host, written differently, keeps the user and the
        // certificate.
        let overrides = Overrides {
            host: Some("https://192.168.178.1:49443".to_owned()),
            ..Overrides::default()
        };
        let target = Target::resolve(Some(&profile), &overrides).unwrap();
        assert_eq!(target.username.as_deref(), Some("agent"));
        assert_eq!(target.fingerprint, Some(fingerprint(1)));

        // The same host over HTTP keeps the user, but not the certificate.
        let overrides = Overrides {
            host: Some("http://192.168.178.1:49000".to_owned()),
            ..Overrides::default()
        };
        let target = Target::resolve(Some(&profile), &overrides).unwrap();
        assert_eq!(target.username.as_deref(), Some("agent"));
        assert_eq!(target.fingerprint, None);

        // Another host, like a repeater, gets neither.
        let overrides = Overrides {
            host: Some("http://192.168.178.4:49000".to_owned()),
            ..Overrides::default()
        };
        let target = Target::resolve(Some(&profile), &overrides).unwrap();
        assert_eq!(target.username, None);
        assert_eq!(target.fingerprint, None);
        assert_eq!(target.account().unwrap(), "192.168.178.4");
    }

    #[test]
    fn prefers_the_overrides() {
        let profile = Profile {
            host: "192.168.178.1".to_owned(),
            username: Some("agent".to_owned()),
            fingerprint: Some(fingerprint(1)),
        };
        let overrides = Overrides {
            host: Some("lab.example:8443".to_owned()),
            username: Some("admin".to_owned()),
            fingerprint: None,
        };
        let target = Target::resolve(Some(&profile), &overrides).unwrap();
        assert_eq!(target.url.as_str(), "https://lab.example:8443/");
        assert_eq!(target.account().unwrap(), "admin@lab.example");
        // The certificate of the profile is not trusted for another host.
        assert_eq!(target.fingerprint, None);

        let overrides = Overrides {
            fingerprint: Some(fingerprint(2).to_string()),
            ..overrides
        };
        let target = Target::resolve(Some(&profile), &overrides).unwrap();
        assert_eq!(target.fingerprint, Some(fingerprint(2)));

        // An invalid fingerprint fails only when it is used.
        let overrides = Overrides {
            fingerprint: Some("se://docker/mcp/fritz.fingerprint".to_owned()),
            ..overrides
        };
        assert_eq!(
            Target::resolve(Some(&profile), &overrides)
                .unwrap_err()
                .to_string(),
            "se://docker/mcp/fritz.fingerprint is not a SHA-256 fingerprint of 64 hex digits"
        );
    }

    #[test]
    fn ignores_the_certificate_for_http() {
        let overrides = Overrides {
            host: Some("http://127.0.0.1:49000".to_owned()),
            fingerprint: Some(fingerprint(1).to_string()),
            ..Overrides::default()
        };
        let target = Target::resolve(None, &overrides).unwrap();
        assert!(target.client().unwrap().build().is_ok());
    }

    #[test]
    fn derives_the_web_interface() {
        for (host, web) in [
            ("fritz.box", "https://fritz.box/"),
            ("http://fritz.box:49000", "http://fritz.box/"),
            ("fritz.box:8443", "https://fritz.box:8443/"),
            ("http://127.0.0.1:49443", "http://127.0.0.1:49443/"),
            ("https://fritz.box", "https://fritz.box/"),
        ] {
            let overrides = Overrides {
                host: Some(host.to_owned()),
                username: Some("agent".to_owned()),
                fingerprint: Some(fingerprint(1).to_string()),
            };
            let target = Target::resolve(None, &overrides).unwrap();
            let expected = Target {
                url: Url::parse(web).unwrap(),
                ..target.clone()
            };
            assert_eq!(target.web(), expected, "{host}");
        }
    }

    #[test]
    fn reports_an_invalid_profile_host() {
        let profile = Profile {
            host: "fritz box".to_owned(),
            ..Profile::default()
        };
        assert!(Target::resolve(Some(&profile), &Overrides::default()).is_err());
    }
}
