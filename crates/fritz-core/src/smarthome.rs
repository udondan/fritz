//! Smart Home devices, read through the REST API of the web interface
//! (FRITZ!OS 8.20 and later) with a [`Session`].
//!
//! The API describes devices, like a plug, and their units, like the switch
//! and the thermometer of the plug. Each unit has interfaces with the
//! values. [`devices`] turns this into a compact list: each device with its
//! units and their current values, in common units of measurement. Values
//! the device does not report as valid are left out.

use serde::{Deserialize, Serialize};

use crate::session::{Error, Session};

/// The overview of all devices, units and groups.
pub const OVERVIEW: &str = "/api/v0/smarthome/overview";

/// A Smart Home device.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Device {
    /// The actor identification number, like `11630 0015376`.
    pub ain: String,
    pub name: String,
    /// Like `FRITZ!Smart Energy 200`.
    pub product: Option<String>,
    /// `blind`, `control`, `lamp`, `other`, `sensor`, `socket` or
    /// `thermostat`.
    pub category: Option<String>,
    pub firmware: Option<String>,
    /// Whether the device is reachable. If not, the values are the last
    /// known ones.
    pub connected: bool,
    /// The charge of the battery in percent, for devices with one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub battery: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub battery_low: Option<bool>,
    pub units: Vec<Unit>,
}

/// A function of a device, like a switch, and its values.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Unit {
    pub ain: String,
    pub name: String,
    /// Like `avmPlugSocket`, `avmThermostat` or `dimmableColorBulb`.
    pub kind: Option<String>,
    /// Whether a switch is on.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub on: Option<bool>,
    /// The measured temperature in °C.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    /// The target of a thermostat.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<Target>,
    /// The power in W.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub power: Option<f64>,
    /// The energy in Wh since the measurement started.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub energy: Option<u64>,
    /// The voltage in V.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub voltage: Option<f64>,
    /// The level of a dimmer or a blind in percent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub level: Option<u8>,
    /// The relative humidity in percent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub humidity: Option<u8>,
    /// Active alerts, like `open` or `smoke`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub alerts: Vec<String>,
}

/// The target of a thermostat: a temperature in °C, or `on` or `off`.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
pub enum Target {
    Celsius(f64),
    Mode(String),
}

/// Reads the devices, ordered by name.
///
/// # Errors
///
/// The errors of the request, and [`Error::Invalid`] if the answer is not
/// an overview.
pub async fn devices(session: &Session) -> Result<Vec<Device>, Error> {
    parse(&session.get(OVERVIEW).await?)
}

/// The devices of an overview, ordered by name.
///
/// # Errors
///
/// [`Error::Invalid`] if `json` is not an overview.
pub fn parse(json: &str) -> Result<Vec<Device>, Error> {
    let overview: Overview = serde_json::from_str(json).map_err(|e| Error::Invalid {
        what: "the Smart Home API",
        reason: e.to_string(),
    })?;
    let mut devices: Vec<Device> = overview
        .devices
        .into_iter()
        .map(|device| Device {
            units: device
                .unit_uids
                .iter()
                .filter_map(|uid| overview.units.iter().find(|u| &u.uid == uid))
                .map(RawUnit::summary)
                .collect(),
            ain: device.ain,
            name: device.name,
            product: device.product_name,
            category: device.product_category,
            firmware: device.firmware_version,
            connected: device.is_connected,
            battery: device.battery_value,
            battery_low: device.is_battery_low,
        })
        .collect();
    devices.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.ain.cmp(&b.ain)));
    Ok(devices)
}

