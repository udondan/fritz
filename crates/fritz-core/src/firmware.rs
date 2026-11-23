//! The firmware of a device: the installed version, the update the device
//! offers, automatic updates and the last update, read with the getters of
//! `DeviceInfo` and `UserInterface` only.
//!
//! Checking for a new version (`X_AVM-DE_CheckUpdate`) is a change and not
//! done here. The download URL of an update is never read into the result,
//! since it may carry tokens.

use serde::Serialize;

use crate::generic::Error;
use crate::read::{Reader, Values};

/// The service with the update state.
const SERVICE: &str = "UserInterface";

/// Whether the device offers an update.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    /// The device offers an update.
    Available,
    /// The device says that there is no update.
    #[serde(rename = "none")]
    NoUpdate,
    /// The device does not tell, for example because its last check failed
    /// or the user may not read the state.
    #[default]
    Unknown,
    /// The device has no service for updates, like some repeaters.
    Unsupported,
}

/// The firmware and its updates. Values the device does not report are
/// `None`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Firmware {
    pub state: State,
    /// The installed version, like `285.08.25`.
    pub installed: Option<String>,
    /// The kind of build of the installed version, like `Release`.
    pub build_type: Option<String>,
    /// The version the device offers.
    pub offered: Option<String>,
    /// The page with the changes of the offered version.
    pub info_url: Option<String>,
    /// The state of the update process, like `Stopped`, `UpdateAvailable`
    /// or `Error`.
    pub update_state: Option<String>,
    /// What the device does with new versions by itself: `off`, `check`
    /// (only notifies), `important` or `all`.
    pub auto_update: Option<String>,
    /// When the last update was installed.
    pub last_update_time: Option<String>,
    /// Whether the last update worked: `succeeded` or `failed`.
    pub last_update_result: Option<String>,
    /// The version before the last update.
    pub previous_version: Option<String>,
}

/// Reads the firmware state.
///
/// # Errors
///
/// The errors of reading other than missing parts, see
/// [`Reader::try_read`].
pub async fn firmware(reader: &mut Reader) -> Result<Firmware, Error> {
    let info = reader.try_read("DeviceInfo1", "GetInfo", &[]).await?;
    let info = info.unwrap_or_default();
    if reader.services(SERVICE).is_empty() {
        return Ok(Firmware {
            state: State::Unsupported,
            installed: info.text("NewSoftwareVersion"),
            ..Firmware::default()
        });
    }
    let interface = reader.try_read("UserInterface1", "GetInfo", &[]).await?;
    let getter = reader.try_read("UserInterface1", "X_AVM-DE_GetInfo", &[]);
    let extended = getter.await?.unwrap_or_default();
    Ok(build(&info, interface.as_ref(), &extended))
}

/// The firmware from the answers of `DeviceInfo:GetInfo`,
/// `UserInterface:GetInfo`, if the device gave one, and
/// `UserInterface:X_AVM-DE_GetInfo`.
fn build(info: &Values, interface: Option<&Values>, extended: &Values) -> Firmware {
    let state = interface.map_or(State::Unknown, state);
    let interface = interface.cloned().unwrap_or_default();
    let installed = info
        .text("NewSoftwareVersion")
        .or_else(|| extended.text("NewX_AVM-DE_CurrentFwVersion"));
    // Devices that never updated report a time in the year 1.
    let time = extended.text("NewX_AVM-DE_UpdateTime");
    let result = extended.text("NewX_AVM-DE_UpdateSuccessful");
    // Like `285.08.22,129542`: the version and its build number.
    let previous = extended.text("NewX_AVM-DE_LastFwVersion");
    Firmware {
        state,
        installed,
        build_type: interface.text("NewX_AVM-DE_BuildType"),
        offered: interface.text("NewX_AVM-DE_Version"),
        info_url: interface.text("NewX_AVM-DE_InfoURL"),
        update_state: interface.text("NewX_AVM-DE_UpdateState"),
        auto_update: extended.text("NewX_AVM-DE_AutoUpdateMode"),
        last_update_time: time.filter(|t| !t.starts_with("0001-")),
        last_update_result: result.filter(|r| r != "unknown"),
        previous_version: previous.map(|v| v.split(',').next().unwrap_or_default().to_owned()),
    }
}

/// Whether an update is offered, by the flag `NewUpgradeAvailable` and the
/// update state. A failed check makes it unknown, never `none`.
fn state(interface: &Values) -> State {
    let flag = interface.flag("NewUpgradeAvailable");
    let update = interface.get("NewX_AVM-DE_UpdateState");
    if flag == Some(true) || update == Some("UpdateAvailable") {
        State::Available
    } else if matches!(update, Some("Error" | "Unknown")) {
        State::Unknown
    } else if flag == Some(false) || update == Some("NoUpdate") {
        State::NoUpdate
    } else {
        State::Unknown
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn values(pairs: &[(&str, &str)]) -> Values {
        Values(
            pairs
                .iter()
                .map(|(n, v)| ((*n).to_owned(), (*v).to_owned()))
                .collect(),
        )
    }

    #[test]
    fn tells_the_state() {
        for (flag, update, expected) in [
            ("1", "Stopped", State::Available),
            ("0", "UpdateAvailable", State::Available),
            ("0", "Stopped", State::NoUpdate),
            ("", "NoUpdate", State::NoUpdate),
            ("0", "Error", State::Unknown),
            ("", "Unknown", State::Unknown),
            ("", "Stopped", State::Unknown),
            ("", "", State::Unknown),
        ] {
            let interface = values(&[
                ("NewUpgradeAvailable", flag),
                ("NewX_AVM-DE_UpdateState", update),
            ]);
            assert_eq!(state(&interface), expected, "{flag} {update}");
        }
    }

    #[test]
    fn reports_missing_answers_as_unknown() {
        let firmware = build(&Values::default(), None, &Values::default());
        assert_eq!(firmware, Firmware::default());
        assert_eq!(firmware.state, State::Unknown);
        let json = serde_json::to_value(&firmware).unwrap();
        assert_eq!(json["state"], "unknown");
        // All fields, with explicit nulls.
        assert_eq!(json.as_object().unwrap().len(), 10);
        assert!(json["offered"].is_null());
    }

    #[test]
    fn leaves_out_placeholders() {
        let extended = values(&[
            ("NewX_AVM-DE_CurrentFwVersion", "134.07.32"),
            ("NewX_AVM-DE_UpdateTime", "0001-01-01T00:00:00"),
            ("NewX_AVM-DE_UpdateSuccessful", "unknown"),
            ("NewX_AVM-DE_LastFwVersion", "134.07.31"),
        ]);
        let firmware = build(&Values::default(), None, &extended);
        assert_eq!(firmware.installed.as_deref(), Some("134.07.32"));
        assert_eq!(firmware.last_update_time, None);
        assert_eq!(firmware.last_update_result, None);
        assert_eq!(firmware.previous_version.as_deref(), Some("134.07.31"));
    }

    #[test]
    fn names_the_states_in_json() {
        let names: Vec<String> = [
            State::Available,
            State::NoUpdate,
            State::Unknown,
            State::Unsupported,
        ]
        .iter()
        .map(|s| serde_json::to_string(s).unwrap())
        .collect();
        assert_eq!(
            names,
            [
                "\"available\"",
                "\"none\"",
                "\"unknown\"",
                "\"unsupported\""
            ]
        );
    }
}
