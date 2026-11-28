//! `fritz tr064`: the services and actions of the device, and calls of the
//! actions that only read.

use std::collections::BTreeMap;
use std::io::Write;

use anyhow::{Context as _, bail};
use fritz_core::capture::short_id;
use fritz_core::generic;
use fritz_core::policy;
use serde::Serialize;
use tr064::{Argument, Client, Direction, Scpd, Service, StateVariable};

use crate::context::Context;
use crate::output::{self, Format};

/// A client without credentials, for the descriptions, which need no login.
fn anonymous(context: &Context) -> anyhow::Result<Client> {
    Ok(context.builder(&context.target()?)?.build()?)
}

async fn service(client: &Client, reference: &str) -> anyhow::Result<(Service, Scpd)> {
    generic::service(client, reference)
        .await
        .with_context(|| format!("cannot read the service {reference}"))
}

/// Prints the services, one per line, like `DeviceInfo:1  DeviceInfo1`, or
/// as a map from the short ID to the type.
pub(crate) async fn services(
    context: &Context,
    format: Format,
    out: &mut impl Write,
) -> anyhow::Result<()> {
    let description = anonymous(context)?
        .description()
        .await
        .context("cannot read the device description")?;
    let rows: Vec<(String, &str)> = description
        .services()
        .map(|s| (format!("{}:{}", s.name(), s.version()), short_id(s)))
        .collect();
    let map: BTreeMap<&str, &str> = rows.iter().map(|(t, id)| (*id, t.as_str())).collect();
    output::show(&map, format, out, |_, out| {
        let width = rows.iter().map(|(t, _)| t.len()).max().unwrap_or_default();
        for (service_type, id) in &rows {
            writeln!(out, "{service_type:width$}  {id}")?;
        }
        Ok(())
    })
}

/// Prints the actions of a service with their classes, like
/// `read      GetInfo`, or as a map from the action to its class.
pub(crate) async fn actions(
    context: &Context,
    reference: &str,
    format: Format,
    out: &mut impl Write,
) -> anyhow::Result<()> {
    let (service, scpd) = service(&anonymous(context)?, reference).await?;
    let rows: Vec<(&str, &str)> = scpd
        .actions
        .iter()
        .map(|a| (a.name.as_str(), policy::classify(&service, a).name()))
        .collect();
    let map: BTreeMap<&str, &str> = rows.iter().copied().collect();
    output::show(&map, format, out, |_, out| {
        for (name, class) in &rows {
            writeln!(out, "{class:<9} {name}")?;
        }
        Ok(())
    })
}

/// An action with its class and its arguments with their types.
#[derive(Serialize)]
struct Description<'a> {
    action: &'a str,
    class: String,
    #[serde(rename = "in")]
    inputs: BTreeMap<&'a str, String>,
    #[serde(rename = "out")]
    outputs: BTreeMap<&'a str, String>,
}

/// Prints the class of an action and its arguments with their types.
pub(crate) async fn describe(
    context: &Context,
    reference: &str,
    name: &str,
    format: Format,
    out: &mut impl Write,
) -> anyhow::Result<()> {
    let (service, scpd) = service(&anonymous(context)?, reference).await?;
    let action = generic::action(&service, &scpd, name)?;
    let class = policy::classify(&service, action);
    let kind_of = |argument: &Argument| {
        scpd.state_variable(&argument.related_state_variable)
            .map_or_else(String::new, kind)
    };
    let mut description = Description {
        action: &action.name,
        class: class.to_string(),
        inputs: BTreeMap::new(),
        outputs: BTreeMap::new(),
    };
    for argument in &action.arguments {
        let arguments = match argument.direction {
            Direction::In => &mut description.inputs,
            Direction::Out => &mut description.outputs,
        };
        arguments.insert(&argument.name, kind_of(argument));
    }
    output::show(&description, format, out, |_, out| {
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
            let argument_name = &argument.name;
            let kind = kind_of(argument);
            writeln!(out, "  {direction}  {argument_name:width$}  {kind}")?;
        }
        Ok(())
    })
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

/// A call of an action, like `DeviceInfo1 GetInfo`.
pub(crate) struct Call<'a> {
    /// The service, like `DeviceInfo1` or `DeviceInfo:1`.
    pub(crate) service: &'a str,
    /// The action, like `GetInfo`.
    pub(crate) action: &'a str,
    /// The inputs, like `NewIndex=0`.
    pub(crate) arguments: &'a [String],
}

/// Calls an action that only reads, and prints its output with secrets
/// hidden, as aligned lines or as a map from the argument to its value. The
/// action is checked before the password is read and before anything is sent
/// to its control URL.
pub(crate) async fn call(
    context: &Context,
    call: &Call<'_>,
    format: Format,
    out: &mut impl Write,
) -> anyhow::Result<()> {
    let arguments = parse_arguments(call.arguments)?;
    let target = context.target()?;
    let (service, scpd) = service(&context.builder(&target)?.build()?, call.service).await?;
    let action = generic::action(&service, &scpd, call.action)?;
    generic::check(&service, action, &arguments)?;
    let client = context.client().await?;
    let output = generic::call(&client, &service, action, &arguments)
        .await
        .with_context(|| format!("{} failed", action.name))?;
    let map: BTreeMap<&str, &str> = output
        .iter()
        .map(|(n, v)| (n.as_str(), v.as_str()))
        .collect();
    output::show(&map, format, out, |_, out| {
        let width = output
            .iter()
            .map(|(n, _)| n.len())
            .max()
            .unwrap_or_default();
        for (argument, value) in &output {
            writeln!(out, "{argument:width$}  {value}")?;
        }
        Ok(())
    })
}
