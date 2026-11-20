//! Reading a device: calls of actions that only read, through
//! [`generic::call`], with the service descriptions read once.

use std::collections::HashMap;
use std::str::FromStr;

use tr064::{Client, DeviceDescription, Scpd, Service};

use crate::generic::{self, Error};
use crate::redact;

/// Reads a device through a client with credentials.
#[derive(Debug)]
pub struct Reader {
    client: Client,
    description: DeviceDescription,
    scpds: HashMap<String, Scpd>,
}

impl Reader {
    /// Reads the device description.
    ///
    /// # Errors
    ///
    /// The errors of reading the device description.
    pub async fn new(client: Client) -> Result<Self, Error> {
        let description = client.description().await?;
        Ok(Self {
            client,
            description,
            scpds: HashMap::new(),
        })
    }

    /// Calls an action that only reads, and returns its output with secrets
    /// hidden.
    ///
    /// # Errors
    ///
    /// The errors of finding the service and the action, of
    /// [`generic::check`], and of the call.
    pub async fn read(
        &mut self,
        reference: &str,
        name: &str,
        arguments: &[(&str, &str)],
    ) -> Result<Values, Error> {
        let output = self.output(reference, name, arguments).await?;
        Ok(Values(
            output
                .into_iter()
                .map(|(name, value)| {
                    let value = redact::value(&name, &value);
                    (name, value)
                })
                .collect(),
        ))
    }

    /// Fetches a document whose path an action without inputs returns, like
    /// the host list at the path `X_AVM-DE_GetHostListPath` returns in
    /// `NewX_AVM-DE_HostListPath`. The path holds a session ID, which never
    /// leaves this function. `None` if the device has no such action, or
    /// answers it with a fault or without a path, like
    /// [`try_read`](Self::try_read).
    ///
    /// # Errors
    ///
    /// The other errors of the call, and the errors of fetching the
    /// document.
    pub async fn document(
        &mut self,
        reference: &str,
        name: &str,
        output: &str,
    ) -> Result<Option<String>, Error> {
        let values = match self.output(reference, name, &[]).await {
            Ok(values) => values,
            Err(error) if is_missing(&error) => return Ok(None),
            Err(error) => return Err(error),
        };
        let Some((_, path)) = values.iter().find(|(n, v)| n == output && !v.is_empty()) else {
            return Ok(None);
        };
        Ok(Some(self.client.get(path).await.map_err(redact::error)?))
    }

    /// Calls an action that only reads, and returns its output as it is.
    async fn output(
        &mut self,
        reference: &str,
        name: &str,
        arguments: &[(&str, &str)],
    ) -> Result<Vec<(String, String)>, Error> {
        let (service, _) = self.service(reference).await?;
        let scpd = &self.scpds[&service.service_id];
        let action = generic::action(&service, scpd, name)?;
        let arguments: Vec<(String, String)> = arguments
            .iter()
            .map(|(n, v)| ((*n).to_owned(), (*v).to_owned()))
            .collect();
        generic::send(&self.client, &service, action, &arguments).await
    }

    /// Like [`read`](Self::read), but `None` if the device has no such
    /// service or action, or answers with a fault, like 606 for missing
    /// rights.
    ///
    /// # Errors
    ///
    /// The other errors of [`read`](Self::read), like failed logins and
    /// connections.
    pub async fn try_read(
        &mut self,
        reference: &str,
        name: &str,
        arguments: &[(&str, &str)],
    ) -> Result<Option<Values>, Error> {
        match self.read(reference, name, arguments).await {
            Ok(values) => Ok(Some(values)),
            Err(error) if is_missing(&error) => Ok(None),
            Err(error) => Err(error),
        }
    }

    /// Reads a list the device gives one entry per call: the number of
    /// entries from the output `count` of the action `counter`, then the
    /// entries from `getter` with each index as its input `index`, up to
    /// `max`. Stops early at the first index the device has no entry for.
    /// Returns the number the device reported and the entries, or `None`
    /// like [`try_read`](Self::try_read) if it has no such list.
    ///
    /// # Errors
    ///
    /// The other errors of [`read`](Self::read).
    pub async fn entries(
        &mut self,
        reference: &str,
        (counter, count): (&str, &str),
        (getter, index): (&str, &str),
        max: u32,
    ) -> Result<Option<(u32, Vec<Values>)>, Error> {
        let Some(values) = self.try_read(reference, counter, &[]).await? else {
            return Ok(None);
        };
        let reported: u32 = values.number(count).unwrap_or_default();
        let mut entries = Vec::new();
        for i in 0..reported.min(max) {
            let i = i.to_string();
            let Some(entry) = self.try_read(reference, getter, &[(index, &i)]).await? else {
                break;
            };
            entries.push(entry);
        }
        Ok(Some((reported, entries)))
    }

