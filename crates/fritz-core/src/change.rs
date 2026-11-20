//! Changes to a device, in two steps: a [plan](Changer::plan) shows what an
//! action would change and issues a token, and [apply](Changer::apply)
//! makes the change with that token and reads the values back.
//!
//! Only builds with the feature `write` have this module. A change has to
//! pass every one of these checks, before anything is sent to the control
//! URL of the action:
//!
//! 1. The action is a change: a [write](Class::Write), allowed by
//!    `FRITZ_ALLOW_WRITE=1`, or a [critical](Class::Critical) one, which
//!    needs `FRITZ_ALLOW_CRITICAL=1` as well. Getters are never planned.
//! 2. The arguments are exactly the inputs of the action.
//! 3. The [identity](crate::identity) of the device is on the allow list,
//!    and not on the deny list.
//! 4. To apply a change, the token of its plan: issued less than five
//!    minutes ago for the same device, action and arguments, and not used
//!    before.
//!
//! Every plan, change and refusal is logged in `audit.jsonl` in the state
//! directory, with secrets hidden. A change is logged before it is sent;
//! if that fails, it is not sent.

mod audit;
mod tokens;

use std::collections::HashMap;
use std::io;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Serialize;
use tr064::{Action, Client, Scpd, Service};

use crate::capture::short_id;
use crate::generic;
use crate::identity::{Identity, Targets, Verdict};
use crate::names::{Sensitivity, sensitivity};
use crate::policy::{self, Class};
use crate::redact;
use audit::{Audit, Entry, Event};
use tokens::Tokens;

/// The environment variable that allows changes.
pub const ENV_ALLOW_WRITE: &str = "FRITZ_ALLOW_WRITE";
/// The environment variable that allows critical changes, together with
/// [`ENV_ALLOW_WRITE`].
pub const ENV_ALLOW_CRITICAL: &str = "FRITZ_ALLOW_CRITICAL";
/// How long the token of a plan is valid.
pub const TTL: Duration = Duration::from_mins(5);

/// Why a change was not planned or applied.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Call(#[from] generic::Error),
    /// The action only reads.
    #[error("{action} is {class}, not a change")]
    NotAChange { action: String, class: Class },
    /// The action is a change the environment does not allow.
    #[error("{action} is {class}; it needs {needs}")]
    NotPermitted {
        action: String,
        class: Class,
        needs: &'static str,
    },
    /// The device is on the deny list.
    #[error("the device {0} is on the deny list for changes")]
    DeniedDevice(Identity),
    /// The device is not on the allow list.
    #[error("the device {0} is not on the allow list for changes")]
    UnknownDevice(Identity),
    /// The device does not tell who it is.
    #[error("cannot identify the device: {0}")]
    Unidentified(&'static str),
    /// The token does not fit the change.
    #[error("the token is unknown, used, expired or for another change")]
    InvalidToken,
    /// The tokens or the audit log cannot be read or written.
    #[error("cannot use {}", path.display())]
    State { path: PathBuf, source: io::Error },
}

impl From<tr064::Error> for Error {
    fn from(error: tr064::Error) -> Self {
        Self::Call(error.into())
    }
}

/// The kinds of changes the environment allows.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Permit {
    /// Writes, see [`ENV_ALLOW_WRITE`].
    pub write: bool,
    /// Critical changes as well, see [`ENV_ALLOW_CRITICAL`].
    pub critical: bool,
}

impl Permit {
    /// The changes the environment of the process allows.
    pub fn from_env() -> Self {
        Self::from_vars(|name| std::env::var(name).ok())
    }

