//! What `fritz capture` reads from a device, and what it leaves alone.
//!
//! The capture only ever calls getters. Of those it skips the ones that need
//! input other than an index, the ones that could return secrets, and the
//! ones that return personal lists unless asked for. The decision depends
//! only on the SCPD, so the plan can be reviewed before anything is called.

use std::fmt;

use tr064::{Action, Client, DeviceDescription, Direction, Scpd, Service};

use crate::policy::{self, Class};

mod run;

pub use run::{Capture, List, Options, Outcome, Response, run, write};

/// What the capture does with one action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// A getter without input: called once.
    Call,
    /// A getter whose only input is an index: called for each entry.
    Iterate { index: String },
    /// A getter that needs other input: not called.
    NeedsInput { inputs: Vec<String> },
    /// A getter for personal lists like calls or contacts: only called when
    /// asked for.
    Personal,
    /// A getter that could return secrets: never called.
    Denied { reason: String },
    /// Not a getter: never called.
    NotGetter,
}

/// Decides what the capture does with an action of a service, following
/// its [class](policy::classify).
pub fn decide(service: &Service, action: &Action) -> Decision {
    match policy::classify(service, action) {
        Class::Read => shape(action),
        Class::Personal => Decision::Personal,
        Class::Secret { reason } => Decision::Denied { reason },
        Class::Write | Class::Critical { .. } => Decision::NotGetter,
    }
}

/// How a getter is called, depending on its inputs: [`Decision::Call`],
/// [`Decision::Iterate`] or [`Decision::NeedsInput`].
fn shape(action: &Action) -> Decision {
    let inputs: Vec<String> = action
        .arguments
        .iter()
        .filter(|a| a.direction == Direction::In)
        .map(|a| a.name.clone())
        .collect();
    match inputs.as_slice() {
        [] => Decision::Call,
        [index] if index.ends_with("Index") => Decision::Iterate {
            index: index.clone(),
        },
        _ => Decision::NeedsInput { inputs },
    }
}

/// The decisions for all actions of a device, with the documents they are
/// based on.
#[derive(Debug)]
pub struct Plan {
    /// The device description as the device sent it.
    pub description: String,
    pub services: Vec<ServicePlan>,
}

/// The decisions for the actions of one service, or why its SCPD could not
/// be read.
#[derive(Debug)]
pub struct ServicePlan {
    pub service: Service,
    pub scpd: Result<ServiceScpd, tr064::Error>,
}

/// The SCPD of a service and the decision for each of its actions.
#[derive(Debug)]
pub struct ServiceScpd {
    /// The SCPD as the device sent it.
    pub xml: String,
    pub actions: Vec<(Action, Decision)>,
}

/// Reads the device description and all SCPDs and decides for each action.
/// Calls no action. A service whose SCPD cannot be read is part of the plan
/// with its error.
///
/// # Errors
///
/// The errors of reading and parsing the device description.
pub async fn plan(client: &Client) -> Result<Plan, tr064::Error> {
    let description = client.get("/tr64desc.xml").await?;
    let parsed = DeviceDescription::parse(&description)?;
    let mut services = Vec::new();
    for service in parsed.services() {
        services.push(ServicePlan {
            service: service.clone(),
            scpd: plan_service(client, service).await,
        });
    }
    Ok(Plan {
        description,
        services,
    })
}

async fn plan_service(client: &Client, service: &Service) -> Result<ServiceScpd, tr064::Error> {
    let xml = client.get(&service.scpd_url).await?;
    let actions = Scpd::parse(&xml)?
        .actions
        .into_iter()
        .map(|action| {
            let decision = decide(service, &action);
            (action, decision)
        })
        .collect();
    Ok(ServiceScpd { xml, actions })
}

/// The last part of the service ID, like `Hosts1`. Unique within a device.
pub fn short_id(service: &Service) -> &str {
    service.service_id.rsplit(':').next().unwrap_or_default()
}

/// How many actions got each decision, and how many services failed.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Summary {
    pub call: usize,
    pub iterate: usize,
    pub needs_input: usize,
    pub personal: usize,
    pub denied: usize,
    pub not_getter: usize,
    pub failed_services: usize,
}

impl Summary {
    fn count(&mut self, decision: &Decision) {
        let counter = match decision {
            Decision::Call => &mut self.call,
            Decision::Iterate { .. } => &mut self.iterate,
            Decision::NeedsInput { .. } => &mut self.needs_input,
            Decision::Personal => &mut self.personal,
            Decision::Denied { .. } => &mut self.denied,
            Decision::NotGetter => &mut self.not_getter,
        };
        *counter += 1;
    }
}

