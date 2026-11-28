//! Which actions `fritz` may call.
//!
//! Every action of a service falls into a [`Class`]. Getters read, unless
//! they could return secrets or personal lists. Everything else changes the
//! device: a plain write, or a critical one if it can lock the user out,
//! cost money, expose the device, or is unknown. The class depends only on
//! the names in the SCPD, so it is known before anything is called.

use std::fmt;

use tr064::{Action, Direction, Service};

use crate::names::{Sensitivity, sensitivity, words};

/// Getters that are never called although they look harmless by name.
const DENIED_GETTERS: &[(&str, &str)] = &[
    (
        "X_AVM-DE_GetConfigFile",
        "exports the whole configuration including passwords",
    ),
    (
        "GetPersistentData",
        "returns opaque data that can hold credentials",
    ),
];

/// Services whose changes can lock the user out, expose the device, or
/// touch its firmware, users or internet connection. Services whose name
/// starts with `WAN` count as well.
const CRITICAL_SERVICES: &[&str] = &[
    "DeviceConfig",
    "DeviceInfo",
    "LANConfigSecurity",
    "LANHostConfigManagement",
    "Layer3Forwarding",
    "ManagementServer",
    "UserInterface",
    "X_AVM-DE_AppSetup",
    "X_AVM-DE_Auth",
    "X_AVM-DE_MyFritz",
    "X_AVM-DE_RemoteAccess",
    "X_AVM-DE_Storage",
    "X_AVM-DE_USPController",
    "X_AVM-DE_WANMobileConnection",
];

/// Words in action names of critical changes: restarts, resets, firmware
/// updates and phone calls.
const CRITICAL_WORDS: &[&str] = &["dial", "firmware", "reboot", "reset", "update", "upgrade"];

/// The first words of action names that change a setting or an entry.
/// Other verbs are critical, because nothing is known about them.
const WRITE_VERBS: &[&str] = &[
    "add", "del", "delete", "discard", "disallow", "mark", "new", "set",
];

/// What an action does, as far as `fritz` is concerned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Class {
    /// A getter of settings or state.
    Read,
    /// A getter of personal lists: calls, contacts, voice messages and the
    /// event log.
    Personal,
    /// A getter that could return passwords, keys or other secrets.
    Secret { reason: String },
    /// A change of a setting or an entry.
    Write,
    /// A change that can lock the user out, cost money, expose the device or
    /// reset it, or an action nothing is known about.
    Critical { reason: String },
}

impl Class {
    /// The class in one word, like `read`.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Personal => "personal",
            Self::Secret { .. } => "secret",
            Self::Write => "write",
            Self::Critical { .. } => "critical",
        }
    }

    /// Whether the action only reads, so that `fritz` may call it.
    pub fn is_read(&self) -> bool {
        matches!(self, Self::Read | Self::Personal)
    }

    /// Why the action is a secret or critical one.
    pub fn reason(&self) -> Option<&str> {
        match self {
            Self::Secret { reason } | Self::Critical { reason } => Some(reason),
            _ => None,
        }
    }
}

impl fmt::Display for Class {
    /// The [name](Class::name), followed by the reason if there is one.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.reason() {
            Some(reason) => write!(f, "{}: {reason}", self.name()),
            None => f.write_str(self.name()),
        }
    }
}

/// Whether an action name is that of a getter, like `GetInfo` or
/// `X_AVM-DE_GetHostListPath`.
pub fn is_getter(action: &str) -> bool {
    action.starts_with("Get") || action.starts_with("X_AVM-DE_Get")
}

/// The class of an action of a service.
pub fn classify(service: &Service, action: &Action) -> Class {
    let name = action.name.as_str();
    if !is_getter(name) {
        return change(service, action);
    }
    if let Some(reason) = denial(service, action) {
        return Class::Secret { reason };
    }
    if is_personal(service, name) {
        return Class::Personal;
    }
    Class::Read
}

/// Why a getter could return secrets, if it could.
fn denial(service: &Service, action: &Action) -> Option<String> {
    let name = action.name.as_str();
    if let Some((_, reason)) = DENIED_GETTERS.iter().find(|(n, _)| *n == name) {
        return Some((*reason).to_owned());
    }
    if service.name() == "X_VoIP" && name.starts_with("X_AVM-DE_GetClient") {
        return Some("returns the credentials of telephony clients".to_owned());
    }
    if sensitivity(name) == Sensitivity::Secret {
        return Some("the name points to a secret".to_owned());
    }
    action
        .arguments
        .iter()
        .find(|a| a.direction == Direction::Out && sensitivity(&a.name) == Sensitivity::Secret)
        .map(|a| format!("returns {}, which points to a secret", a.name))
}

/// Whether a getter returns personal data: calls, contacts, voice messages
/// and the event log, which names devices and numbers in free text.
pub(crate) fn is_personal(service: &Service, action: &str) -> bool {
    match service.name() {
        "X_AVM-DE_OnTel" => action == "GetCallList" || action.starts_with("GetPhonebook"),
        "X_AVM-DE_TAM" => action == "GetMessageList",
        "DeviceInfo" => action == "GetDeviceLog",
        _ => false,
    }
}

/// Whether an output argument holds personal data even though its action
/// does not, like the event log that `DeviceInfo:GetInfo` returns.
pub fn is_personal_output(name: &str) -> bool {
    name == "NewDeviceLog"
}

