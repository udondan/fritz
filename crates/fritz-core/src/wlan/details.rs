//! The security settings, the radio, WPS, the night time switch-off and the
//! statistics of a wireless network, with hints on weak settings.
//!
//! Only plain getters are called. `GetSecurityKeys` and other getters that
//! return keys are never called, so no passphrase is read.

use std::fmt;

use serde::Serialize;

use crate::generic::Error;
use crate::read::{Reader, Values};

/// The details of a wireless network. A field is `None` if the device does
/// not report it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Details {
    pub security: Security,
    pub radio: Radio,
    pub wps: Wps,
    pub night: Night,
    pub statistics: Statistics,
    /// Weak settings, as hints rather than errors.
    pub hints: Vec<Hint>,
}

/// How a network is secured.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Security {
    /// As the device names it, like `11i` or `11iandWPA3`.
    pub beacon_type: Option<String>,
    /// The beacon type as text, like `WPA2/WPA3 mixed, AES`.
    pub encryption: Option<String>,
    /// The beacon types the device offers.
    pub possible_beacon_types: Option<Vec<String>>,
    /// The encryption of the beacon type `Basic`, like `WEPEncryption`.
    pub basic_encryption: Option<String>,
    /// The authentication of the beacon type `Basic`.
    pub basic_authentication: Option<String>,
    /// Whether the SSID is not advertised.
    pub hidden: Option<bool>,
    /// Whether only known MAC addresses may connect.
    pub mac_filter: Option<bool>,
    /// Whether the clients cannot reach each other, as on a guest network.
    pub user_isolation: Option<bool>,
}

/// The radio of a network.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Radio {
    pub channel: Option<u32>,
    /// Whether the device chooses the channel.
    pub auto_channel: Option<bool>,
    /// The channels the device offers.
    pub possible_channels: Option<Vec<u32>>,
    pub bssid: Option<String>,
    /// Like `Auto`.
    pub max_bit_rate: Option<String>,
    /// Whether the network is optimized for IPTV.
    pub iptv_optimized: Option<bool>,
}

/// Wi-Fi Protected Setup.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Wps {
    /// Whether WPS is on, by its status; `None` if the status, like
    /// `err_common`, does not tell.
    pub enabled: Option<bool>,
    /// Like `stop` or `pbc`.
    pub mode: Option<String>,
    /// Like `off`, `inactive` or `active`.
    pub status: Option<String>,
}

/// The night time switch-off of the wireless function.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Night {
    /// The schedule as the device reports it; `None` without one.
    pub schedule: Option<String>,
    /// Whether the wireless function stays on while clients are connected.
    pub no_forced_off: Option<bool>,
}

/// The packets since the last restart.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Statistics {
    pub packets_sent: Option<u64>,
    pub packets_received: Option<u64>,
}

/// A weak setting of a network.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Hint {
    /// No encryption.
    OpenNetwork,
    /// WEP, which is broken.
    Wep,
    /// WPA with TKIP, which is broken, is allowed.
    TkipAllowed,
    WpsEnabled,
    HiddenSsid,
}

impl fmt::Display for Hint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::OpenNetwork => "open network",
            Self::Wep => "WEP",
            Self::TkipAllowed => "TKIP allowed",
            Self::WpsEnabled => "WPS enabled",
            Self::HiddenSsid => "hidden SSID",
        })
    }
}

