//! The powerline adapters a device knows, read with the getters of
//! `X_AVM-DE_Homeplug` only.
//!
//! Updating an adapter (`DeviceDoUpdate`) is a change and not done here.
//! Names are untrusted strings from the device.

use serde::Serialize;

use crate::generic::Error;
use crate::read::{Reader, Values};

/// The service with the powerline adapters.
const SERVICE: &str = "X_AVM-DE_Homeplug";

/// The most adapters read.
pub const MAX_ADAPTERS: u32 = 100;

/// The powerline adapters of a device.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Powerline {
    /// Whether the device has the service for powerline adapters.
    pub supported: bool,
    pub adapters: Vec<Adapter>,
}

/// A powerline adapter. Values the device does not report are `None`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Adapter {
    pub name: Option<String>,
    pub model: Option<String>,
    pub mac: Option<String>,
    /// Whether the adapter is connected.
    pub active: bool,
    /// Whether the device offers a firmware update for the adapter.
    pub update_available: Option<bool>,
    /// Whether the last update of the adapter worked: `succeeded` or
    /// `failed`.
    pub update_result: Option<String>,
}

/// Reads the powerline adapters.
///
/// # Errors
///
/// The errors of reading other than missing parts, see
/// [`Reader::try_read`].
pub async fn powerline(reader: &mut Reader) -> Result<Powerline, Error> {
    if reader.services(SERVICE).is_empty() {
        return Ok(Powerline::default());
    }
    let entries = reader
        .entries(
            "X_AVM-DE_Homeplug1",
            ("GetNumberOfDeviceEntries", "NewNumberOfEntries"),
            ("GetGenericDeviceEntry", "NewIndex"),
            MAX_ADAPTERS,
        )
        .await?
        .map_or_else(Vec::new, |(_, entries)| entries);
    Ok(Powerline {
        supported: true,
        adapters: entries.iter().map(adapter).collect(),
    })
}

/// An adapter from the answer of `GetGenericDeviceEntry`.
fn adapter(entry: &Values) -> Adapter {
    Adapter {
        name: entry.text("NewName"),
        model: entry.text("NewModel"),
        mac: entry.text("NewMACAddress"),
        active: entry.flag("NewActive").unwrap_or_default(),
        update_available: entry.flag("NewUpdateAvailable"),
        update_result: entry.text("NewUpdateSuccessful").filter(|r| r != "unknown"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_an_entry() {
        let entry = Values(
            [
                ("NewMACAddress", "AA:BB:CC:00:00:01"),
                ("NewActive", "1"),
                ("NewName", "Office"),
                ("NewModel", "FRITZ!Powerline 1260"),
                ("NewUpdateAvailable", "0"),
                ("NewUpdateSuccessful", "unknown"),
            ]
            .iter()
            .map(|(n, v)| ((*n).to_owned(), (*v).to_owned()))
            .collect(),
        );
        assert_eq!(
            adapter(&entry),
            Adapter {
                name: Some("Office".to_owned()),
                model: Some("FRITZ!Powerline 1260".to_owned()),
                mac: Some("AA:BB:CC:00:00:01".to_owned()),
                active: true,
                update_available: Some(false),
                update_result: None,
            }
        );
        let json = serde_json::to_value(adapter(&Values::default())).unwrap();
        // All fields, with explicit nulls.
        assert_eq!(json.as_object().unwrap().len(), 6);
        assert!(json["name"].is_null());
        assert_eq!(json["active"], false);
    }
}