    /// The device description.
    pub fn description(&self) -> &DeviceDescription {
        &self.description
    }

    /// A service and its description, which is read once.
    ///
    /// # Errors
    ///
    /// The errors of finding the service and of reading its description.
    pub async fn service(&mut self, reference: &str) -> Result<(Service, &Scpd), Error> {
        let service = self.description.service(reference)?.clone();
        if !self.scpds.contains_key(&service.service_id) {
            let scpd = self.client.scpd(&service).await?;
            self.scpds.insert(service.service_id.clone(), scpd);
        }
        let scpd = &self.scpds[&service.service_id];
        Ok((service, scpd))
    }

    /// The short ids of the services whose name is `name`, like
    /// `WLANConfiguration1` and `WLANConfiguration2`, in order.
    pub fn services(&self, name: &str) -> Vec<String> {
        self.description
            .services()
            .filter(|s| s.name() == name)
            .map(|s| crate::capture::short_id(s).to_owned())
            .collect()
    }
}

/// Whether the error says that the device lacks something, or keeps it from
/// this user, rather than that it could not be asked. Blocked and busy
/// confirmations are failures: the device holds back the next calls too.
pub(crate) fn is_missing(error: &Error) -> bool {
    match error {
        Error::UnknownAction { .. } => true,
        Error::Fault { fault, .. } => !fault.is_temporary(),
        Error::Tr064(error) => {
            error.upnp_code().is_some() || matches!(error, tr064::Error::UnknownService(_))
        }
        _ => false,
    }
}

/// The output arguments of a call.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Values(pub Vec<(String, String)>);

impl Values {
    /// The value of an argument.
    pub fn get(&self, name: &str) -> Option<&str> {
        self.0
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }

    /// The value of an argument, unless it is empty.
    pub fn text(&self, name: &str) -> Option<String> {
        self.get(name).filter(|v| !v.is_empty()).map(str::to_owned)
    }

    /// The value of an argument as a number, if it is one.
    pub fn number<T: FromStr>(&self, name: &str) -> Option<T> {
        self.get(name).and_then(|v| v.trim().parse().ok())
    }

    /// The value of a boolean argument: `1` or `0`.
    pub fn flag(&self, name: &str) -> Option<bool> {
        match self.get(name)? {
            "1" | "true" => Some(true),
            "0" | "false" => Some(false),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_values() {
        let values = Values(vec![
            ("NewName".to_owned(), "box".to_owned()),
            ("NewEmpty".to_owned(), String::new()),
            ("NewCount".to_owned(), " 42".to_owned()),
            ("NewOn".to_owned(), "1".to_owned()),
            ("NewOff".to_owned(), "false".to_owned()),
        ]);
        assert_eq!(values.get("NewName"), Some("box"));
        assert_eq!(values.get("NewOther"), None);
        assert_eq!(values.text("NewName").as_deref(), Some("box"));
        assert_eq!(values.text("NewEmpty"), None);
        assert_eq!(values.number::<u32>("NewCount"), Some(42));
        assert_eq!(values.number::<u32>("NewName"), None);
        assert_eq!(values.flag("NewOn"), Some(true));
        assert_eq!(values.flag("NewOff"), Some(false));
        assert_eq!(values.flag("NewName"), None);
        assert_eq!(values.flag("NewOther"), None);
    }

    #[test]
    fn tells_missing_things_from_failures() {
        let unknown = Error::UnknownAction {
            service: "Hosts1".to_owned(),
            action: "Nothing".to_owned(),
        };
        assert!(is_missing(&unknown));
        let service = Error::Tr064(tr064::Error::UnknownService("Nothing1".to_owned()));
        assert!(is_missing(&service));
        assert!(!is_missing(&Error::Tr064(
            tr064::Error::AuthenticationFailed
        )));
        for (code, missing) in [
            (606, true),
            (713, true),
            (866, true),
            (867, false),
            (868, false),
        ] {
            let error = Error::from(tr064::Error::Upnp {
                code,
                description: String::new(),
            });
            assert_eq!(is_missing(&error), missing, "{code}");
        }
        let refused = Error::MissingArgument {
            action: "GetGenericHostEntry".to_owned(),
            argument: "NewIndex".to_owned(),
        };
        assert!(!is_missing(&refused));
    }
}