/// Reads the details of the network of `service`, whose answers to
/// `GetInfo` and `X_AVM-DE_GetWLANExtInfo` are `info` and `ext`. Getters the
/// device lacks leave their fields `None`.
///
/// # Errors
///
/// The errors of [`Reader::try_read`].
pub(super) async fn read(
    reader: &mut Reader,
    service: &str,
    info: &Values,
    ext: Option<&Values>,
) -> Result<Details, Error> {
    let channel = reader.try_read(service, "GetChannelInfo", &[]).await?;
    let advertised = reader
        .try_read(service, "GetBeaconAdvertisement", &[])
        .await?
        .and_then(|v| v.flag("NewBeaconAdvertisementEnabled"));
    let wps = reader.try_read(service, "X_AVM-DE_GetWPSInfo", &[]).await?;
    let night = reader
        .try_read(service, "X_AVM-DE_GetNightControl", &[])
        .await?;
    let mut statistics = reader.try_read(service, "GetStatistics", &[]).await?;
    if statistics.is_none() {
        statistics = reader.try_read(service, "GetPacketStatistics", &[]).await?;
    }
    let iptv = reader
        .try_read(service, "X_AVM-DE_GetIPTVOptimized", &[])
        .await?
        .and_then(|v| v.flag("NewX_AVM-DE_IPTVoptimize"));

    let beacon_type = info.text("NewBeaconType");
    let basic_encryption = info.text("NewBasicEncryptionModes");
    let security = Security {
        encryption: beacon_type
            .as_deref()
            .and_then(|b| encryption(b, basic_encryption.as_deref()))
            .map(str::to_owned),
        possible_beacon_types: info
            .text("NewX_AVM-DE_PossibleBeaconTypes")
            .map(|t| list(&t)),
        basic_authentication: info.text("NewBasicAuthenticationMode"),
        hidden: advertised.map(|a| !a),
        mac_filter: info.flag("NewMACAddressControlEnabled"),
        user_isolation: ext.and_then(|e| e.flag("NewX_AVM-DE_UserIsolation")),
        beacon_type,
        basic_encryption,
    };
    let radio = Radio {
        channel: channel
            .as_ref()
            .and_then(|c| c.number("NewChannel"))
            .or_else(|| info.number("NewChannel")),
        auto_channel: channel
            .as_ref()
            .and_then(|c| c.flag("NewX_AVM-DE_AutoChannelEnabled")),
        possible_channels: channel
            .as_ref()
            .and_then(|c| c.text("NewPossibleChannels"))
            .map(|c| list(&c).iter().filter_map(|n| n.parse().ok()).collect()),
        bssid: info.text("NewBSSID"),
        max_bit_rate: info.text("NewMaxBitRate"),
        iptv_optimized: iptv,
    };
    let wps_status = wps.as_ref().and_then(|w| w.text("NewX_AVM-DE_WPSStatus"));
    let wps = Wps {
        enabled: wps_status.as_deref().and_then(wps_enabled),
        mode: wps.as_ref().and_then(|w| w.text("NewX_AVM-DE_WPSMode")),
        status: wps_status,
    };
    let night = Night {
        schedule: night.as_ref().and_then(|n| n.text("NewNightControl")),
        no_forced_off: night
            .as_ref()
            .and_then(|n| n.flag("NewNightTimeControlNoForcedOff")),
    };
    let statistics = Statistics {
        packets_sent: statistics
            .as_ref()
            .and_then(|s| s.number("NewTotalPacketsSent")),
        packets_received: statistics
            .as_ref()
            .and_then(|s| s.number("NewTotalPacketsReceived")),
    };
    let mut details = Details {
        security,
        radio,
        wps,
        night,
        statistics,
        hints: Vec::new(),
    };
    details.hints = hints(&details);
    Ok(details)
}

/// The beacon type as text, with the encryption of `Basic` beacons, like
/// `WEPEncryption`.
fn encryption(beacon_type: &str, basic: Option<&str>) -> Option<&'static str> {
    Some(match beacon_type {
        "Basic" if basic.is_some_and(|b| b.contains("WEP")) => "WEP",
        "None" | "Basic" => "none",
        "WPA" => "WPA, TKIP",
        "11i" => "WPA2, AES",
        "WPAand11i" => "WPA/WPA2 mixed, TKIP and AES",
        "11iandWPA3" => "WPA2/WPA3 mixed, AES",
        "WPA3" => "WPA3, AES",
        "OWE" => "Enhanced Open (OWE)",
        "OWETrans" => "Enhanced Open (OWE), transition mode",
        _ => return None,
    })
}

/// Whether WPS is on, by its status: `off` is off, `inactive` is on but not
/// running, `active` and `success` are on. Errors do not tell.
fn wps_enabled(status: &str) -> Option<bool> {
    match status {
        "off" => Some(false),
        "inactive" | "active" | "success" => Some(true),
        _ => None,
    }
}

