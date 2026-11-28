//! Settings of the device, read through the REST API of the web interface
//! with a [`Session`]: `/api/v0/generic/<module>` answers with the settings
//! of a module, like `ipv6` or `dnscfg`, as JSON.
//!
//! The API is not documented. Modules and their fields come from the web
//! interface and may change with FRITZ!OS. Only `GET` is used. Values whose
//! names point to secrets, like passwords and keys, are hidden.

use std::collections::BTreeMap;

use serde::Serialize;
use serde_json::{Map, Value};

use crate::names::{Sensitivity, sensitivity};
use crate::redact::{self, HIDDEN};
use crate::session::{Error, Session};

/// Where the modules are.
pub const PREFIX: &str = "/api/v0/generic/";

/// The modules the device is known to have, with what they hold, and
/// whether they are read by default. The others are large or personal.
pub const MODULES: &[(&str, &str, bool)] = &[
    (
        "box",
        "name, LEDs, night time, UPnP and DHCP lease times",
        true,
    ),
    (
        "connections",
        "internet connections, with IPv4 and IPv6 DNS",
        true,
    ),
    ("dnscfg", "DNS servers and DNS over TLS", true),
    (
        "dns_excepted_domains",
        "domains excepted from DNS rebind protection",
        true,
    ),
    (
        "eth_ports",
        "LAN ports, their speed and energy saving",
        true,
    ),
    ("forwardrules", "whether a host is exposed", true),
    ("ipv6", "IPv6 of the home network, MTU and DHCPv6", true),
    (
        MESH,
        "whether mesh devices choose the best connection themselves, and the role in the mesh",
        true,
    ),
    ("route", "static IPv4 routes", true),
    (
        "sync_groups",
        "IPv4 and IPv6 modes of the internet connection",
        true,
    ),
    ("time", "time server and time zone", true),
    ("dect", "DECT and its handsets", false),
    (
        "filter_profile",
        "access profiles of the parental controls",
        false,
    ),
    (
        "inetstat",
        "online counter: traffic by day, week and month",
        false,
    ),
    ("landevice", "hosts of the home network", false),
    ("mobiled", "mobile modem", false),
    ("plc", "powerline energy saving", false),
    ("tam", "answering machines", false),
    ("user", "users of the parental controls", false),
    ("vpn", "VPN connections", false),
    ("wlan", "wireless networks and their clients", false),
];

/// A module that the device does not have: the settings of the mesh, put
/// together from the [`MESH_FIELDS`].
pub const MESH: &str = "mesh";

/// The fields of [`MESH`], by the module that holds them:
/// `backhaul_steering` is the switch "Mesh devices choose the best
/// connection automatically" of the web interface.
pub const MESH_FIELDS: &[(&str, &[&str])] = &[
    ("wlan", &["backhaul_steering"]),
    ("nexus", &["role", "is_master"]),
];

/// The settings of several modules.
#[derive(Debug, Default, Serialize)]
pub struct Settings {
    /// The settings by module.
    pub modules: BTreeMap<String, Value>,
    /// Why modules could not be read, by module.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub unavailable: BTreeMap<String, String>,
}

/// Reads `modules`, or the [`MODULES`] if empty. A module that cannot be
/// read is listed in [`Settings::unavailable`] instead.
pub async fn settings(session: &Session, modules: &[String]) -> Settings {
    let defaults: Vec<String> = MODULES
        .iter()
        .filter(|(_, _, default)| *default)
        .map(|(name, _, _)| (*name).to_owned())
        .collect();
    let modules = if modules.is_empty() {
        &defaults
    } else {
        modules
    };
    let mut settings = Settings::default();
    for name in modules {
        let value = if name == MESH {
            mesh(session).await
        } else {
            module(session, name).await
        };
        match value {
            Ok(value) => {
                settings.modules.insert(name.clone(), value);
            }
            Err(e) => {
                settings.unavailable.insert(name.clone(), e.to_string());
            }
        }
    }
    settings
}

/// Reads the settings of one module, with secrets hidden.
///
/// # Errors
///
/// [`Error::Invalid`] if `name` is not a module name or the answer is not
/// JSON, and the errors of the request.
pub async fn module(session: &Session, name: &str) -> Result<Value, Error> {
    let valid = !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
    if !valid {
        return Err(Error::Invalid {
            what: "the module name",
            reason: format!("{name:?} is not a module"),
        });
    }
    parse(&session.get(&format!("{PREFIX}{name}")).await?)
}