impl Plan {
    /// Counts the decisions.
    pub fn summary(&self) -> Summary {
        let mut summary = Summary::default();
        for service in &self.services {
            match &service.scpd {
                Ok(scpd) => scpd.actions.iter().for_each(|(_, d)| summary.count(d)),
                Err(_) => summary.failed_services += 1,
            }
        }
        summary
    }
}

impl fmt::Display for Plan {
    /// One block per service listing the getters, followed by a summary.
    /// Actions that are not getters are only counted.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for service in &self.services {
            write_service(f, service)?;
        }
        let s = self.summary();
        write!(
            f,
            "{} services: {} to call, {} to iterate, {} need input, {} personal, \
             {} denied, {} not getters, {} SCPDs failed",
            self.services.len(),
            s.call,
            s.iterate,
            s.needs_input,
            s.personal,
            s.denied,
            s.not_getter,
            s.failed_services
        )
    }
}

fn write_service(f: &mut fmt::Formatter<'_>, plan: &ServicePlan) -> fmt::Result {
    let service = &plan.service;
    let id = short_id(service);
    writeln!(f, "{}:{} ({id})", service.name(), service.version())?;
    match &plan.scpd {
        Ok(scpd) => {
            let mut not_getters = 0;
            for (action, decision) in &scpd.actions {
                match decision {
                    Decision::NotGetter => not_getters += 1,
                    _ => writeln!(f, "  {}", Line(&action.name, decision))?,
                }
            }
            if not_getters > 0 {
                writeln!(f, "  never        not getters: {not_getters}")?;
            }
        }
        Err(error) => writeln!(f, "  SCPD failed: {error}")?,
    }
    writeln!(f)
}

/// One action of the plan, like `iterate      GetGenericHostEntry (NewIndex)`.
struct Line<'a>(&'a str, &'a Decision);