    /// The changes the variables allow: `1`, `true` or `yes` turns them on.
    pub fn from_vars(var: impl Fn(&str) -> Option<String>) -> Self {
        let on = |name| {
            var(name)
                .is_some_and(|v| matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes"))
        };
        Self {
            write: on(ENV_ALLOW_WRITE),
            critical: on(ENV_ALLOW_CRITICAL),
        }
    }

    /// Whether `class` is a change this allows.
    fn check(self, action: &str, class: &Class) -> Result<(), Error> {
        let needs = match class {
            Class::Write if self.write => return Ok(()),
            Class::Critical { .. } if self.write && self.critical => return Ok(()),
            Class::Write => "FRITZ_ALLOW_WRITE=1",
            Class::Critical { .. } => "FRITZ_ALLOW_WRITE=1 and FRITZ_ALLOW_CRITICAL=1",
            Class::Read | Class::Personal | Class::Secret { .. } => {
                return Err(Error::NotAChange {
                    action: action.to_owned(),
                    class: class.clone(),
                });
            }
        };
        Err(Error::NotPermitted {
            action: action.to_owned(),
            class: class.clone(),
            needs,
        })
    }
}

/// What a [`Changer`] allows, and where it keeps its state.
#[derive(Debug, Clone)]
pub struct Settings {
    pub permit: Permit,
    pub targets: Targets,
    /// The directory of the tokens and the audit log, see [`default_dir`].
    pub dir: PathBuf,
    /// Where changes come from, for the audit log, like `cli`.
    pub source: &'static str,
}

/// The usual state directory, like `~/.local/state/fritz` on Linux, or the
/// local data directory where there is none. `None` if the home directory
/// is unknown.
pub fn default_dir() -> Option<PathBuf> {
    directories::ProjectDirs::from("", "", "fritz")
        .map(|dirs| dirs.state_dir().unwrap_or(dirs.data_local_dir()).to_owned())
}

/// The change of an input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Change {
    pub argument: String,
    /// The value the device returns now, if a getter returns it.
    pub before: Option<String>,
    pub after: String,
}

/// A planned change.
#[derive(Debug, Clone, Serialize)]
pub struct Plan {
    pub device: Identity,
    /// The service, like `WLANConfiguration1`.
    pub service: String,
    pub action: String,
    /// The class of the action, like `write`.
    pub class: String,
    pub changes: Vec<Change>,
    /// The token to [apply](Changer::apply) the change with.
    pub token: String,
    /// Seconds the token is valid.
    pub expires_in: u64,
}

/// The value of an input the device returns after a change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Check {
    pub argument: String,
    pub expected: String,
    /// The value a getter returns, if one returns it.
    pub actual: Option<String>,
}

/// An applied change.
#[derive(Debug, Clone, Serialize)]
pub struct Outcome {
    pub device: Identity,
    pub service: String,
    pub action: String,
    pub checks: Vec<Check>,
    /// Whether the device returns the values it was given: `None` if no
    /// getter returns any of them.
    pub verified: Option<bool>,
}

/// Plans and applies changes to a device.
#[derive(Debug)]
pub struct Changer {
    client: Client,
    settings: Settings,
    tokens: Tokens,
    audit: Audit,
    identity: Option<Identity>,
}

/// A change that passed the checks.
struct Checked {
    service: Service,
    scpd: Scpd,
    action: Action,
    class: Class,
    identity: Identity,
}

impl Changer {
    /// Changes the device of `client`, which needs credentials.
    pub fn new(client: Client, settings: Settings) -> Self {
        let tokens = Tokens::new(settings.dir.join("tokens.json"));
        let audit = Audit::new(settings.dir.join("audit.jsonl"));
        Self {
            client,
            settings,
            tokens,
            audit,
            identity: None,
        }
    }

    /// The identity of the device, from its description and
    /// `DeviceInfo:GetInfo`.
    ///
    /// # Errors
    ///
    /// [`Error::Unidentified`] if the device has no UDN or serial number,
    /// and the errors of reading them.
    pub async fn identity(&mut self) -> Result<Identity, Error> {
        if let Some(identity) = &self.identity {
            return Ok(identity.clone());
        }
        let description = self.client.description().await?;
        let udn = description
            .device
            .udn
            .filter(|u| !u.is_empty())
            .ok_or(Error::Unidentified("the device description has no UDN"))?;
        let (service, scpd) = generic::service(&self.client, "DeviceInfo1").await?;
        let action = generic::action(&service, &scpd, "GetInfo")?;
        let serial = generic::send(&self.client, &service, action, &[])
            .await?
            .into_iter()
            .find(|(name, value)| name == "NewSerialNumber" && !value.is_empty())
            .map(|(_, value)| value)
            .ok_or(Error::Unidentified("the device has no serial number"))?;
        let identity = Identity::of(&udn, &serial);
        self.identity = Some(identity.clone());
        Ok(identity)
    }

