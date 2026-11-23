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
//! A plan shows only the inputs of the action. A device may change other
//! settings along with them, like the options of a feature it turns off, so
//! [apply](Changer::apply) reads the getters of the service before and after
//! the change and reports the other values that changed as
//! [side effects](SideEffect).
//!
//! Every plan, change and refusal is logged in `audit.jsonl` in the state
//! directory, with secrets hidden. A change is logged before it is sent;
//! if that fails, it is not sent.
//!
//! [`Changer::plan_reboot`] and [`Changer::reboot`] restart the device the
//! same way, see [`reboot`].

mod audit;
pub mod reboot;
mod tokens;

use std::collections::{BTreeMap, BTreeSet, HashMap};
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
/// What a [`Plan`] leaves out.
pub const PLAN_NOTE: &str = "shows only the inputs of the action; the device may \
change other settings along with them, which apply reports as side effects";
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
    /// Another device answers after a restart.
    #[error("another device answers after the restart: {0}")]
    OtherDevice(Identity),
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

/// The usual state directory, like `~/.local/state/fritz` on Linux and
/// macOS, see [`dirs`](crate::dirs). `None` if the home directory is
/// unknown.
pub fn default_dir() -> Option<PathBuf> {
    crate::dirs::state()
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
    /// What the plan leaves out, see [`PLAN_NOTE`].
    pub note: &'static str,
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

/// A value of the service that changed along with a change, although the
/// change did not set it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SideEffect {
    /// The getter that returns it, like `GetInfo`.
    pub getter: String,
    /// The output argument, like `NewChannel`.
    pub argument: String,
    /// The value before, if the getter returned it.
    pub before: Option<String>,
    /// The value after, if the getter still returns it.
    pub after: Option<String>,
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
    /// Other values of the service that changed. Values that change on
    /// their own between two reads before the change, like counters, are
    /// left out; slower ones may still show up.
    pub side_effects: Vec<SideEffect>,
}

/// What the getters of a service return: the values by getter and output
/// argument, as they are, and the getters that failed. Never leaves the
/// crate; side effects show the values [redacted](redact::value).
#[derive(Debug, Default)]
struct Snapshot {
    values: BTreeMap<(String, String), String>,
    failed: BTreeSet<String>,
}