/// The settings of the mesh: the [`MESH_FIELDS`] of the modules that can
/// be read.
///
/// # Errors
///
/// The error of the first module if none can be read.
pub async fn mesh(session: &Session) -> Result<Value, Error> {
    let mut modules = Vec::new();
    let mut first = None;
    for (name, _) in MESH_FIELDS {
        match module(session, name).await {
            Ok(value) => modules.push((*name, value)),
            Err(e) => {
                first.get_or_insert(e);
            }
        }
    }
    match first {
        Some(e) if modules.is_empty() => Err(e),
        _ => Ok(mesh_of(&modules)),
    }
}

/// The [`MESH_FIELDS`] found in `modules`, by module name.
fn mesh_of(modules: &[(&str, Value)]) -> Value {
    let mut mesh = Map::new();
    for (name, value) in modules {
        let fields = MESH_FIELDS
            .iter()
            .filter(|(module, _)| module == name)
            .flat_map(|(_, fields)| fields.iter());
        for field in fields {
            if let Some(found) = value.get(field) {
                mesh.insert((*field).to_owned(), found.clone());
            }
        }
    }
    Value::Object(mesh)
}

/// The settings in `json`, with secrets hidden.
///
/// # Errors
///
/// [`Error::Invalid`] if `json` is not JSON.
pub fn parse(json: &str) -> Result<Value, Error> {
    let value = serde_json::from_str(json).map_err(|e| Error::Invalid {
        what: "the settings API",
        reason: e.to_string(),
    })?;
    Ok(hide(value))
}

/// `value` with the values of [`secret`] names, and texts with private
/// keys, replaced by [`HIDDEN`], at any depth, and session IDs in URLs
/// hidden.
pub fn hide(value: Value) -> Value {
    match value {
        Value::Object(object) => Value::Object(
            object
                .into_iter()
                .map(|(name, value)| {
                    let value = if secret(&name) {
                        Value::String(HIDDEN.to_owned())
                    } else {
                        hide(value)
                    };
                    (name, value)
                })
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.into_iter().map(hide).collect()),
        Value::String(text) if PRIVATE.iter().any(|p| text.contains(p)) => {
            Value::String(HIDDEN.to_owned())
        }
        Value::String(text) => Value::String(redact::value("", &text)),
        other => other,
    }
}

/// Parts of names of secrets that are not words of their own, like in
/// `pskvalue`, the password of a wireless network.
const SECRET_PARTS: &[&str] = &["psk", "passw", "secret", "privatekey", "private_key"];

/// Texts that only private keys contain, like a `WireGuard` configuration.
const PRIVATE: &[&str] = &["PrivateKey", "PRIVATE KEY"];

/// Whether the value of `name` is hidden: the names that are not plain for
/// TR-064, names with one of the [`SECRET_PARTS`], and `WireGuard`
/// configurations like `wg_slave_config`, which contain private keys.
fn secret(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    sensitivity(name) != Sensitivity::Plain
        || SECRET_PARTS.iter().any(|part| lower.contains(part))
        || (lower.starts_with("wg_") && lower.ends_with("config"))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn hides_secrets_at_any_depth() {
        let value = json!({
            "name": "box",
            "password": "secret",
            "accounts": [{"username": "user", "psk": {"value": "key"}, "pin_enabled": "1"}],
            "url": "/x.lua?sid=0123",
            "mtu": 1492,
            "pskvalue": "wlan key",
            "guest_pskvalue": "guest key",
            "wg_slave_config": "[Interface]",
            "dns_domains": "[Interface]\nPrivateKey = abc",
            "ip4_is_private": "1",
        });
        assert_eq!(
            hide(value),
            json!({
                "name": "box",
                "password": "***",
                "accounts": [{"username": "***", "psk": "***", "pin_enabled": "1"}],
                "url": "/x.lua?sid=***",
                "mtu": 1492,
                "pskvalue": "***",
                "guest_pskvalue": "***",
                "wg_slave_config": "***",
                "dns_domains": "***",
                "ip4_is_private": "1",
            })
        );
    }

    #[test]
    fn puts_the_mesh_together() {
        let modules = [
            (
                "wlan",
                json!({"backhaul_steering": "1", "pskvalue": "***", "ssid": "wlan-1"}),
            ),
            (
                "nexus",
                json!({"role": "auto", "is_master": "1", "peers": []}),
            ),
        ];
        assert_eq!(
            mesh_of(&modules),
            json!({"backhaul_steering": "1", "role": "auto", "is_master": "1"})
        );
        // Fields the device does not have are left out.
        assert_eq!(
            mesh_of(&modules[1..]),
            json!({"role": "auto", "is_master": "1"})
        );
    }

    #[test]
    fn rejects_invalid_json() {
        assert!(matches!(parse("<html>"), Err(Error::Invalid { .. })));
    }
}