    /// Plans a change: checks it, reads the current values of its inputs
    /// and issues a token for it.
    ///
    /// # Errors
    ///
    /// The errors of the checks, of reading the values, and
    /// [`Error::State`] if the token cannot be saved or the plan not logged.
    pub async fn plan(
        &mut self,
        reference: &str,
        name: &str,
        arguments: &[(String, String)],
    ) -> Result<Plan, Error> {
        let plan = match self.try_plan(reference, name, arguments).await {
            Ok(plan) => plan,
            Err(error) => return Err(self.refuse(subject(reference, name, arguments), error)),
        };
        let planned = subject(&plan.service, name, arguments);
        self.log(Event::Planned, planned, None, None)?;
        Ok(plan)
    }

    async fn try_plan(
        &mut self,
        reference: &str,
        name: &str,
        arguments: &[(String, String)],
    ) -> Result<Plan, Error> {
        let checked = self.check(reference, name, arguments).await?;
        let before = self.read_back(&checked, arguments).await?;
        let changes = checked
            .action
            .inputs()
            .filter_map(|input| {
                let value = value(arguments, &input.name)?;
                Some(Change {
                    argument: input.name.clone(),
                    before: before
                        .get(&input.related_state_variable)
                        .map(|v| redact::value(&input.name, v)),
                    after: redact::value(&input.name, value),
                })
            })
            .collect();
        let now = now();
        let token =
            self.tokens
                .issue(&fingerprint(&checked, arguments), now, now + TTL.as_secs())?;
        Ok(Plan {
            device: checked.identity,
            service: short_id(&checked.service).to_owned(),
            action: checked.action.name,
            class: checked.class.to_string(),
            changes,
            token,
            expires_in: TTL.as_secs(),
        })
    }

    /// Applies a planned change with the token of its plan, and reads the
    /// values back.
    ///
    /// # Errors
    ///
    /// The errors of the checks, [`Error::InvalidToken`], [`Error::State`]
    /// if the change cannot be logged, and the errors of the call.
    pub async fn apply(
        &mut self,
        reference: &str,
        name: &str,
        arguments: &[(String, String)],
        token: &str,
    ) -> Result<Outcome, Error> {
        let checked = match self.check(reference, name, arguments).await {
            Ok(checked) => checked,
            Err(error) => return Err(self.refuse(subject(reference, name, arguments), error)),
        };
        if let Err(error) = self
            .tokens
            .redeem(token, &fingerprint(&checked, arguments), now())
        {
            return Err(self.refuse(subject(reference, name, arguments), error));
        }
        let service = short_id(&checked.service).to_owned();
        let sent = subject(&service, name, arguments);
        self.log(Event::Sending, sent, None, None)?;
        let inputs: Vec<(&str, &str)> = arguments
            .iter()
            .map(|(n, v)| (n.as_str(), v.as_str()))
            .collect();
        if let Err(error) = self
            .client
            .call(&checked.service, &checked.action.name, &inputs)
            .await
        {
            let error = Error::from(error);
            // Logged as sending already, so a failed log loses nothing.
            let _ = self.log(Event::Failed, sent, Some(&error), None);
            return Err(error);
        }
        let after = self.read_back(&checked, arguments).await?;
        let checks: Vec<Check> = checked
            .action
            .inputs()
            .filter_map(|input| {
                let value = value(arguments, &input.name)?;
                Some(Check {
                    argument: input.name.clone(),
                    expected: redact::value(&input.name, value),
                    actual: after
                        .get(&input.related_state_variable)
                        .map(|v| redact::value(&input.name, v)),
                })
            })
            .collect();
        let verified = verify(&checked, arguments, &after);
        // Logged as sending already, so a failed log loses nothing.
        let _ = self.log(Event::Applied, sent, None, verified);
        Ok(Outcome {
            device: checked.identity,
            service,
            action: checked.action.name,
            checks,
            verified,
        })
    }

