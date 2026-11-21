//! Calling the planned getters and storing their answers.

use std::fmt;
use std::fs;
use std::io;
use std::path::Path;
use std::time::Duration;

use serde_json::{Map, Value, json};
use tr064::{Action, Client, Error, Service};

use super::{Decision, Plan, shape, short_id};
use crate::fault::Fault;
use crate::redact;

/// The getters that return the path of a list document: the action, the
/// output with the path, and the file the document is stored in.
const LISTS: [(&str, &str, &str); 2] = [
    (
        "X_AVM-DE_GetHostListPath",
        "NewX_AVM-DE_HostListPath",
        "devicehostlist.xml",
    ),
    (
        "X_AVM-DE_GetMeshListPath",
        "NewX_AVM-DE_MeshListPath",
        "meshlist.json",
    ),
];

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

/// A list document, fetched at the path a getter returned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct List {
    /// The file name, like `devicehostlist.xml`.
    pub name: &'static str,
    /// The document, or why it could not be fetched.
    pub outcome: Result<String, String>,
}

/// The answers of a capture run.
#[derive(Debug, Default)]
pub struct Capture {
    pub responses: Vec<Response>,
    pub lists: Vec<List>,
    /// The error that ended the run early, if any.
    pub stopped: Option<Error>,
}

/// Calls the getters the plan decided to call, one after the other, and
/// reports each answer to `progress`. An indexed getter is called with
/// rising indexes until it fails or [`Options::max_entries`] is reached.
/// The host and mesh lists are fetched at the paths their getters return.
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
        let mut list = None;
        let outcome = match self.client.call(service, &action.name, &arguments).await {
            Ok(out) => {
                list = list_path(&action.name, |n| out.get(n));
                Outcome::Ok(
                    out.into_iter()
                        .map(|(n, v)| {
                            let v = redact::value(&n, &v);
                            (n, v)
                        })
                        .collect(),
                )
            }
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
        if let Some((name, path)) = list {
            self.fetch(name, &path).await?;
        }
        Ok(ok)
    }

    /// Fetches a list document and records it, or the error that ends the
    /// run.
    async fn fetch(&mut self, name: &'static str, path: &str) -> Result<(), Error> {
        tokio::time::sleep(self.options.delay).await;
        let outcome = match self.client.get(path).await.map_err(redact::error) {
            Ok(document) => Ok(document),
            Err(error) if stops(&error) => return Err(error),
            Err(error) => Err(error.to_string()),
        };
        self.capture.lists.push(List { name, outcome });
        Ok(())
    }
}

/// The file name and path of the list document whose path the action
/// returned, if it is one of the [lists](LISTS). `out` gives the value of
/// an output.
fn list_path<'a>(
    action: &str,
    out: impl Fn(&str) -> Option<&'a str>,
) -> Option<(&'static str, String)> {
    let (_, output, name) = LISTS.iter().find(|(a, ..)| *a == action)?;
    let path = out(output).filter(|p| !p.is_empty())?;
    Some((name, path.to_owned()))
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
    ) || Fault::of(error) == Some(Fault::Blocked)
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
    /// A summary like `12 calls: 9 ok, 2 faults, 1 failed, 2 of 2 lists`,
    /// without the lists if there were none.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (ok, faults, failed) = self.counts();
        let fetched = self.lists.iter().filter(|l| l.outcome.is_ok()).count();
        let lists = match self.lists.len() {
            0 => String::new(),
            n => format!(", {fetched} of {n} lists"),
        };
        write!(
            f,
            "{} calls: {ok} ok, {faults} faults, {failed} failed{lists}",
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
/// lists/<name>
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
    for list in &capture.lists {
        if let Ok(document) = &list.outcome {
            let lists_dir = dir.join("lists");
            fs::create_dir_all(&lists_dir)?;
            fs::write(lists_dir.join(list.name), document)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_paths_of_lists() {
        let out = |path: &'static str| {
            move |name: &str| (name == "NewX_AVM-DE_HostListPath").then_some(path)
        };
        assert_eq!(
            list_path("X_AVM-DE_GetHostListPath", out("/devicehostlist.lua?sid=1")),
            Some(("devicehostlist.xml", "/devicehostlist.lua?sid=1".to_owned()))
        );
        assert_eq!(list_path("X_AVM-DE_GetHostListPath", out("")), None);
        assert_eq!(list_path("X_AVM-DE_GetMeshListPath", out("/x")), None);
        assert_eq!(list_path("GetInfo", out("/x")), None);
    }

    #[test]
    fn stops_only_at_errors_that_recur() {
        let locked = Error::Upnp {
            code: 867,
            description: "second factor authentication blocked".to_owned(),
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
