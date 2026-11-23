//! `fritz auth`: the password in the keychain.

use std::io::{self, Write};

use anyhow::{Context as _, bail};
use fritz_core::secrets::Source;
use secrecy::ExposeSecret as _;
use tr064::SecretString;

use crate::context::Context;

/// Reads the password and stores it in the keychain. Does not contact the
/// device.
pub(crate) fn login(
    context: &Context,
    read_password: impl FnOnce(&str) -> io::Result<SecretString>,
    out: &mut impl Write,
) -> anyhow::Result<()> {
    let account = context.target()?.account()?;
    let keychain = context.keychain()?;
    let password =
        read_password(&format!("Password of {account}: ")).context("cannot read the password")?;
    if password.expose_secret().is_empty() {
        bail!("the password is empty");
    }
    keychain.set(&account, &password)?;
    writeln!(out, "stored the password of {account} in the keychain")?;
    Ok(())
}

/// Removes the password from the keychain.
pub(crate) fn logout(context: &Context, out: &mut impl Write) -> anyhow::Result<()> {
    let account = context.target()?.account()?;
    if context.keychain()?.delete(&account)? {
        writeln!(out, "removed the password of {account} from the keychain")?;
    } else {
        writeln!(out, "the keychain has no password for {account}")?;
    }
    Ok(())
}

/// Tells where the password comes from. Fails without one.
pub(crate) async fn status(context: &Context, out: &mut impl Write) -> anyhow::Result<()> {
    let target = context.target()?;
    let (_, source) = context.password(&target).await?;
    let account = target.account()?;
    let ran = if matches!(source, Source::Command(_)) {
        ", which ran successfully"
    } else {
        ""
    };
    writeln!(out, "the password of {account} comes from {source}{ran}")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::tests::{agent, context};

    fn run(result: anyhow::Result<()>, out: Vec<u8>) -> String {
        result.unwrap();
        String::from_utf8(out).unwrap()
    }

    #[tokio::test]
    async fn logs_in_and_out() {
        let dir = tempfile::tempdir().unwrap();
        let context = context(dir.path(), agent());

        let mut out = Vec::new();
        let error = status(&context, &mut out).await.unwrap_err();
        assert!(error.to_string().starts_with("no password"), "{error}");

        let mut out = Vec::new();
        let result = login(
            &context,
            |prompt| {
                assert_eq!(prompt, "Password of agent@192.168.178.1: ");
                Ok("secret".into())
            },
            &mut out,
        );
        assert_eq!(
            run(result, out),
            "stored the password of agent@192.168.178.1 in the keychain\n"
        );

        let mut out = Vec::new();
        assert_eq!(
            run(status(&context, &mut out).await, out),
            "the password of agent@192.168.178.1 comes from the keychain\n"
        );

        let mut out = Vec::new();
        assert_eq!(
            run(logout(&context, &mut out), out),
            "removed the password of agent@192.168.178.1 from the keychain\n"
        );
        let mut out = Vec::new();
        assert_eq!(
            run(logout(&context, &mut out), out),
            "the keychain has no password for agent@192.168.178.1\n"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn tells_that_the_command_ran() {
        let dir = tempfile::tempdir().unwrap();
        let command = vec!["printf".to_owned(), "secret".to_owned()];
        let context =
            context(dir.path(), agent()).with_password_command(Some(command.try_into().unwrap()));
        let mut out = Vec::new();
        assert_eq!(
            run(status(&context, &mut out).await, out),
            "the password of agent@192.168.178.1 comes from the command `printf`, \
             which ran successfully\n"
        );
    }

    #[test]
    fn rejects_unreadable_and_empty_passwords() {
        let dir = tempfile::tempdir().unwrap();
        let context = context(dir.path(), agent());
        let mut out = Vec::new();
        let error = login(&context, |_| Ok(String::new().into()), &mut out).unwrap_err();
        assert_eq!(error.to_string(), "the password is empty");
        let error = login(
            &context,
            |_| Err(io::ErrorKind::BrokenPipe.into()),
            &mut out,
        )
        .unwrap_err();
        assert_eq!(error.to_string(), "cannot read the password");
        assert_eq!(String::from_utf8(out).unwrap(), "");
    }
}