    /// Checks a change: the class of the action, the arguments and the
    /// device.
    async fn check(
        &mut self,
        reference: &str,
        name: &str,
        arguments: &[(String, String)],
    ) -> Result<Checked, Error> {
        let (service, scpd) = generic::service(&self.client, reference).await?;
        let action = generic::action(&service, &scpd, name)?.clone();
        let class = policy::classify(&service, &action);
        self.settings.permit.check(&action.name, &class)?;
        generic::check_arguments(&action, arguments)?;
        let identity = self.identity().await?;
        match self.settings.targets.verdict(&identity) {
            Verdict::Allowed => Ok(Checked {
                service,
                scpd,
                action,
                class,
                identity,
            }),
            Verdict::Denied => Err(Error::DeniedDevice(identity)),
            Verdict::Unknown => Err(Error::UnknownDevice(identity)),
        }
    }

    /// The values of the state variables the inputs of the change refer
    /// to, as the getters of the service return them. A getter is called if
    /// it only reads, returns one of these variables, and all its inputs,
    /// like an index, refer to them as well.
    async fn read_back(
        &self,
        checked: &Checked,
        arguments: &[(String, String)],
    ) -> Result<HashMap<String, String>, Error> {
        let inputs: HashMap<&str, &str> = checked
            .action
            .inputs()
            .filter_map(|a| {
                Some((
                    a.related_state_variable.as_str(),
                    value(arguments, &a.name)?,
                ))
            })
            .collect();
        let mut values = HashMap::new();
        for getter in &checked.scpd.actions {
            if policy::classify(&checked.service, getter) != Class::Read
                || !getter
                    .outputs()
                    .any(|o| inputs.contains_key(o.related_state_variable.as_str()))
            {
                continue;
            }
            let Some(getter_arguments) = getter
                .inputs()
                .map(|i| {
                    let value = inputs.get(i.related_state_variable.as_str())?;
                    Some((i.name.clone(), (*value).to_owned()))
                })
                .collect::<Option<Vec<_>>>()
            else {
                continue;
            };
            let output = match generic::call(
                &self.client,
                &checked.service,
                getter,
                &getter_arguments,
            )
            .await
            {
                Ok(output) => output,
                Err(error) if is_answer(&error) => continue,
                Err(error) => return Err(error.into()),
            };
            for (name, value) in output {
                if let Some(argument) = getter.outputs().find(|o| o.name == name)
                    && inputs.contains_key(argument.related_state_variable.as_str())
                {
                    values
                        .entry(argument.related_state_variable.clone())
                        .or_insert(value);
                }
            }
        }
        Ok(values)
    }

    /// Logs a refused change, and returns the reason.
    fn refuse(&self, subject: Subject<'_>, error: Error) -> Error {
        // The change is refused either way.
        let _ = self.log(Event::Refused, subject, Some(&error), None);
        error
    }

    fn log(
        &self,
        event: Event,
        subject: Subject<'_>,
        error: Option<&Error>,
        verified: Option<bool>,
    ) -> Result<(), Error> {
        let arguments: Vec<(String, String)> = subject
            .arguments
            .iter()
            .map(|(n, v)| (n.clone(), redact::value(n, v)))
            .collect();
        self.audit.append(&Entry {
            time: now(),
            source: self.settings.source,
            event,
            device: self.identity.as_ref(),
            service: subject.service,
            action: subject.action,
            arguments: &arguments,
            error: error.map(ToString::to_string),
            verified,
        })
    }
}

/// What a line of the audit log is about.
#[derive(Clone, Copy)]
struct Subject<'a> {
    service: &'a str,
    action: &'a str,
    arguments: &'a [(String, String)],
}

fn subject<'a>(
    service: &'a str,
    action: &'a str,
    arguments: &'a [(String, String)],
) -> Subject<'a> {
    Subject {
        service,
        action,
        arguments,
    }
}

/// The value of the argument `name`.
fn value<'a>(arguments: &'a [(String, String)], name: &str) -> Option<&'a str> {
    arguments
        .iter()
        .find(|(n, _)| n == name)
        .map(|(_, v)| v.as_str())
}

/// Whether the device answered, like with a missing right or entry, rather
/// than not at all.
fn is_answer(error: &generic::Error) -> bool {
    matches!(error, generic::Error::Fault { .. })
        || matches!(error, generic::Error::Tr064(error) if error.upnp_code().is_some())
}

/// A hash of what a token is for: the device, the action and the
/// arguments in any order.
fn fingerprint(checked: &Checked, arguments: &[(String, String)]) -> String {
    let mut arguments = arguments.to_vec();
    arguments.sort();
    let change = serde_json::json!([
        checked.identity.to_string(),
        checked.service.service_id,
        checked.action.name,
        arguments,
    ]);
    tokens::hash(&change.to_string())
}

