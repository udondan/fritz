//! `fritz tls`: trust on first use of the certificate of a device.

use std::io::{BufRead, Write};
use std::time::Duration;

use anyhow::{Context as _, bail};
use fritz_core::config::Profile;
use fritz_core::connection::{Overrides, Target};
use tr064::{Fingerprint, Url};

use crate::context::{Context, read_line};

const TIMEOUT: Duration = Duration::from_secs(10);

/// The fingerprint of the certificate the device at `url` presents.
pub(crate) async fn fetch(url: &Url) -> anyhow::Result<Fingerprint> {
    if url.scheme() != "https" {
        bail!("{url} is not https, so it has no certificate");
    }
    tr064::fetch_fingerprint(url, TIMEOUT)
        .await
        .with_context(|| format!("cannot get the certificate of {url}"))
}

/// Prints the fingerprint of the certificate the device presents.
pub(crate) async fn fingerprint(context: &Context, out: &mut impl Write) -> anyhow::Result<()> {
    let target = context.target()?;
    writeln!(out, "{}", fetch(&target.url).await?)?;
    Ok(())
}

/// Saves the fingerprint of the certificate the device presents to the
/// profile, after asking, or right away if it matches `--fingerprint`.
pub(crate) async fn trust(
    context: &mut Context,
    input: &mut impl BufRead,
    out: &mut impl Write,
) -> anyhow::Result<()> {
    let stored = Overrides {
        fingerprint: None,
        ..context.overrides().clone()
    };
    let target = Target::resolve(context.profile(), &stored)?;
    let url = &target.url;
    let found = fetch(url).await?;
    match (context.overrides().fingerprint()?, target.fingerprint) {
        (Some(expected), _) if expected != found => {
            bail!("{url} presents the certificate {found}, not {expected}")
        }
        (Some(_), _) => {}
        (None, Some(trusted)) if trusted == found => {
            writeln!(out, "the certificate of {url} is already trusted")?;
            return Ok(());
        }
        (None, Some(trusted)) => {
            eprintln!("warning: the certificate of {url} changed, the trusted one is {trusted}");
            confirm(url, found, input)?;
        }
        (None, None) => confirm(url, found, input)?,
    }
    let profile = Profile {
        host: context.host(),
        username: target.username.clone(),
        fingerprint: Some(found),
        password_command: context.profile().and_then(|p| p.password_command.clone()),
    };
    context.save_profile(profile)?;
    let name = context.profile_name();
    let message = format!("the profile {name} trusts the certificate {found} of {url}");
    writeln!(out, "{message}")?;
    Ok(())
}

fn confirm(url: &Url, found: Fingerprint, input: &mut impl BufRead) -> anyhow::Result<()> {
    eprintln!("{url} presents the certificate");
    eprintln!("  {found}");
    eprintln!("Compare it with the certificate in the web interface of the device.");
    eprint!("Trust it? [y/N] ");
    let answer = read_line(input)?;
    if !matches!(answer.trim().to_lowercase().as_str(), "y" | "yes") {
        bail!("the certificate was not trusted");
    }
    Ok(())
}

/// Removes the trusted certificate from the profile.
pub(crate) fn forget(context: &mut Context, out: &mut impl Write) -> anyhow::Result<()> {
    let name = context.profile_name().to_owned();
    let profile = context
        .profile()
        .filter(|p| p.fingerprint.is_some())
        .cloned()
        .with_context(|| format!("the profile {name} trusts no certificate"))?;
    context.save_profile(Profile {
        fingerprint: None,
        ..profile
    })?;
    writeln!(out, "the profile {name} no longer trusts a certificate")?;
    Ok(())
}