/// The weak settings of a network.
fn hints(details: &Details) -> Vec<Hint> {
    let security = &details.security;
    let mut hints = Vec::new();
    match security.encryption.as_deref() {
        Some("none") => hints.push(Hint::OpenNetwork),
        Some("WEP") => hints.push(Hint::Wep),
        _ => {}
    }
    if matches!(security.beacon_type.as_deref(), Some("WPA" | "WPAand11i")) {
        hints.push(Hint::TkipAllowed);
    }
    if details.wps.enabled == Some(true) {
        hints.push(Hint::WpsEnabled);
    }
    if security.hidden == Some(true) {
        hints.push(Hint::HiddenSsid);
    }
    hints
}

/// The items of a comma-separated list, without empty ones.
fn list(text: &str) -> Vec<String> {
    text.split(',')
        .map(str::trim)
        .filter(|i| !i.is_empty())
        .map(str::to_owned)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_the_encryption() {
        assert_eq!(encryption("None", None), Some("none"));
        assert_eq!(encryption("Basic", Some("WEPEncryption")), Some("WEP"));
        assert_eq!(encryption("Basic", Some("None")), Some("none"));
        assert_eq!(encryption("WPA", None), Some("WPA, TKIP"));
        assert_eq!(encryption("11i", None), Some("WPA2, AES"));
        assert_eq!(
            encryption("WPAand11i", None),
            Some("WPA/WPA2 mixed, TKIP and AES")
        );
        assert_eq!(encryption("11iandWPA3", None), Some("WPA2/WPA3 mixed, AES"));
        assert_eq!(encryption("WPA3", None), Some("WPA3, AES"));
        assert_eq!(encryption("OWE", None), Some("Enhanced Open (OWE)"));
        assert_eq!(
            encryption("OWETrans", None),
            Some("Enhanced Open (OWE), transition mode")
        );
        assert_eq!(encryption("Other", None), None);
    }

    #[test]
    fn tells_whether_wps_is_on() {
        assert_eq!(wps_enabled("off"), Some(false));
        assert_eq!(wps_enabled("inactive"), Some(true));
        assert_eq!(wps_enabled("active"), Some(true));
        assert_eq!(wps_enabled("success"), Some(true));
        assert_eq!(wps_enabled("err_common"), None);
    }

    #[test]
    fn finds_weak_settings() {
        let details = |beacon: &str, basic: Option<&str>, wps: Option<bool>, hidden| Details {
            security: Security {
                beacon_type: Some(beacon.to_owned()),
                encryption: encryption(beacon, basic).map(str::to_owned),
                hidden,
                ..Security::default()
            },
            wps: Wps {
                enabled: wps,
                ..Wps::default()
            },
            ..Details::default()
        };
        assert_eq!(
            hints(&details("11iandWPA3", None, Some(false), Some(false))),
            []
        );
        assert_eq!(hints(&details("11i", None, None, None)), []);
        assert_eq!(
            hints(&details("None", None, Some(true), Some(true))),
            [Hint::OpenNetwork, Hint::WpsEnabled, Hint::HiddenSsid]
        );
        assert_eq!(
            hints(&details("Basic", Some("WEPEncryption"), None, None)),
            [Hint::Wep]
        );
        assert_eq!(
            hints(&details("WPA", None, None, None)),
            [Hint::TkipAllowed]
        );
        assert_eq!(
            hints(&details("WPAand11i", None, None, None)),
            [Hint::TkipAllowed]
        );
        let text: Vec<String> = [
            Hint::OpenNetwork,
            Hint::Wep,
            Hint::TkipAllowed,
            Hint::WpsEnabled,
            Hint::HiddenSsid,
        ]
        .iter()
        .map(ToString::to_string)
        .collect();
        assert_eq!(
            text,
            [
                "open network",
                "WEP",
                "TKIP allowed",
                "WPS enabled",
                "hidden SSID"
            ]
        );
    }

    #[test]
    fn splits_lists() {
        assert_eq!(list("None, 11i,,WPA3"), ["None", "11i", "WPA3"]);
        assert_eq!(list(""), [] as [String; 0]);
    }
}