/// Whether the device returns the values of the inputs it was given,
/// leaving out secrets. `None` if it returns none of them.
fn verify(
    checked: &Checked,
    arguments: &[(String, String)],
    after: &HashMap<String, String>,
) -> Option<bool> {
    let mut verified = None;
    let plain = checked
        .action
        .inputs()
        .filter(|i| sensitivity(&i.name) == Sensitivity::Plain);
    for input in plain {
        let variable = &input.related_state_variable;
        let actual = after.get(variable).filter(|a| *a != redact::HIDDEN);
        let (Some(expected), Some(actual)) = (value(arguments, &input.name), actual) else {
            continue;
        };
        let boolean = checked
            .scpd
            .state_variable(variable)
            .is_some_and(|v| v.data_type == "boolean");
        let same = same(boolean, expected, actual);
        verified = Some(verified.unwrap_or(true) && same);
    }
    verified
}

/// Whether two values are the same, with `1`, `true` and `yes` the same
/// for booleans.
fn same(boolean: bool, a: &str, b: &str) -> bool {
    let truth = |v: &str| match v.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" => Some(true),
        "0" | "false" | "no" => Some(false),
        _ => None,
    };
    match (boolean, truth(a), truth(b)) {
        (true, Some(a), Some(b)) => a == b,
        _ => a.trim() == b.trim(),
    }
}

/// Seconds since the Unix epoch.
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_permit_from_variables() {
        let permit = |vars: &[(&str, &str)]| {
            let vars: HashMap<String, String> = vars
                .iter()
                .map(|(n, v)| ((*n).to_owned(), (*v).to_owned()))
                .collect();
            Permit::from_vars(|name| vars.get(name).cloned())
        };
        assert_eq!(permit(&[]), Permit::default());
        assert_eq!(
            permit(&[(ENV_ALLOW_WRITE, "TRUE"), (ENV_ALLOW_CRITICAL, "0")]),
            Permit {
                write: true,
                critical: false
            }
        );
        assert_eq!(
            permit(&[(ENV_ALLOW_WRITE, "yes"), (ENV_ALLOW_CRITICAL, "1")]),
            Permit {
                write: true,
                critical: true
            }
        );
    }

    #[test]
    fn reads_the_permit_from_the_environment() {
        let var = |name: &str| std::env::var(name).ok();
        assert_eq!(Permit::from_env(), Permit::from_vars(var));
    }

    #[test]
    fn permits_classes() {
        let critical = Class::Critical {
            reason: "x".to_owned(),
        };
        let all = Permit {
            write: true,
            critical: true,
        };
        let write = Permit {
            write: true,
            critical: false,
        };
        let only_critical = Permit {
            write: false,
            critical: true,
        };
        assert!(all.check("A", &Class::Write).is_ok());
        assert!(all.check("A", &critical).is_ok());
        assert!(write.check("A", &Class::Write).is_ok());
        assert_eq!(
            write.check("A", &critical).unwrap_err().to_string(),
            "A is critical: x; it needs FRITZ_ALLOW_WRITE=1 and FRITZ_ALLOW_CRITICAL=1"
        );
        assert!(only_critical.check("A", &critical).is_err());
        assert_eq!(
            Permit::default()
                .check("A", &Class::Write)
                .unwrap_err()
                .to_string(),
            "A is write; it needs FRITZ_ALLOW_WRITE=1"
        );
        for class in [
            Class::Read,
            Class::Personal,
            Class::Secret {
                reason: "x".to_owned(),
            },
        ] {
            assert!(matches!(
                all.check("A", &class),
                Err(Error::NotAChange { .. })
            ));
        }
    }

    #[test]
    fn compares_values() {
        assert!(same(true, "1", "true"));
        assert!(same(true, "no", "0"));
        assert!(!same(true, "1", "0"));
        assert!(same(true, "x", " x"));
        assert!(!same(false, "1", "true"));
        assert!(same(false, "a ", "a"));
    }

    #[test]
    fn has_a_default_dir() {
        assert!(default_dir().unwrap().ends_with("fritz"));
    }
}
