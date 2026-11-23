//! What every command needs: the configuration, the settings of the command
//! line and the environment, and the password sources.

use std::io::{self, BufRead};
use std::path::{Path, PathBuf};

use anyhow::{Context as _, bail};
use fritz_core::config::{Config, Profile};
use fritz_core::connection::{DEFAULT_HOST, Overrides, Target};
use fritz_core::secrets::{self, Keychain, Source};
use fritz_core::session::Session;
use tr064::SecretString;

/// The environment variable that turns the keychain off, for containers and
/// tests.
pub(crate) const ENV_NO_KEYCHAIN: &str = "FRITZ_NO_KEYCHAIN";

/// The name of the profile that is created when none is given.
pub(crate) const DEFAULT_PROFILE: &str = "default";

/// Which keychain to use.
#[derive(Debug)]
pub(crate) enum Keychains {
    /// None, because of [`ENV_NO_KEYCHAIN`].
    Disabled,
    /// The keychain that the function opens, when it is first needed.
    Open(fn() -> Result<Keychain, secrets::Error>),
}

/// The configuration and the settings of one run.
#[derive(Debug)]
pub(crate) struct Context {
    path: PathBuf,
    config: Config,
    profile: Option<String>,
    overrides: Overrides,
    password: Option<SecretString>,
    keychains: Keychains,
}

impl Context {
    /// Loads the configuration from `path`, or from the default place.
    pub(crate) fn load(
        path: Option<PathBuf>,
        profile: Option<String>,
        overrides: Overrides,
        password: Option<SecretString>,
        keychains: Keychains,
    ) -> anyhow::Result<Self> {
        let path = path
            .or_else(Config::default_path)
            .context("cannot find the home directory, pass --config")?;
        let config = Config::load(&path)?;
        Ok(Self {
            path,
            config,
            profile,
            overrides,
            password,
            keychains,
        })
    }

    /// The configuration file.
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    /// The devices that may and may never be changed.
    #[cfg(feature = "write")]
    pub(crate) fn write_targets(&self) -> &fritz_core::identity::Targets {
        &self.config.write
    }

    /// The settings of the command line and the environment.
    pub(crate) fn overrides(&self) -> &Overrides {
        &self.overrides
    }

    /// The contents of the configuration file.
    pub(crate) fn config(&self) -> &Config {
        &self.config
    }

    /// The device to work with. A profile that is named, by `--profile` or
    /// as the default, must exist.
    pub(crate) fn target(&self) -> anyhow::Result<Target> {
        let profile = self.config.profile(self.profile.as_deref())?;
        Ok(Target::resolve(profile.map(|(_, p)| p), &self.overrides)?)
    }

    /// The name of the profile that commands write to: the given one, else
    /// the default one, else [`DEFAULT_PROFILE`].
    pub(crate) fn profile_name(&self) -> &str {
        self.config
            .profile_name(self.profile.as_deref())
            .unwrap_or(DEFAULT_PROFILE)
    }

    /// The profile that commands write to, if it exists.
    pub(crate) fn profile(&self) -> Option<&Profile> {
        self.config.profiles.get(self.profile_name())
    }

    /// The host to write to the profile: the given one, else the one of the
    /// profile, else [`DEFAULT_HOST`].
    pub(crate) fn host(&self) -> String {
        self.overrides
            .host
            .clone()
            .or_else(|| self.profile().map(|p| p.host.clone()))
            .unwrap_or_else(|| DEFAULT_HOST.to_owned())
    }

    /// Stores the profile under [`profile_name`](Self::profile_name), makes
    /// it the default if there is none, and saves the configuration.
    pub(crate) fn save_profile(&mut self, profile: Profile) -> anyhow::Result<()> {
        let name = self.profile_name().to_owned();
        self.config
            .default_profile
            .get_or_insert_with(|| name.clone());
        self.config.profiles.insert(name, profile);
        Ok(self.config.save(&self.path)?)
    }

    /// The keychain, or an error if it is disabled or unavailable.
    pub(crate) fn keychain(&self) -> anyhow::Result<Keychain> {
        match &self.keychains {
            Keychains::Disabled => bail!("the keychain is disabled by {ENV_NO_KEYCHAIN}"),
            Keychains::Open(open) => Ok(open()?),
        }
    }

