//! `fritz config`: the profiles in the configuration file.

use std::io::Write;

use fritz_core::config::Profile;
use fritz_core::connection::Target;
use fritz_core::secrets::command::PasswordCommand;
use serde::Serialize;

use crate::context::Context;
use crate::status::{Format, json, table, yes_no};

/// Creates or updates the profile with the given settings. Keeps the
/// password command of the profile unless a new one is given.
pub(crate) fn init(
    context: &mut Context,
    password_command: Option<PasswordCommand>,
    out: &mut impl Write,
) -> anyhow::Result<()> {
    let target = Target::resolve(context.profile(), context.overrides())?;
    let password_command =
        password_command.or_else(|| context.profile().and_then(|p| p.password_command.clone()));
    let profile = Profile {
        host: context.host(),
        username: target.username,
        fingerprint: target.fingerprint,
        password_command,
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

/// One profile in the list.
#[derive(Debug, Serialize)]
struct Entry<'a> {
    name: &'a str,
    default: bool,
    host: &'a str,
    username: Option<&'a str>,
    fingerprint: Option<String>,
}

/// Lists the profiles of the configuration file, without contacting a device.
pub(crate) fn list(context: &Context, format: Format, out: &mut impl Write) -> anyhow::Result<()> {
    let config = context.config();
    let entries: Vec<Entry> = config
        .profiles
        .iter()
        .map(|(name, profile)| Entry {
            name,
            default: config.default_profile.as_deref() == Some(name.as_str()),
            host: &profile.host,
            username: profile.username.as_deref(),
            fingerprint: profile.fingerprint.map(|f| f.to_string()),
        })
        .collect();
    match format {
        Format::Json => json(&entries, out),
        Format::Text if entries.is_empty() => {
            let path = context.path().display();
            writeln!(out, "no profiles in {path}, run `fritz config init`")?;
            Ok(())
        }
        Format::Text => table(
            &["DEFAULT", "PROFILE", "HOST", "USER", "TRUSTED"],
            entries.iter().map(|entry| {
                vec![
                    if entry.default { "*" } else { "" }.to_owned(),
                    entry.name.to_owned(),
                    entry.host.to_owned(),
                    entry.username.unwrap_or("-").to_owned(),
                    yes_no(entry.fingerprint.is_some()),
                ]
            }),
            out,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::tests::{agent, context};

    const FINGERPRINT: &str = "00:11:22:33:44:55:66:77:88:99:AA:BB:CC:DD:EE:FF:00:11:22:33:44:55:66:77:88:99:AA:BB:CC:DD:EE:FF";

    #[test]
    fn creates_and_shows_a_profile() {
        let dir = tempfile::tempdir().unwrap();
        let mut context = context(dir.path(), agent());
        let mut out = Vec::new();
        init(&mut context, None, &mut out).unwrap();
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

    #[test]
    fn lists_the_profiles() {
        let dir = tempfile::tempdir().unwrap();
        let mut context = context(dir.path(), agent());
        let mut out = Vec::new();
        list(&context, Format::Text, &mut out).unwrap();
        assert_eq!(
            String::from_utf8(out).unwrap(),
            format!(
                "no profiles in {}, run `fritz config init`\n",
                dir.path().join("config.toml").display()
            )
        );
        let mut out = Vec::new();
        list(&context, Format::Json, &mut out).unwrap();
        assert_eq!(String::from_utf8(out).unwrap(), "[]\n");

        init(&mut context, None, &mut Vec::new()).unwrap();
        let mut context = crate::context::Context::load(
            Some(dir.path().join("config.toml")),
            Some("repeater".to_owned()),
            fritz_core::connection::Overrides {
                host: Some("192.168.178.4".to_owned()),
                username: None,
                fingerprint: Some(FINGERPRINT.parse().unwrap()),
            },
            None,
            crate::context::Keychains::Disabled,
        )
        .unwrap();
        init(&mut context, None, &mut Vec::new()).unwrap();

        let mut out = Vec::new();
        list(&context, Format::Text, &mut out).unwrap();
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "DEFAULT  PROFILE   HOST           USER   TRUSTED\n\
             *        default   192.168.178.1  agent  no\n\
             \x20        repeater  192.168.178.4  -      yes\n"
        );
        let mut out = Vec::new();
        list(&context, Format::Json, &mut out).unwrap();
        let value: serde_json::Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(
            value,
            serde_json::json!([
                {"name": "default", "default": true, "host": "192.168.178.1",
                 "username": "agent", "fingerprint": null},
                {"name": "repeater", "default": false, "host": "192.168.178.4",
                 "username": null, "fingerprint": FINGERPRINT},
            ])
        );
    }
}