/// Plans and applies changes to a device.
#[derive(Debug)]
pub struct Changer {
    client: Client,
    settings: Settings,
    tokens: Tokens,
    audit: Audit,
    /// The identity of the device and its UDN, once read.
    identity: Option<(Identity, String)>,
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
        if let Some((identity, _)) = &self.identity {
            return Ok(identity.clone());
        }
        let description = self.client.description().await?;
        let udn = description
            .device
            .udn
            .filter(|u| !u.is_empty())
            .ok_or(Error::Unidentified("the device description has no UDN"))?;
        let serial = reboot::Probe::new(&self.client)
            .await?
            .read(&self.client)
            .await?
            .serial
            .filter(|s| !s.is_empty())
            .ok_or(Error::Unidentified("the device has no serial number"))?;
        let identity = Identity::of(&udn, &serial);
        self.identity = Some((identity.clone(), udn));
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
            note: PLAN_NOTE,
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
        let refused = subject(reference, name, arguments);
        let checked = self.checked(refused).await?;
        // Read twice, to tell the values that change on their own.
        let (before, again) = match self.snapshots(&checked, arguments).await {
            Ok(snapshots) => snapshots,
            Err(error) => return Err(self.refuse(refused, error)),
        };
        let service = self.send(&checked, refused, token).await?;
        let sent = subject(&service, name, arguments);
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
        let side_effects =
            side_effects(&before, &again, &self.snapshot(&checked, arguments).await?);
        // Logged as sending already, so a failed log loses nothing.
        let _ = self.log(Event::Applied, sent, None, verified);
        Ok(Outcome {
            device: checked.identity,
            service,
            action: checked.action.name,
            checks,
            verified,
            side_effects,
        })
    }

    /// [Checks](Self::check) a change, and logs it as refused if it does
    /// not pass.
    async fn checked(&mut self, change: Subject<'_>) -> Result<Checked, Error> {
        match self
            .check(change.service, change.action, change.arguments)
            .await
        {
            Ok(checked) => Ok(checked),
            Err(error) => Err(self.refuse(change, error)),
        }
    }

    /// Redeems the token of a checked change, logs the change and sends
    /// it. Returns the short ID of the service, like `DeviceConfig1`.
    async fn send(
        &mut self,
        checked: &Checked,
        change: Subject<'_>,
        token: &str,
    ) -> Result<String, Error> {
        let arguments = change.arguments;
        if let Err(error) = self
            .tokens
            .redeem(token, &fingerprint(checked, arguments), now())
        {
            return Err(self.refuse(change, error));
        }
        let service = short_id(&checked.service).to_owned();
        let sent = subject(&service, change.action, arguments);
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
        Ok(service)
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
    /// it only reads, returns one of these variables, all its inputs refer
    /// to them as well, and it takes every one it does not return, like an
    /// index or an address: otherwise it may read another entry.
    async fn read_back(
        &self,
        checked: &Checked,
        arguments: &[(String, String)],
    ) -> Result<HashMap<String, String>, Error> {
        let inputs = inputs(checked, arguments);
        let mut values = HashMap::new();
        for getter in &checked.scpd.actions {
            if policy::classify(&checked.service, getter) != Class::Read
                || !getter
                    .outputs()
                    .any(|o| inputs.contains_key(o.related_state_variable.as_str()))
            {
                continue;
            }
            let scoped = inputs.keys().all(|variable| {
                getter
                    .arguments
                    .iter()
                    .any(|a| a.related_state_variable == *variable)
            });
            if !scoped {
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

    /// Two snapshots of the service, one right after the other.
    async fn snapshots(
        &self,
        checked: &Checked,
        arguments: &[(String, String)],
    ) -> Result<(Snapshot, Snapshot), Error> {
        let first = self.snapshot(checked, arguments).await?;
        let second = self.snapshot(checked, arguments).await?;
        Ok((first, second))
    }

    /// The values the getters of the service return, except those of the
    /// state variables the inputs of the change refer to. A getter is
    /// called if it only reads and the inputs of the change give all of its
    /// own, like an index; it may return other entries, but the same ones
    /// before and after. Personal values like the event log are left out.
    async fn snapshot(
        &self,
        checked: &Checked,
        arguments: &[(String, String)],
    ) -> Result<Snapshot, Error> {
        let inputs = inputs(checked, arguments);
        let mut snapshot = Snapshot::default();
        for getter in &checked.scpd.actions {
            if policy::classify(&checked.service, getter) != Class::Read {
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
            // Unredacted, so that a changed secret shows up as well.
            let output = match generic::send(
                &self.client,
                &checked.service,
                getter,
                &getter_arguments,
            )
            .await
            {
                Ok(output) => output,
                Err(error) if is_answer(&error) => {
                    snapshot.failed.insert(getter.name.clone());
                    continue;
                }
                Err(error) => return Err(error.into()),
            };
            for (name, value) in output {
                let planned = getter
                    .outputs()
                    .find(|o| o.name == name)
                    .is_none_or(|o| inputs.contains_key(o.related_state_variable.as_str()));
                if !(planned || policy::is_personal_output(&name)) {
                    snapshot.values.insert((getter.name.clone(), name), value);
                }
            }
        }
        Ok(snapshot)
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
            device: self.identity.as_ref().map(|(identity, _)| identity),
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

/// The values of the inputs of a change, by the state variable they refer
/// to.
fn inputs<'a>(
    checked: &'a Checked,
    arguments: &'a [(String, String)],
) -> HashMap<&'a str, &'a str> {
    checked
        .action
        .inputs()
        .filter_map(|a| {
            Some((
                a.related_state_variable.as_str(),
                value(arguments, &a.name)?,
            ))
        })
        .collect()
}

/// The values that differ between `before` and `after`, except those that
/// already differ between `before` and `again`, read right after it, and
/// those of getters that failed in any of them.
fn side_effects(before: &Snapshot, again: &Snapshot, after: &Snapshot) -> Vec<SideEffect> {
    let snapshots = [before, again, after];
    let keys: BTreeSet<&(String, String)> =
        before.values.keys().chain(after.values.keys()).collect();
    keys.into_iter()
        .filter(|(getter, _)| snapshots.iter().all(|s| !s.failed.contains(getter)))
        .filter(|key| {
            let [before, again, after] = snapshots.map(|s| s.values.get(*key));
            before != after && before == again
        })
        .map(|key| {
            let (getter, argument) = key;
            let shown = |snapshot: &Snapshot| {
                let value = snapshot.values.get(key)?;
                Some(redact::value(argument, value))
            };
            SideEffect {
                getter: getter.clone(),
                argument: argument.clone(),
                before: shown(before),
                after: shown(after),
            }
        })
        .collect()
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
    fn finds_side_effects() {
        let snapshot = |values: &[(&str, &str, &str)]| Snapshot {
            values: values
                .iter()
                .map(|(g, n, v)| (((*g).to_owned(), (*n).to_owned()), (*v).to_owned()))
                .collect(),
            failed: BTreeSet::new(),
        };
        let before = snapshot(&[
            ("GetInfo", "NewChannel", "6"),
            ("GetInfo", "NewCount", "1"),
            ("GetInfo", "NewKeep", "x"),
            ("GetInfo", "NewGone", "y"),
            ("GetInfo", "NewKey", "old"),
            ("GetStatus", "NewStatus", "Up"),
        ]);
        let again = snapshot(&[
            ("GetInfo", "NewChannel", "6"),
            ("GetInfo", "NewCount", "2"),
            ("GetInfo", "NewKeep", "x"),
            ("GetInfo", "NewGone", "y"),
            ("GetInfo", "NewKey", "old"),
            ("GetStatus", "NewStatus", "Up"),
        ]);
        let mut after = snapshot(&[
            ("GetInfo", "NewChannel", "1"),
            ("GetInfo", "NewCount", "3"),
            ("GetInfo", "NewKeep", "x"),
            ("GetInfo", "NewPassword", "secret"),
            ("GetInfo", "NewKey", "new"),
        ]);
        // A getter that fails afterwards changed nothing.
        after.failed.insert("GetStatus".to_owned());
        let effects = side_effects(&before, &again, &after);
        let effect = |argument: &str, before: Option<&str>, after: Option<&str>| SideEffect {
            getter: "GetInfo".to_owned(),
            argument: argument.to_owned(),
            before: before.map(str::to_owned),
            after: after.map(str::to_owned),
        };
        assert_eq!(
            effects,
            [
                effect("NewChannel", Some("6"), Some("1")),
                effect("NewGone", Some("y"), None),
                effect("NewKey", Some(redact::HIDDEN), Some(redact::HIDDEN)),
                effect("NewPassword", None, Some(redact::HIDDEN)),
            ]
        );
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
