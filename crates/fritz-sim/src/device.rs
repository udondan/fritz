//! The simulated device: how it answers action calls, and the state that
//! writes change.
//!
//! Every argument of an action refers to a state variable of its service.
//! The state starts with the output arguments of the recorded getters.
//! Calls of other actions write their input arguments into the state.
//! Getters answer as recorded, except for the variables written since, and
//! answer from the state if they were not recorded. This makes a write
//! visible to the getters that read the same variable, without knowing
//! anything about the device.
//!
//! Recorded answers take precedence over the state because getters of the
//! same service can answer differently for the same variable: the device
//! might leave a value empty in one of them.

use std::collections::HashMap;

use tr064::{Action, Direction, Scpd, Service, StateVariable};

use crate::fixture::{Answer, Fixture, Recording};

/// A UPnP error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Fault {
    pub(crate) code: u16,
    pub(crate) description: String,
}

impl Fault {
    /// The error with the given code and its usual description.
    pub(crate) fn new(code: u16) -> Self {
        Self {
            code,
            description: description(code).to_owned(),
        }
    }
}

const INVALID_ACTION: u16 = 401;
const INVALID_ARGS: u16 = 402;
const INVALID_VALUE: u16 = 600;
const INVALID_INDEX: u16 = 713;

/// The usual description of a UPnP error code, as FRITZ!Box devices send
/// it.
fn description(code: u16) -> &'static str {
    match code {
        INVALID_ACTION => "Invalid Action",
        INVALID_ARGS => "Invalid Args",
        501 => "Action Failed",
        INVALID_VALUE => "Argument Value Invalid",
        601 => "Argument Value Out of Range",
        606 => "Action not authorized",
        INVALID_INDEX => "SpecifiedArrayIndexInvalid",
        714 => "NoSuchEntryInArray",
        820 => "Internal Error",
        866 => "second factor authentication required",
        867 => "second factor authentication blocked",
        868 => "second factor authentication busy",
        _ => "Unknown Error",
    }
}

/// What tests changed about the next call.
#[derive(Debug, Default)]
pub(crate) struct Hooks {
    /// The call fails with this error once its arguments are valid.
    pub(crate) fault: Option<Fault>,
    /// Writes succeed, but change nothing.
    pub(crate) discard_writes: bool,
    /// The state variables a write sets as well, with their values.
    pub(crate) side_effects: Vec<(String, String)>,
}

#[derive(Debug)]
pub(crate) struct Device {
    pub(crate) fixture: Fixture,
    /// The recorded values of the state variables by service, like
    /// `Hosts1`, and variable.
    recorded: HashMap<(String, String), String>,
    /// The values written since the start, by service and variable.
    written: HashMap<(String, String), String>,
}

impl Device {
    pub(crate) fn new(fixture: Fixture) -> Self {
        let mut recorded = HashMap::new();
        for service in fixture.description.services() {
            let Some(scpd) = fixture.scpd(service) else {
                continue;
            };
            let id = short_id(service);
            for action in &scpd.actions {
                let recording = fixture
                    .recordings
                    .get(&(id.to_owned(), action.name.clone()));
                let Some(Answer::Ok(out)) = recording.and_then(|r| r.once.as_ref()) else {
                    continue;
                };
                recorded.extend(outputs(action).filter_map(|argument| {
                    let value = out.get(&argument.name)?;
                    let key = (id.to_owned(), argument.related_state_variable.clone());
                    Some((key, value.clone()))
                }));
            }
        }
        Self {
            fixture,
            recorded,
            written: HashMap::new(),
        }
    }

    /// The value of a state variable of the service `id`, like `Hosts1`.
    pub(crate) fn state(&self, id: &str, variable: &str) -> Option<&str> {
        let key = (id.to_owned(), variable.to_owned());
        self.written
            .get(&key)
            .or_else(|| self.recorded.get(&key))
            .map(String::as_str)
    }

