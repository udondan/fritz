//! `fritz config`: the profiles in the configuration file.

use std::io::Write;

use fritz_core::config::Profile;
use fritz_core::connection::Target;

use crate::context::Context;

/// Creates or updates the profile with the given settings.
pub(crate) fn init(context: &mut Context, out: &mut impl Write) -> anyhow::Result<()> {
    let target = Target::resolve(context.profile(), context.overrides())?;
    let profile = Profile {
        host: context.host(),
        username: target.username,
        fingerprint: target.fingerprint,
    };
    context.save_profile(profile)?;
    let (name, path) = (context.profile_name(), context.path().display());
    writeln!(out, "saved the profile {name} to {path}")?;
    Ok(())
}

/// Shows the settings in effect, without the password.
pub(crate) fn show(context: &Context, out: &mut impl Write) -> anyhow::Result<()> {
    let target = context.target()?;
    let none = || "none".to_owned();
    let profile = context
        .profile()
        .map_or_else(none, |_| context.profile_name().to_owned());
    let certificate = match (target.url.scheme(), target.fingerprint) {
        ("https", Some(fingerprint)) => fingerprint.to_string(),
        ("https", None) => "not trusted, run `fritz tls trust`".to_owned(),
        _ => "none, the connection is not encrypted".to_owned(),
    };
    let password = match context.password(&target) {
        Ok((_, source)) => format!("from {source}"),
        Err(error) => format!("{error:#}"),
    };
    writeln!(out, "config       {}", context.path().display())?;
    writeln!(out, "profile      {profile}")?;
    writeln!(out, "address      {}", target.url)?;
    let user = target
        .username
        .clone()
        .unwrap_or_else(|| "none, for devices with only a password".to_owned());
    writeln!(out, "user         {user}")?;
    writeln!(out, "certificate  {certificate}")?;
    writeln!(out, "password     {password}")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::tests::{agent, context};

    #[test]
    fn creates_and_shows_a_profile() {
        let dir = tempfile::tempdir().unwrap();
        let mut context = context(dir.path(), agent());
        let mut out = Vec::new();
        init(&mut context, &mut out).unwrap();
        let path = dir.path().join("config.toml");
        assert_eq!(
            String::from_utf8(out).unwrap(),
            format!("saved the profile default to {}\n", path.display())
        );

        let mut out = Vec::new();
        show(&context, &mut out).unwrap();
        assert_eq!(
            String::from_utf8(out).unwrap(),
            format!(
                "config       {}\n\
                 profile      default\n\
                 address      https://192.168.178.1:49443/\n\
                 user         agent\n\
                 certificate  not trusted, run `fritz tls trust`\n\
                 password     no password for agent@192.168.178.1, run `fritz auth login` or set FRITZ_PASSWORD\n",
                path.display()
            )
        );
    }
}