/// The class of an action that is not a getter.
fn change(service: &Service, action: &Action) -> Class {
    let critical = |reason: String| Class::Critical { reason };
    let service_name = service.name();
    if service_name.starts_with("WAN") || CRITICAL_SERVICES.contains(&service_name) {
        return critical(format!("changes {service_name}"));
    }
    let name = action.name.as_str();
    if sensitivity(name) == Sensitivity::Secret {
        return critical("sets a secret".to_owned());
    }
    if let Some(input) = action
        .inputs()
        .find(|a| sensitivity(&a.name) == Sensitivity::Secret)
    {
        return critical(format!("sets {}, which points to a secret", input.name));
    }
    let words = words(name);
    if let Some(word) = words.iter().find(|w| CRITICAL_WORDS.contains(&w.as_str())) {
        return critical(format!("the name says {word}"));
    }
    let verb = words
        .iter()
        .find(|w| !["x", "avm", "de"].contains(&w.as_str()))
        .map_or("", String::as_str);
    if WRITE_VERBS.contains(&verb) {
        Class::Write
    } else {
        critical(format!("unknown kind of action {name}"))
    }
}

#[cfg(test)]
mod tests {
    use tr064::Argument;

    use super::*;

    fn service(name: &str) -> Service {
        Service {
            service_type: format!("urn:dslforum-org:service:{name}:1"),
            service_id: format!("urn:{name}-com:serviceId:{name}1"),
            control_url: "/upnp/control/x".to_owned(),
            event_sub_url: None,
            scpd_url: "/x.xml".to_owned(),
        }
    }

    fn action(name: &str, outputs: &[&str]) -> Action {
        with_arguments(name, Direction::Out, outputs)
    }

    fn with_arguments(name: &str, direction: Direction, arguments: &[&str]) -> Action {
        Action {
            name: name.to_owned(),
            arguments: arguments
                .iter()
                .map(|name| Argument {
                    name: (*name).to_owned(),
                    direction,
                    related_state_variable: "X".to_owned(),
                })
                .collect(),
        }
    }

    fn class(service_name: &str, name: &str) -> Class {
        classify(&service(service_name), &action(name, &[]))
    }

    #[test]
    fn reads_getters() {
        assert_eq!(class("DeviceInfo", "GetInfo"), Class::Read);
        assert_eq!(class("Hosts", "X_AVM-DE_GetHostListPath"), Class::Read);
        assert_eq!(class("X_AVM-DE_Auth", "GetState"), Class::Read);
        assert_eq!(class("X_AVM-DE_OnTel", "GetCallList"), Class::Personal);
    }

    #[test]
    fn keeps_secrets() {
        assert_eq!(
            class("DeviceConfig", "X_AVM-DE_GetConfigFile"),
            Class::Secret {
                reason: "exports the whole configuration including passwords".to_owned()
            }
        );
        assert_eq!(
            classify(
                &service("WLANConfiguration"),
                &action("GetInfo", &["NewSSID", "NewWEPKey0"])
            )
            .to_string(),
            "secret: returns NewWEPKey0, which points to a secret"
        );
    }

    #[test]
    fn tells_writes_from_critical_changes() {
        for (service_name, name) in [
            ("WLANConfiguration", "SetSSID"),
            ("WLANConfiguration", "SetDefaultWEPKeyIndex"),
            ("Hosts", "X_AVM-DE_SetFriendlyNameByMAC"),
            ("X_AVM-DE_OnTel", "DeletePhonebookEntryUID"),
            ("X_AVM-DE_HostFilter", "DisallowWANAccessByIP"),
            ("X_AVM-DE_Filelinks", "NewFilelinkEntry"),
            ("X_VoIP", "X_AVM-DE_DelVoIPAccount"),
            ("Other", "X_SetEnable"),
        ] {
            assert_eq!(class(service_name, name), Class::Write, "{name}");
        }
        for (service_name, name, reason) in [
            ("DeviceConfig", "Reboot", "changes DeviceConfig"),
            (
                "WANPPPConnection",
                "SetUserName",
                "changes WANPPPConnection",
            ),
            ("WLANConfiguration", "SetSecurityKeys", "sets a secret"),
            ("X_VoIP", "X_AVM-DE_DialNumber", "the name says dial"),
            ("Hosts", "X_AVM-DE_HostDoUpdate", "the name says update"),
            (
                "Hosts",
                "X_AVM-DE_WakeOnLANByMACAddress",
                "unknown kind of action X_AVM-DE_WakeOnLANByMACAddress",
            ),
            ("Other", "__", "unknown kind of action __"),
        ] {
            assert_eq!(
                class(service_name, name),
                Class::Critical {
                    reason: reason.to_owned()
                },
                "{name}"
            );
        }
        let set_client = with_arguments(
            "X_AVM-DE_SetClient",
            Direction::In,
            &["NewX_AVM-DE_ClientIndex", "NewX_AVM-DE_ClientPassword"],
        );
        assert_eq!(
            classify(&service("X_VoIP"), &set_client).to_string(),
            "critical: sets NewX_AVM-DE_ClientPassword, which points to a secret"
        );
    }

    #[test]
    fn finds_personal_outputs() {
        assert!(is_personal_output("NewDeviceLog"));
        assert!(!is_personal_output("NewDeviceLogPath"));
    }

    #[test]
    fn names_the_classes() {
        let names: Vec<String> = [
            Class::Read,
            Class::Personal,
            Class::Write,
            class("DeviceConfig", "Reboot"),
        ]
        .iter()
        .map(ToString::to_string)
        .collect();
        assert_eq!(
            names,
            [
                "read",
                "personal",
                "write",
                "critical: changes DeviceConfig"
            ]
        );
        assert!(Class::Personal.is_read());
        assert!(!Class::Write.is_read());
    }
}