    /// Answers a call of `action` with `arguments`: the output arguments, or
    /// the error.
    pub(crate) fn call(
        &mut self,
        service: &Service,
        action: &str,
        arguments: &[(String, String)],
        hooks: &Hooks,
    ) -> Result<Vec<(String, String)>, Fault> {
        let id = short_id(service);
        let scpd = self
            .fixture
            .scpd(service)
            .ok_or_else(|| Fault::new(INVALID_ACTION))?;
        let action = scpd
            .action(action)
            .ok_or_else(|| Fault::new(INVALID_ACTION))?;
        validate(scpd, action, arguments)?;
        if let Some(fault) = &hooks.fault {
            return Err(fault.clone());
        }
        let recording = self
            .fixture
            .recordings
            .get(&(id.to_owned(), action.name.clone()));
        if let Some(recording) = recording.filter(|r| !r.indexed.is_empty()) {
            return indexed(action, recording, arguments);
        }
        let out = match recording.and_then(|r| r.once.as_ref()) {
            Some(Answer::Fault { code, description }) if is_getter(&action.name) => {
                return Err(Fault {
                    code: *code,
                    description: description.clone(),
                });
            }
            Some(Answer::Ok(out)) => Some(out),
            _ => None,
        };
        if !(is_getter(&action.name) || hooks.discard_writes) {
            let written = arguments.iter().filter_map(|(name, value)| {
                let argument = action.arguments.iter().find(|a| &a.name == name)?;
                let key = (id.to_owned(), argument.related_state_variable.clone());
                Some((key, value.clone()))
            });
            self.written.extend(written);
            let side_effects = hooks
                .side_effects
                .iter()
                .map(|(variable, value)| ((id.to_owned(), variable.clone()), value.clone()));
            self.written.extend(side_effects);
        }
        Ok(outputs(action)
            .map(|argument| {
                let variable = &argument.related_state_variable;
                let key = (id.to_owned(), variable.clone());
                let value = self
                    .written
                    .get(&key)
                    .or_else(|| out.and_then(|out| out.get(&argument.name)))
                    .or_else(|| self.recorded.get(&key))
                    .or_else(|| scpd.state_variable(variable)?.default_value.as_ref());
                (argument.name.clone(), value.cloned().unwrap_or_default())
            })
            .collect())
    }
}

impl Fixture {
    /// The service description of a service.
    pub(crate) fn scpd(&self, service: &Service) -> Option<&Scpd> {
        let name = service.scpd_url.rsplit('/').next().unwrap_or_default();
        self.scpds.get(name).map(|(_, scpd)| scpd)
    }
}

/// The last part of the service ID, like `Hosts1`. Unique within a device.
pub(crate) fn short_id(service: &Service) -> &str {
    service.service_id.rsplit(':').next().unwrap_or_default()
}

fn is_getter(action: &str) -> bool {
    action.starts_with("Get") || action.starts_with("X_AVM-DE_Get")
}

fn outputs(action: &Action) -> impl Iterator<Item = &tr064::Argument> {
    action
        .arguments
        .iter()
        .filter(|a| a.direction == Direction::Out)
}

/// The recorded answer for the index in the only input argument.
fn indexed(
    action: &Action,
    recording: &Recording,
    arguments: &[(String, String)],
) -> Result<Vec<(String, String)>, Fault> {
    let answer = arguments
        .first()
        .and_then(|(_, index)| index.parse().ok())
        .and_then(|index| recording.indexed.get(&index))
        .ok_or_else(|| Fault::new(INVALID_INDEX))?;
    match answer {
        Answer::Ok(out) => Ok(outputs(action)
            .map(|a| {
                let value = out.get(&a.name).cloned().unwrap_or_default();
                (a.name.clone(), value)
            })
            .collect()),
        Answer::Fault { code, description } => Err(Fault {
            code: *code,
            description: description.clone(),
        }),
    }
}

