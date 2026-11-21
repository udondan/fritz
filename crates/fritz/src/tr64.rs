//! `fritz tr64`: the services and actions of the device, and calls of the
//! actions that only read.

use std::io::Write;

use anyhow::{Context as _, bail};
use fritz_core::capture::short_id;
use fritz_core::generic;
use fritz_core::policy;
use tr064::{Client, Direction, Scpd, Service, StateVariable};

use crate::context::Context;

/// A client without credentials, for the descriptions, which need no login.
fn anonymous(context: &Context) -> anyhow::Result<Client> {
    Ok(context.target()?.client()?.build()?)
}

async fn service(client: &Client, reference: &str) -> anyhow::Result<(Service, Scpd)> {
    generic::service(client, reference)
        .await
        .with_context(|| format!("cannot read the service {reference}"))
}

/// Prints the services, one per line, like `DeviceInfo:1  DeviceInfo1`.
pub(crate) async fn services(context: &Context, out: &mut impl Write) -> anyhow::Result<()> {
    let description = anonymous(context)?
        .description()
        .await
        .context("cannot read the device description")?;
    let rows: Vec<(String, &str)> = description
        .services()
        .map(|s| (format!("{}:{}", s.name(), s.version()), short_id(s)))
        .collect();
    let width = rows.iter().map(|(t, _)| t.len()).max().unwrap_or_default();
    for (service_type, id) in rows {
        writeln!(out, "{service_type:width$}  {id}")?;
    }
    Ok(())
}

/// Prints the actions of a service with their classes, like
/// `read      GetInfo`.
pub(crate) async fn actions(
    context: &Context,
    reference: &str,
    out: &mut impl Write,
) -> anyhow::Result<()> {
    let (service, scpd) = service(&anonymous(context)?, reference).await?;
    for action in &scpd.actions {
        let class = policy::classify(&service, action);
        writeln!(out, "{:<9} {}", class.name(), action.name)?;
    }
    Ok(())
}

/// Prints the class of an action and its arguments with their types.
pub(crate) async fn describe(
    context: &Context,
    reference: &str,
    name: &str,
    out: &mut impl Write,
) -> anyhow::Result<()> {
    let (service, scpd) = service(&anonymous(context)?, reference).await?;
    let action = generic::action(&service, &scpd, name)?;
    let class = policy::classify(&service, action);
    writeln!(out, "{} ({class})", action.name)?;
    let width = action
        .arguments
        .iter()
        .map(|a| a.name.len())
        .max()
        .unwrap_or_default();
    for argument in &action.arguments {
        let direction = match argument.direction {
            Direction::In => "in ",
            Direction::Out => "out",
        };
        let kind = scpd
            .state_variable(&argument.related_state_variable)
            .map_or_else(String::new, kind);
        let argument_name = &argument.name;
        writeln!(out, "  {direction}  {argument_name:width$}  {kind}")?;
    }
    Ok(())
}

/// The data type of a variable with the values it allows, like
/// `string (Ethernet, 802.11)` or `ui2 (0..100)`.
pub(crate) fn kind(variable: &StateVariable) -> String {
    let data_type = &variable.data_type;
    if !variable.allowed_values.is_empty() {
        return format!("{data_type} ({})", variable.allowed_values.join(", "));
    }
    match &variable.allowed_range {
        Some(range) => {
            let minimum = range.minimum.as_deref().unwrap_or_default();
            let maximum = range.maximum.as_deref().unwrap_or_default();
            format!("{data_type} ({minimum}..{maximum})")
        }
        None => data_type.clone(),
    }
}

/// Splits arguments like `NewIndex=0` into names and values.
pub(crate) fn parse_arguments(arguments: &[String]) -> anyhow::Result<Vec<(String, String)>> {
    arguments
        .iter()
        .map(|a| match a.split_once('=') {
            Some((n, v)) => Ok((n.to_owned(), v.to_owned())),
            None => bail!("{a} is not NAME=VALUE"),
        })
        .collect()
}

/// Calls an action that only reads, and prints its output with secrets
/// hidden. The action is checked before the password is read and before
/// anything is sent to its control URL.
pub(crate) async fn call(
    context: &Context,
    reference: &str,
    name: &str,
    arguments: &[String],
    out: &mut impl Write,
) -> anyhow::Result<()> {
    let arguments = parse_arguments(arguments)?;
    let target = context.target()?;
    let (service, scpd) = service(&target.client()?.build()?, reference).await?;
    let action = generic::action(&service, &scpd, name)?;
    generic::check(&service, action, &arguments)?;
    let client = context.client()?;
    let output = generic::call(&client, &service, action, &arguments)
        .await
        .with_context(|| format!("{} failed", action.name))?;
    let width = output
        .iter()
        .map(|(n, _)| n.len())
        .max()
        .unwrap_or_default();
    for (argument, value) in output {
        writeln!(out, "{argument:width$}  {value}")?;
    }
    Ok(())
}