    /// The password for the target and where it came from. Opens the
    /// keychain only without a password in the environment.
    pub(crate) fn password(&self, target: &Target) -> anyhow::Result<(SecretString, Source)> {
        let account = target.account()?;
        let keychain = match (&self.password, &self.keychains) {
            (Some(_), _) | (None, Keychains::Disabled) => None,
            _ => Some(self.keychain()?),
        };
        Ok(secrets::password(
            self.password.clone(),
            keychain.as_ref(),
            &account,
        )?)
    }

    /// A client for the target with the password, for calls of actions.
    pub(crate) fn client(&self) -> anyhow::Result<tr064::Client> {
        let target = self.target()?;
        let (password, _) = self.password(&target)?;
        let builder = target.client()?.credentials(target.username()?, password);
        Ok(builder.build()?)
    }

    /// A session of the web interface of the target, logged in with the
    /// password.
    pub(crate) async fn session(&self) -> anyhow::Result<Session> {
        let target = self.target()?;
        let (password, _) = self.password(&target)?;
        let web = target.web();
        let client = web.client()?.build()?;
        Ok(Session::login(client, web.web_username(), &password).await?)
    }
}

/// Reads one line without its line break. Empty at the end of the input.
pub(crate) fn read_line(input: &mut impl BufRead) -> io::Result<String> {
    let mut line = String::new();
    input.read_line(&mut line)?;
    let end = line.trim_end_matches(['\r', '\n']).len();
    line.truncate(end);
    Ok(line)
}

#[cfg(test)]
pub(crate) mod tests {
    use fritz_core::config::Profile;
    use keyring_core::mock;
    use secrecy::ExposeSecret as _;

    use super::*;

    thread_local! {
        static KEYCHAIN: Keychain = Keychain::with_store(mock::Store::new().unwrap());
    }

    /// A mock keychain, shared by the contexts of one test.
    #[expect(clippy::unnecessary_wraps, reason = "the signature of Keychains::Open")]
    fn mock_keychain() -> Result<Keychain, secrets::Error> {
        Ok(KEYCHAIN.with(Keychain::clone))
    }

    /// A context with a mock keychain and a configuration in the directory.
    pub(crate) fn context(dir: &Path, overrides: Overrides) -> Context {
        Context::load(
            Some(dir.join("config.toml")),
            None,
            overrides,
            None,
            Keychains::Open(mock_keychain),
        )
        .unwrap()
    }

    pub(crate) fn agent() -> Overrides {
        Overrides {
            host: Some("192.168.178.1".to_owned()),
            username: Some("agent".to_owned()),
            fingerprint: None,
        }
    }

    #[test]
    fn reads_lines() {
        let mut input = "first\r\nsecond\n".as_bytes();
        assert_eq!(read_line(&mut input).unwrap(), "first");
        assert_eq!(read_line(&mut input).unwrap(), "second");
        assert_eq!(read_line(&mut input).unwrap(), "");
    }

    #[test]
    fn saves_profiles() {
        let dir = tempfile::tempdir().unwrap();
        let mut context = context(dir.path(), agent());
        assert_eq!(context.profile_name(), "default");
        assert!(context.profile().is_none());
        let profile = Profile {
            host: "fritz.box".to_owned(),
            ..Profile::default()
        };
        context.save_profile(profile.clone()).unwrap();
        assert_eq!(context.profile(), Some(&profile));

        let config = Config::load(context.path()).unwrap();
        assert_eq!(config.default_profile.as_deref(), Some("default"));
        assert_eq!(config.profiles["default"], profile);
    }

    #[test]
    fn finds_passwords() {
        let dir = tempfile::tempdir().unwrap();
        let mut context = context(dir.path(), agent());
        let target = context.target().unwrap();
        assert!(context.password(&target).is_err());

        let keychain = context.keychain().unwrap();
        keychain
            .set("agent@192.168.178.1", &"stored".into())
            .unwrap();
        let (password, source) = context.password(&target).unwrap();
        assert_eq!(password.expose_secret(), "stored");
        assert_eq!(source, Source::Keychain);

        context.password = Some("given".into());
        context.keychains = Keychains::Disabled;
        let (password, source) = context.password(&target).unwrap();
        assert_eq!(password.expose_secret(), "given");
        assert_eq!(source, Source::Environment);
    }
}