#[derive(Debug, Deserialize)]
struct Overview {
    #[serde(default)]
    devices: Vec<RawDevice>,
    #[serde(default)]
    units: Vec<RawUnit>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawDevice {
    #[serde(default)]
    ain: String,
    #[serde(default)]
    name: String,
    product_name: Option<String>,
    product_category: Option<String>,
    firmware_version: Option<String>,
    #[serde(default)]
    is_connected: bool,
    battery_value: Option<u8>,
    is_battery_low: Option<bool>,
    #[serde(default)]
    unit_uids: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawUnit {
    #[serde(rename = "UID")]
    uid: String,
    #[serde(default)]
    ain: String,
    #[serde(default)]
    name: String,
    unit_type: Option<String>,
    #[serde(default)]
    interfaces: Interfaces,
}

impl RawUnit {
    fn summary(&self) -> Unit {
        let i = &self.interfaces;
        Unit {
            ain: self.ain.clone(),
            name: self.name.clone(),
            kind: self.unit_type.clone(),
            on: valid(i.on_off.as_ref()).and_then(|v| v.active),
            temperature: valid(i.temperature.as_ref()).and_then(|v| v.celsius),
            target: valid(i.thermostat.as_ref())
                .and_then(|v| v.set_point_temperature.as_ref())
                .and_then(SetPoint::target),
            power: valid(i.multimeter.as_ref())
                .and_then(|v| v.power)
                .map(|milliwatts| f64::from(milliwatts) / 1000.0),
            energy: valid(i.multimeter.as_ref()).and_then(|v| v.energy),
            voltage: valid(i.multimeter.as_ref())
                .and_then(|v| v.voltage)
                .map(|millivolts| f64::from(millivolts) / 1000.0),
            level: valid(i.level_control.as_ref()).and_then(|v| v.level),
            humidity: valid(i.humidity.as_ref()).and_then(|v| v.relative_humidity),
            alerts: valid(i.alert.as_ref())
                .map_or_default(|v| v.alerts.iter().filter(|a| *a != "none").cloned().collect()),
        }
    }
}

#[derive(Debug, Default, Deserialize)]
struct Interfaces {
    #[serde(rename = "onOffInterface")]
    on_off: Option<Interface<OnOff>>,
    #[serde(rename = "temperatureInterface")]
    temperature: Option<Interface<Temperature>>,
    #[serde(rename = "thermostatInterface")]
    thermostat: Option<Interface<Thermostat>>,
    #[serde(rename = "multimeterInterface")]
    multimeter: Option<Interface<Multimeter>>,
    #[serde(rename = "levelControlInterface")]
    level_control: Option<Interface<Level>>,
    #[serde(rename = "humidityInterface")]
    humidity: Option<Interface<Humidity>>,
    #[serde(rename = "alertInterface")]
    alert: Option<Interface<Alert>>,
}

/// An interface with its state: `valid`, `unknown` or `notConnected`.
#[derive(Debug, Deserialize)]
struct Interface<T> {
    state: Option<String>,
    #[serde(flatten)]
    values: T,
}

/// The values of an interface, if they are valid.
fn valid<T>(interface: Option<&Interface<T>>) -> Option<&T> {
    interface
        .filter(|i| i.state.as_deref() == Some("valid"))
        .map(|i| &i.values)
}

#[derive(Debug, Deserialize)]
struct OnOff {
    active: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct Temperature {
    celsius: Option<f64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Thermostat {
    set_point_temperature: Option<SetPoint>,
}

#[derive(Debug, Deserialize)]
struct SetPoint {
    mode: String,
    celsius: Option<f64>,
}

impl SetPoint {
    fn target(&self) -> Option<Target> {
        match (self.mode.as_str(), self.celsius) {
            ("temperature", Some(celsius)) => Some(Target::Celsius(celsius)),
            (mode @ ("on" | "off"), _) => Some(Target::Mode(mode.to_owned())),
            _ => None,
        }
    }
}

#[derive(Debug, Deserialize)]
struct Multimeter {
    /// In mW.
    power: Option<i32>,
    /// In Wh.
    energy: Option<u64>,
    /// In mV.
    voltage: Option<i32>,
}

#[derive(Debug, Deserialize)]
struct Level {
    level: Option<u8>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Humidity {
    relative_humidity: Option<u8>,
}

#[derive(Debug, Deserialize)]
struct Alert {
    #[serde(default)]
    alerts: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/fritzbox-7690/8.25/api/v0/smarthome/overview.json"
    ));

    #[test]
    fn reads_devices_and_their_units() {
        let devices = parse(FIXTURE).unwrap();
        let names: Vec<_> = devices.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, ["device-1", "device-2", "device-3", "device-4"]);

        let plug = &devices[0];
        assert_eq!(plug.product.as_deref(), Some("FRITZ!Smart Energy 200"));
        assert_eq!(plug.category.as_deref(), Some("socket"));
        assert!(plug.connected);
        assert_eq!(plug.battery, None);
        assert_eq!(
            plug.units,
            [Unit {
                ain: "11630 0000001".to_owned(),
                name: "device-1".to_owned(),
                kind: Some("avmPlugSocket".to_owned()),
                on: Some(true),
                temperature: Some(21.5),
                power: Some(23.45),
                energy: Some(1053),
                voltage: Some(228.055),
                ..Unit::default()
            }]
        );

        let thermostat = &devices[1];
        assert_eq!(thermostat.battery, Some(80));
        assert_eq!(thermostat.battery_low, Some(false));
        assert_eq!(thermostat.units[0].target, Some(Target::Celsius(21.0)));
        assert_eq!(thermostat.units[0].temperature, Some(20.5));

        let lamp = &devices[2];
        assert_eq!(lamp.units[0].on, Some(false));
        assert_eq!(lamp.units[0].level, Some(25));

        // Not connected: the values of interfaces that are not valid are
        // left out.
        let sensor = &devices[3];
        assert!(!sensor.connected);
        assert_eq!(sensor.units[0].temperature, None);
        assert_eq!(sensor.units[0].humidity, Some(55));
        assert_eq!(sensor.units[0].alerts, ["open"]);
    }

    #[test]
    fn prints_compact_json() {
        let devices = parse(FIXTURE).unwrap();
        let json = serde_json::to_value(&devices[1].units[0]).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "ain": "09995 0000002",
                "name": "device-2",
                "kind": "avmThermostat",
                "temperature": 20.5,
                "target": 21.0,
            })
        );
    }

    #[test]
    fn reads_thermostat_modes() {
        let target = |mode: &str, celsius| {
            SetPoint {
                mode: mode.to_owned(),
                celsius,
            }
            .target()
        };
        assert_eq!(target("on", None), Some(Target::Mode("on".to_owned())));
        assert_eq!(
            target("off", Some(8.0)),
            Some(Target::Mode("off".to_owned()))
        );
        assert_eq!(target("temperature", None), None);
        assert_eq!(target("unknown", Some(8.0)), None);
    }

    #[test]
    fn tolerates_missing_parts() {
        let devices =
            parse(r#"{"devices": [{"name": "x", "unitUids": ["missing"]}], "units": []}"#).unwrap();
        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].units, []);
        assert_eq!(parse("{}").unwrap(), []);
    }

    #[test]
    fn rejects_other_answers() {
        let error = parse("[1]").unwrap_err().to_string();
        assert!(
            error.starts_with("invalid answer from the Smart Home API: "),
            "{error}"
        );
    }
}
