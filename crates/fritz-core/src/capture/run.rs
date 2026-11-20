//! Calling the planned getters and storing their answers.

use std::fmt;
use std::fs;
use std::io;
use std::path::Path;
use std::time::Duration;

use serde_json::{Map, Value, json};
use tr064::{Action, Client, Error, Service};

use super::{Decision, Plan, looks_secret, shape, short_id};

/// The device locked the account after too many failed attempts.
const LOCKED: u16 = 867;

/// How a capture runs.
#[derive(Debug, Clone)]
pub struct Options {
    /// Also call the getters of personal lists.
    pub include_personal: bool,
    /// The most entries read of each indexed getter.
    pub max_entries: u32,
    /// The pause before each call, to keep the load on the device low.
    pub delay: Duration,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            include_personal: false,
            max_entries: 50,
            delay: Duration::from_millis(100),
        }
    }
}

/// The answer to one call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    /// The [short ID](super::short_id) of the service.
    pub service: String,
    pub action: String,
    /// The index for an indexed getter.
    pub index: Option<u32>,
    pub outcome: Outcome,
}

/// What a call returned. Values that could be secrets are already masked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// The output arguments.
    Ok(Vec<(String, String)>),
    /// A UPnP error, for example 606 when the user lacks the right.
    Fault { code: u16, description: String },
    /// Another error that only concerns this call.
    Failed(String),
}

/// The answers of a capture run.
#[derive(Debug, Default)]
pub struct Capture {
    pub responses: Vec<Response>,
    /// The error that ended the run early, if any.
    pub stopped: Option<Error>,
}

/// Calls the getters the plan decided to call, one after the other, and
/// reports each answer to `progress`. An indexed getter is called with
/// rising indexes until it fails or [`Options::max_entries`] is reached.
///
/// Errors that concern a single call are recorded. The run stops at errors
/// that would recur on every call or put the account at risk: missing or
/// rejected credentials, a locked account, a failed connection or an
/// unexpected certificate.
pub async fn run(
    client: &Client,
    plan: &Plan,
    options: &Options,
    mut progress: impl FnMut(&Response),
) -> Capture {
    let mut capture = Capture::default();
    let mut caller = Caller {
        client,
        options,
        capture: &mut capture,
        progress: &mut progress,
    };
    if let Err(error) = caller.all(plan).await {
        capture.stopped = Some(error);
    }
    capture
}

struct Caller<'a, P> {
    client: &'a Client,
    options: &'a Options,
    capture: &'a mut Capture,
    progress: &'a mut P,
}