/// Checks the arguments of a call against the service description: every
/// input argument must be there, nothing else, and each value must fit its
/// state variable.
fn validate(scpd: &Scpd, action: &Action, arguments: &[(String, String)]) -> Result<(), Fault> {
    let inputs: Vec<_> = action
        .arguments
        .iter()
        .filter(|a| a.direction == Direction::In)
        .collect();
    let given = |name: &str| arguments.iter().any(|(n, _)| n == name);
    let known = |name: &str| inputs.iter().any(|a| a.name == name);
    if !inputs.iter().all(|a| given(&a.name)) || !arguments.iter().all(|(n, _)| known(n)) {
        return Err(Fault::new(INVALID_ARGS));
    }
    let variables = arguments.iter().filter_map(|(name, value)| {
        let argument = inputs.iter().find(|a| &a.name == name)?;
        Some((
            scpd.state_variable(&argument.related_state_variable)?,
            value,
        ))
    });
    for (variable, value) in variables {
        check(variable, value).map_err(Fault::new)?;
    }
    Ok(())
}

/// Checks a value against the data type and the allowed values of a state
/// variable. The allowed range is not checked, because devices don't
/// either: an index beyond the range of its variable gets the error 713
/// like any other index without an entry, and clients rely on that to find
/// the end of a list.
fn check(variable: &StateVariable, value: &str) -> Result<(), u16> {
    let allowed =
        variable.allowed_values.is_empty() || variable.allowed_values.iter().any(|v| v == value);
    if has_type(&variable.data_type, value) && allowed {
        Ok(())
    } else {
        Err(INVALID_VALUE)
    }
}

fn has_type(data_type: &str, value: &str) -> bool {
    match data_type {
        "ui1" => value.parse::<u8>().is_ok(),
        "ui2" => value.parse::<u16>().is_ok(),
        "ui4" => value.parse::<u32>().is_ok(),
        "i1" => value.parse::<i8>().is_ok(),
        "i2" => value.parse::<i16>().is_ok(),
        "i4" => value.parse::<i32>().is_ok(),
        "boolean" => matches!(value, "0" | "1" | "true" | "false" | "yes" | "no"),
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tr064::AllowedRange;

    fn variable(data_type: &str) -> StateVariable {
        let xml = format!(
            "<scpd><serviceStateTable><stateVariable><name>V</name>\
             <dataType>{data_type}</dataType></stateVariable></serviceStateTable></scpd>"
        );
        Scpd::parse(&xml).unwrap().state_variables.remove(0)
    }

    #[test]
    fn checks_data_types() {
        for (data_type, valid, invalid) in [
            ("ui1", "255", "256"),
            ("ui2", "65535", "-1"),
            ("ui4", "4294967295", "x"),
            ("i1", "-128", "128"),
            ("i2", "-32768", "1.5"),
            ("i4", "-1", ""),
            ("boolean", "yes", "2"),
        ] {
            assert_eq!(check(&variable(data_type), valid), Ok(()), "{data_type}");
            assert_eq!(
                check(&variable(data_type), invalid),
                Err(INVALID_VALUE),
                "{data_type}"
            );
        }
        assert_eq!(check(&variable("string"), "anything"), Ok(()));
    }

    #[test]
    fn checks_allowed_values_but_not_ranges() {
        let mut status = variable("string");
        status.allowed_values = vec!["Up".to_owned(), "Disabled".to_owned()];
        assert_eq!(check(&status, "Up"), Ok(()));
        assert_eq!(check(&status, "Down"), Err(INVALID_VALUE));

        let mut index = variable("ui2");
        index.allowed_range = Some(AllowedRange {
            minimum: Some("0".to_owned()),
            maximum: Some("4".to_owned()),
            step: None,
        });
        assert_eq!(check(&index, "5"), Ok(()));
    }

    #[test]
    fn describes_faults() {
        let codes = [
            401, 402, 501, 600, 601, 606, 713, 714, 820, 866, 867, 868, 999,
        ];
        let mut descriptions: Vec<_> = codes.iter().map(|&c| Fault::new(c).description).collect();
        descriptions.sort();
        descriptions.dedup();
        assert_eq!(descriptions.len(), codes.len());
    }
}