impl fmt::Display for Line<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Line(name, decision) = self;
        match decision {
            Decision::Call => write!(f, "call         {name}"),
            Decision::Iterate { index } => write!(f, "iterate      {name} ({index})"),
            Decision::NeedsInput { inputs } => {
                write!(f, "needs input  {name} ({})", inputs.join(", "))
            }
            Decision::Personal => write!(f, "personal     {name}"),
            Decision::Denied { reason } => write!(f, "denied       {name}: {reason}"),
            Decision::NotGetter => write!(f, "never        {name}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use tr064::Argument;

    use super::*;

    fn service(name: &str) -> Service {
        Service {
            service_type: format!("urn:dslforum-org:service:{name}:1"),
            service_id: format!("urn:{name}-com:serviceId:{}1", name.to_lowercase()),
            control_url: "/upnp/control/x".to_owned(),
            event_sub_url: None,
            scpd_url: "/x.xml".to_owned(),
        }
    }

    fn action(name: &str, inputs: &[&str], outputs: &[&str]) -> Action {
        let argument = |direction| {
            move |name: &&str| Argument {
                name: (*name).to_owned(),
                direction,
                related_state_variable: "X".to_owned(),
            }
        };
        Action {
            name: name.to_owned(),
            arguments: inputs
                .iter()
                .map(argument(Direction::In))
                .chain(outputs.iter().map(argument(Direction::Out)))
                .collect(),
        }
    }

    fn decide_for(service_name: &str, a: &Action) -> Decision {
        decide(&service(service_name), a)
    }

    #[test]
    fn calls_getters_without_input() {
        let get_info = action("GetInfo", &[], &["NewSerialNumber"]);
        assert_eq!(decide_for("DeviceInfo", &get_info), Decision::Call);
        let avm = action("X_AVM-DE_GetNumberOfEntries", &[], &[]);
        assert_eq!(decide_for("Hosts", &avm), Decision::Call);
    }

    #[test]
    fn iterates_getters_with_only_an_index() {
        let entry = action("GetGenericHostEntry", &["NewIndex"], &["NewIPAddress"]);
        assert_eq!(
            decide_for("Hosts", &entry),
            Decision::Iterate {
                index: "NewIndex".to_owned()
            }
        );
    }

    #[test]
    fn skips_getters_that_need_other_input() {
        for inputs in [&["NewMACAddress"][..], &["NewIndex", "NewOther"]] {
            let a = action("GetSpecificHostEntry", inputs, &[]);
            assert_eq!(
                decide_for("Hosts", &a),
                Decision::NeedsInput {
                    inputs: inputs.iter().map(|s| (*s).to_owned()).collect()
                }
            );
        }
    }

    #[test]
    fn never_calls_other_actions() {
        for name in [
            "SetEnable",
            "Reboot",
            "X_AVM-DE_CreateUrlSID",
            "X_AVM-DE_SetGet",
        ] {
            let a = action(name, &[], &[]);
            assert_eq!(
                decide_for("DeviceConfig", &a),
                Decision::NotGetter,
                "{name}"
            );
        }
    }

    #[test]
    fn denies_getters_that_could_return_secrets() {
        let cases = [
            (
                "DeviceConfig",
                action("X_AVM-DE_GetConfigFile", &["NewX_AVM-DE_Password"], &[]),
            ),
            ("DeviceConfig", action("GetPersistentData", &[], &[])),
            (
                "X_VoIP",
                action("X_AVM-DE_GetClient3", &["NewX_AVM-DE_ClientIndex"], &[]),
            ),
            ("WLANConfiguration", action("GetSecurityKeys", &[], &[])),
            (
                "WLANConfiguration",
                action("GetInfo", &[], &["NewSSID", "NewWEPKey0"]),
            ),
            (
                "WLANConfiguration",
                action("X_AVM-DE_GetWPSInfo", &[], &["NewX_AVM-DE_WPSClientPIN"]),
            ),
            (
                "LANConfigSecurity",
                action("X_AVM-DE_GetCurrentUser", &[], &["NewX_AVM-DE_UserRights"]),
            ),
        ];
        for (service_name, a) in &cases[..6] {
            assert!(
                matches!(decide_for(service_name, a), Decision::Denied { .. }),
                "{}",
                a.name
            );
        }
        let (service_name, a) = &cases[6];
        assert_eq!(decide_for(service_name, a), Decision::Call);
        assert_eq!(
            decide_for("WLANConfiguration", &cases[4].1),
            Decision::Denied {
                reason: "returns NewWEPKey0, which points to a secret".to_owned()
            }
        );
    }

    #[test]
    fn calls_getters_that_only_describe_secrets() {
        let cases = [
            (
                "WLANConfiguration",
                action(
                    "GetInfo",
                    &[],
                    &["NewMinCharsPSK", "NewMaxCharsPSK", "NewAllowedCharsPSK"],
                ),
            ),
            (
                "WLANConfiguration",
                action("GetDefaultWEPKeyIndex", &[], &["NewDefaultWEPKeyIndex"]),
            ),
            (
                "UserInterface",
                action("GetInfo", &[], &["NewPasswordRequired"]),
            ),
            (
                "X_VoIP",
                action("GetInfoEx", &[], &["NewVoIPPasswordMinChars"]),
            ),
            (
                "X_AVM-DE_WANMobileConnection",
                action("GetInfo", &[], &["NewPINFailureCount"]),
            ),
            (
                "WANPPPConnection",
                action("GetInfo", &[], &["NewMaxCharsPassword", "NewUserName"]),
            ),
            (
                "ManagementServer",
                action("GetInfo", &[], &["NewUsername", "NewParameterKey"]),
            ),
        ];
        for (service_name, a) in &cases {
            assert_eq!(
                decide_for(service_name, a),
                Decision::Call,
                "{service_name} {}",
                a.name
            );
        }
    }

    #[test]
    fn marks_personal_lists() {
        for (service_name, name) in [
            ("X_AVM-DE_OnTel", "GetCallList"),
            ("X_AVM-DE_OnTel", "GetPhonebookList"),
            ("X_AVM-DE_TAM", "GetMessageList"),
            ("DeviceInfo", "GetDeviceLog"),
        ] {
            let a = action(name, &[], &[]);
            assert_eq!(decide_for(service_name, &a), Decision::Personal, "{name}");
        }
        let deflections = action("GetDeflections", &[], &[]);
        assert_eq!(decide_for("X_AVM-DE_OnTel", &deflections), Decision::Call);
    }

    #[test]
    fn renders_every_decision() {
        let lines = [
            (Decision::Call, "call         A"),
            (Decision::Personal, "personal     A"),
            (Decision::NotGetter, "never        A"),
        ];
        for (decision, expected) in lines {
            assert_eq!(Line("A", &decision).to_string(), expected);
        }
    }
}