impl<P: FnMut(&Response)> Caller<'_, P> {
    async fn all(&mut self, plan: &Plan) -> Result<(), Error> {
        for service in &plan.services {
            let Ok(scpd) = &service.scpd else { continue };
            for (action, decision) in &scpd.actions {
                self.action(&service.service, action, decision).await?;
            }
        }
        Ok(())
    }

    async fn action(
        &mut self,
        service: &Service,
        action: &Action,
        decision: &Decision,
    ) -> Result<(), Error> {
        let decision = match decision {
            Decision::Personal if self.options.include_personal => shape(action),
            other => other.clone(),
        };
        match decision {
            Decision::Call => {
                self.call(service, action, None).await?;
            }
            Decision::Iterate { index } => {
                for i in 0..self.options.max_entries {
                    if !self.call(service, action, Some((&index, i))).await? {
                        break;
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// Calls the action and records the answer. Whether it succeeded, or the
    /// error that ends the run.
    async fn call(
        &mut self,
        service: &Service,
        action: &Action,
        index: Option<(&str, u32)>,
    ) -> Result<bool, Error> {
        tokio::time::sleep(self.options.delay).await;
        let value = index.map(|(_, i)| i.to_string());
        let arguments: Vec<(&str, &str)> = index
            .iter()
            .zip(&value)
            .map(|((name, _), value)| (*name, value.as_str()))
            .collect();
        let outcome = match self.client.call(service, &action.name, &arguments).await {
            Ok(out) => Outcome::Ok(out.into_iter().map(|(n, v)| mask(n, &v)).collect()),
            Err(error) if stops(&error) => return Err(error),
            Err(Error::Upnp { code, description }) => Outcome::Fault { code, description },
            Err(error) => Outcome::Failed(error.to_string()),
        };
        let ok = matches!(outcome, Outcome::Ok(_));
        let response = Response {
            service: short_id(service).to_owned(),
            action: action.name.clone(),
            index: index.map(|(_, i)| i),
            outcome,
        };
        (self.progress)(&response);
        self.capture.responses.push(response);
        Ok(ok)
    }
}

/// Whether an error ends the run instead of only the call.
fn stops(error: &Error) -> bool {
    matches!(
        error,
        Error::CredentialsRequired
            | Error::AuthenticationFailed
            | Error::UnsupportedAuth(_)
            | Error::Http(_)
            | Error::CertificateMismatch { .. }
            | Error::Upnp { code: LOCKED, .. }
    )
}

/// Masks the value of an argument whose name points to a secret, and session
/// IDs in URLs like `/hostlist.lua?sid=…`.
fn mask(name: String, value: &str) -> (String, String) {
    if looks_secret(&name) {
        return (name, "***".to_owned());
    }
    let mut masked = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(position) = rest.find("sid=") {
        let start = position + "sid=".len();
        let parameter = rest[..position].ends_with(['?', '&']);
        masked.push_str(&rest[..start]);
        rest = &rest[start..];
        if parameter {
            masked.push_str("***");
            rest = &rest[rest.find(['&', '#']).unwrap_or(rest.len())..];
        }
    }
    masked.push_str(rest);
    (name, masked)
}

impl Capture {
    /// How many calls succeeded, ended with a UPnP error, or failed otherwise.
    pub fn counts(&self) -> (usize, usize, usize) {
        let mut counts = (0, 0, 0);
        for response in &self.responses {
            match response.outcome {
                Outcome::Ok(_) => counts.0 += 1,
                Outcome::Fault { .. } => counts.1 += 1,
                Outcome::Failed(_) => counts.2 += 1,
            }
        }
        counts
    }
}

impl fmt::Display for Capture {
    /// A summary like `12 calls: 9 ok, 2 faults, 1 failed`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (ok, faults, failed) = self.counts();
        write!(
            f,
            "{} calls: {ok} ok, {faults} faults, {failed} failed",
            self.responses.len()
        )
    }
}

impl Response {
    /// The file name of the answer, like `GetInfo.json` or
    /// `GetGenericHostEntry@3.json`.
    fn file_name(&self) -> String {
        match self.index {
            Some(index) => format!("{}@{index}.json", self.action),
            None => format!("{}.json", self.action),
        }
    }

    fn to_json(&self) -> Value {
        match &self.outcome {
            Outcome::Ok(out) => {
                let out: Map<String, Value> = out
                    .iter()
                    .map(|(n, v)| (n.clone(), Value::String(v.clone())))
                    .collect();
                json!({ "status": "ok", "out": out })
            }
            Outcome::Fault { code, description } => {
                json!({ "status": "fault", "code": code, "description": description })
            }
            Outcome::Failed(error) => json!({ "status": "failed", "error": error }),
        }
    }
}

impl fmt::Display for Response {
    /// A line like `Hosts1 GetGenericHostEntry@3: fault 713 SpecifiedArrayIndexInvalid`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}", self.service, self.action)?;
        if let Some(index) = self.index {
            write!(f, "@{index}")?;
        }
        match &self.outcome {
            Outcome::Ok(out) => write!(f, ": ok, {} values", out.len()),
            Outcome::Fault { code, description } => write!(f, ": fault {code} {description}"),
            Outcome::Failed(error) => write!(f, ": failed, {error}"),
        }
    }
}

/// Writes the documents of the plan and the answers of the capture into the
/// existing directory `dir`:
///
/// ```text
/// tr64desc.xml
/// scpd/<file>.xml
/// responses/<service>/<action>.json
/// responses/<service>/<action>@<index>.json
/// ```
///
/// # Errors
///
/// The errors of writing the files.
pub fn write(plan: &Plan, capture: &Capture, dir: &Path) -> io::Result<()> {
    fs::write(dir.join("tr64desc.xml"), &plan.description)?;
    let scpd_dir = dir.join("scpd");
    fs::create_dir_all(&scpd_dir)?;
    for service in &plan.services {
        if let Ok(scpd) = &service.scpd {
            let name = service
                .service
                .scpd_url
                .rsplit('/')
                .next()
                .unwrap_or_default();
            fs::write(scpd_dir.join(name), &scpd.xml)?;
        }
    }
    for response in &capture.responses {
        let service_dir = dir.join("responses").join(&response.service);
        fs::create_dir_all(&service_dir)?;
        let mut json = serde_json::to_string_pretty(&response.to_json())?;
        json.push('\n');
        fs::write(service_dir.join(response.file_name()), json)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn masked(name: &str, value: &str) -> String {
        mask(name.to_owned(), value).1
    }

    #[test]
    fn masks_secrets_and_session_ids() {
        assert_eq!(masked("NewX_AVM-DE_WPSClientPIN", "1234"), "***");
        assert_eq!(masked("NewSSID", "home"), "home");
        assert_eq!(
            masked("NewX_AVM-DE_HostListPath", "/hostlist.lua?sid=0123abcd"),
            "/hostlist.lua?sid=***"
        );
        assert_eq!(
            masked(
                "NewCallListURL",
                "http://box/calllist.lua?a=1&sid=ab&max=9#x"
            ),
            "http://box/calllist.lua?a=1&sid=***&max=9#x"
        );
        assert_eq!(masked("NewName", "outsid=1, sid=2"), "outsid=1, sid=2");
    }

    #[test]
    fn stops_only_at_errors_that_recur() {
        let locked = Error::Upnp {
            code: LOCKED,
            description: "locked".to_owned(),
        };
        let unauthorized = Error::Upnp {
            code: 606,
            description: "Action not authorized".to_owned(),
        };
        assert!(stops(&locked));
        assert!(stops(&Error::AuthenticationFailed));
        assert!(!stops(&unauthorized));
        assert!(!stops(&Error::InvalidResponse("x".to_owned())));
    }
}
