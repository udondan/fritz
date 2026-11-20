//! Generic access to the services of a device: describing their actions and
//! calling those that only read. Every call is checked against the
//! [policy](crate::policy) before anything is sent, and its answer is
//! [redacted](crate::redact).

use tr064::{Action, Client, Scpd, Service};

use crate::policy::{self, Class};
use crate::redact;

/// Why an action was not called.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Tr064(#[from] tr064::Error),
    #[error("{service} has no action {action}")]
    UnknownAction { service: String, action: String },
    #[error("{action} is {class}; only actions that read are called")]
    Refused { action: String, class: Class },
    #[error("{action} has no input {argument}")]
    UnknownArgument { action: String, argument: String },
    #[error("{action} needs the input {argument}")]
    MissingArgument { action: String, argument: String },
}

/// Finds a service by any reference [`Service::matches`] accepts, and reads
/// its SCPD. Both documents need no login.
///
/// # Errors
///
/// The errors of reading the device description and the SCPD, and of
/// finding the service.
pub async fn service(client: &Client, reference: &str) -> Result<(Service, Scpd), Error> {
    let description = client.description().await?;
    let service = description.service(reference)?.clone();
    let scpd = client.scpd(&service).await?;
    Ok((service, scpd))
}

/// The action of a service with the given name.
///
/// # Errors
///
/// [`Error::UnknownAction`] if the service has no such action.
pub fn action<'a>(service: &Service, scpd: &'a Scpd, name: &str) -> Result<&'a Action, Error> {
    scpd.action(name).ok_or_else(|| Error::UnknownAction {
        service: service.service_id.clone(),
        action: name.to_owned(),
    })
}

/// Checks a call before it is made: the action only reads, and the
/// arguments are exactly its inputs.
///
/// # Errors
///
/// [`Error::Refused`] for an action that does not only read, and
/// [`Error::UnknownArgument`] or [`Error::MissingArgument`] if the
/// arguments do not match the inputs.
pub fn check(
    service: &Service,
    action: &Action,
    arguments: &[(String, String)],
) -> Result<(), Error> {
    let class = policy::classify(service, action);
    if !class.is_read() {
        return Err(Error::Refused {
            action: action.name.clone(),
            class,
        });
    }
    if let Some((name, _)) = arguments
        .iter()
        .find(|(name, _)| !action.inputs().any(|a| &a.name == name))
    {
        return Err(Error::UnknownArgument {
            action: action.name.clone(),
            argument: name.clone(),
        });
    }
    if let Some(input) = action
        .inputs()
        .find(|a| !arguments.iter().any(|(name, _)| *name == a.name))
    {
        return Err(Error::MissingArgument {
            action: action.name.clone(),
            argument: input.name.clone(),
        });
    }
    Ok(())
}

/// Calls an action after [checking](check) it, and returns its output
/// arguments with secrets [hidden](redact::value).
///
/// # Errors
///
/// The errors of [`check`] and of the call.
pub async fn call(
    client: &Client,
    service: &Service,
    action: &Action,
    arguments: &[(String, String)],
) -> Result<Vec<(String, String)>, Error> {
    check(service, action, arguments)?;
    let arguments: Vec<(&str, &str)> = arguments
        .iter()
        .map(|(n, v)| (n.as_str(), v.as_str()))
        .collect();
    let out = client.call(service, &action.name, &arguments).await?;
    Ok(out
        .into_iter()
        .map(|(name, value)| {
            let value = redact::value(&name, &value);
            (name, value)
        })
        .collect())
}
