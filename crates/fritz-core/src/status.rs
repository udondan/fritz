//! The state of a device, read through actions that only read: an
//! overview, the hosts, the wireless networks and the internet connection.
//!
//! Parts the device lacks, or that the user may not read, are left out
//! instead of failing the whole read.

use serde::Serialize;

use crate::generic::Error;
use crate::read::{Reader, Values};

/// The most hosts read, as a guard against a device that never ends the
/// list.
pub const MAX_HOSTS: u32 = 1000;

/// The device, its connection and the rights of the user.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Overview {
    pub model: Option<String>,
    pub firmware: Option<String>,
    /// Seconds since the device started.
    pub uptime: Option<u64>,
    pub update_available: Option<bool>,
    /// Whether changes need a confirmation on the device, like a button
    /// press.
    pub two_factor: Option<bool>,
    /// What the user may do, like `BoxAdmin: readwrite`.
    pub rights: Vec<Right>,
    pub internet: Option<Connection>,
}

/// An area of the device and the access of the user to it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Right {
    pub area: String,
    pub access: String,
}

/// The connection to the internet.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Connection {
    /// Like `DSL`, `Ethernet` or `X_AVM-DE_Mobile`.
    pub access_type: Option<String>,
    /// The physical link, like `Up`.
    pub link: Option<String>,
    /// Like `Connected`.
    pub status: Option<String>,
    pub external_address: Option<String>,
    /// Seconds since the connection was made.
    pub uptime: Option<u64>,
}

/// The internet connection in detail.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Internet {
    #[serde(flatten)]
    pub connection: Connection,
    pub provider: Option<String>,
    pub dns_servers: Vec<String>,
    /// The speed of the physical link, in kbit/s.
    pub downstream_max: Option<u64>,
    pub upstream_max: Option<u64>,
    pub dsl: Option<Dsl>,
}

/// The DSL line.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Dsl {
    pub status: Option<String>,
    /// The synchronized and the attainable rates, in kbit/s.
    pub downstream_rate: Option<u64>,
    pub upstream_rate: Option<u64>,
    pub downstream_max: Option<u64>,
    pub upstream_max: Option<u64>,
    /// In dB.
    pub downstream_noise_margin: Option<f64>,
    pub upstream_noise_margin: Option<f64>,
    pub downstream_attenuation: Option<f64>,
    pub upstream_attenuation: Option<f64>,
}

/// A host the device knows, active or not.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Host {
    pub name: Option<String>,
    pub ip: Option<String>,
    pub mac: Option<String>,
    pub active: bool,
    /// Like `Ethernet` or `802.11`.
    pub interface: Option<String>,
    /// Like `DHCP` or `Static`.
    pub address_source: Option<String>,
}

/// The hosts and how many the device reported.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Hosts {
    /// The number of entries the device reported. More than the hosts if
    /// it stopped answering before.
    pub reported: u32,
    pub hosts: Vec<Host>,
}

/// A wireless network.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Wlan {
    /// The service, like `WLANConfiguration1`.
    pub service: String,
    pub ssid: Option<String>,
    /// Like `2.4 GHz`.
    pub band: Option<String>,
    pub channel: Option<u32>,
    /// Like `ax` or `be`.
    pub standard: Option<String>,
    pub enabled: Option<bool>,
    /// Like `Up` or `Disabled`.
    pub status: Option<String>,
    /// The number of connected clients.
    pub clients: Option<u32>,
}

/// Reads the overview.
///
/// # Errors
///
/// The errors of reading other than missing parts, see
/// [`Reader::try_read`].
pub async fn overview(reader: &mut Reader) -> Result<Overview, Error> {
    let info = reader.try_read("DeviceInfo1", "GetInfo", &[]).await?;
    let interface = reader.try_read("UserInterface1", "GetInfo", &[]).await?;
    let auth = reader.try_read("X_AVM-DE_Auth1", "GetInfo", &[]).await?;
    let user = reader
        .try_read("LANConfigSecurity1", "X_AVM-DE_GetCurrentUser", &[])
        .await?;
    let rights = user
        .and_then(|u| u.text("NewX_AVM-DE_CurrentUserRights"))
        .map_or_else(Vec::new, |xml| rights(&xml));
    let info = info.unwrap_or_default();
    Ok(Overview {
        model: info.text("NewModelName"),
        firmware: info.text("NewSoftwareVersion"),
        uptime: info.number("NewUpTime"),
        update_available: interface.and_then(|i| i.flag("NewUpgradeAvailable")),
        two_factor: auth.and_then(|a| a.flag("NewEnabled")),
        rights,
        internet: Some(connection(reader).await?.0).filter(|c| *c != Connection::default()),
    })
}

/// Parses rights like
/// `<rights><path>BoxAdmin</path><access>readwrite</access></rights>`.
fn rights(xml: &str) -> Vec<Right> {
    let Ok(document) = roxmltree::Document::parse(xml) else {
        return Vec::new();
    };
    let mut rights = Vec::new();
    let mut area = None;
    for node in document
        .root_element()
        .children()
        .filter(roxmltree::Node::is_element)
    {
        let text = node.text().unwrap_or_default().trim().to_owned();
        match node.tag_name().name() {
            "path" => area = Some(text),
            "access" => {
                if let Some(area) = area.take() {
                    rights.push(Right { area, access: text });
                }
            }
            _ => {}
        }
    }
    rights
}

/// Reads the connection, and the answer of the connection service for
/// the details.
async fn connection(reader: &mut Reader) -> Result<(Connection, Values), Error> {
    let common = reader
        .try_read("WANCommonInterfaceConfig1", "GetCommonLinkProperties", &[])
        .await?
        .unwrap_or_default();
    // Connections over PPP, like most DSL lines, are in WANPPPConnection,
    // the others in WANIPConnection.
    let mut details = Values::default();
    for service in ["WANPPPConnection1", "WANIPConnection1"] {
        if let Some(values) = reader.try_read(service, "GetInfo", &[]).await? {
            details = values;
            break;
        }
    }
    let connection = Connection {
        access_type: common.text("NewWANAccessType"),
        link: common.text("NewPhysicalLinkStatus"),
        status: details.text("NewConnectionStatus"),
        external_address: details.text("NewExternalIPAddress"),
        uptime: details.number("NewUptime"),
    };
    let details = Values(common.0.into_iter().chain(details.0).collect());
    Ok((connection, details))
}

/// Reads the internet connection.
///
/// # Errors
///
/// The errors of reading other than missing parts, see
/// [`Reader::try_read`].
pub async fn internet(reader: &mut Reader) -> Result<Internet, Error> {
    let (connection, details) = connection(reader).await?;
    let provider = reader
        .try_read(
            "WANCommonInterfaceConfig1",
            "X_AVM-DE_GetActiveProvider",
            &[],
        )
        .await?;
    let dsl = reader
        .try_read("WANDSLInterfaceConfig1", "GetInfo", &[])
        .await?;
    let kbits = |name| details.number::<u64>(name).map(|b| b / 1000);
    Ok(Internet {
        provider: provider.and_then(|p| p.text("NewX_AVM-DE_Provider")),
        dns_servers: details.get("NewDNSServers").map_or_else(Vec::new, |s| {
            s.split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .collect()
        }),
        downstream_max: kbits("NewLayer1DownstreamMaxBitRate"),
        upstream_max: kbits("NewLayer1UpstreamMaxBitRate"),
        dsl: dsl
            .filter(|_| connection.access_type.as_deref() == Some("DSL"))
            .map(|d| dsl_line(&d)),
        connection,
    })
}

/// The DSL line from the answer of `WANDSLInterfaceConfig:GetInfo`, whose
/// noise margins and attenuations are in tenths of a dB.
fn dsl_line(values: &Values) -> Dsl {
    let decibels = |name| values.number::<i64>(name).map(tenths);
    Dsl {
        status: values.text("NewStatus"),
        downstream_rate: values.number("NewDownstreamCurrRate"),
        upstream_rate: values.number("NewUpstreamCurrRate"),
        downstream_max: values.number("NewDownstreamMaxRate"),
        upstream_max: values.number("NewUpstreamMaxRate"),
        downstream_noise_margin: decibels("NewDownstreamNoiseMargin"),
        upstream_noise_margin: decibels("NewUpstreamNoiseMargin"),
        downstream_attenuation: decibels("NewDownstreamAttenuation"),
        upstream_attenuation: decibels("NewUpstreamAttenuation"),
    }
}

/// A value in tenths as a decimal, like 70 as 7.0.
#[expect(clippy::cast_precision_loss, reason = "tenths of a dB are small")]
fn tenths(value: i64) -> f64 {
    value as f64 / 10.0
}

/// Reads the hosts, one call per host, up to [`MAX_HOSTS`]. Stops early at
/// the first index the device has no entry for.
///
/// # Errors
///
/// The errors of reading other than missing parts, see
/// [`Reader::try_read`].
pub async fn hosts(reader: &mut Reader) -> Result<Hosts, Error> {
    let Some(count) = reader
        .try_read("Hosts1", "GetHostNumberOfEntries", &[])
        .await?
    else {
        return Ok(Hosts::default());
    };
    let reported: u32 = count.number("NewHostNumberOfEntries").unwrap_or_default();
    let mut hosts = Vec::new();
    for index in 0..reported.min(MAX_HOSTS) {
        let index = index.to_string();
        let arguments = [("NewIndex", index.as_str())];
        let Some(entry) = reader
            .try_read("Hosts1", "GetGenericHostEntry", &arguments)
            .await?
        else {
            break;
        };
        hosts.push(Host {
            name: entry.text("NewHostName"),
            ip: entry.text("NewIPAddress"),
            mac: entry.text("NewMACAddress"),
            active: entry.flag("NewActive").unwrap_or_default(),
            interface: entry.text("NewInterfaceType"),
            address_source: entry.text("NewAddressSource"),
        });
    }
    Ok(Hosts { reported, hosts })
}

/// Reads the wireless networks.
///
/// # Errors
///
/// The errors of reading other than missing parts, see
/// [`Reader::try_read`].
pub async fn wlans(reader: &mut Reader) -> Result<Vec<Wlan>, Error> {
    let mut wlans = Vec::new();
    for service in reader.services("WLANConfiguration") {
        let Some(info) = reader.try_read(&service, "GetInfo", &[]).await? else {
            continue;
        };
        let clients = reader
            .try_read(&service, "GetTotalAssociations", &[])
            .await?
            .and_then(|v| v.number("NewTotalAssociations"));
        wlans.push(Wlan {
            band: info.get("NewX_AVM-DE_FrequencyBand").and_then(band),
            ssid: info.text("NewSSID"),
            channel: info.number("NewChannel"),
            standard: info.text("NewStandard"),
            enabled: info.flag("NewEnable"),
            status: info.text("NewStatus"),
            clients,
            service,
        });
    }
    Ok(wlans)
}

/// The frequency band in MHz as text, like `2400` as `2.4 GHz`.
fn band(megahertz: &str) -> Option<String> {
    match megahertz {
        "2400" => Some("2.4 GHz".to_owned()),
        "5000" => Some("5 GHz".to_owned()),
        "6000" => Some("6 GHz".to_owned()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_rights() {
        let xml = "<rights><path>BoxAdmin</path><access>readwrite</access>\
                   <access>orphan</access><other/><path>NAS</path><access>read</access></rights>";
        assert_eq!(
            rights(xml),
            [
                Right {
                    area: "BoxAdmin".to_owned(),
                    access: "readwrite".to_owned()
                },
                Right {
                    area: "NAS".to_owned(),
                    access: "read".to_owned()
                },
            ]
        );
        assert_eq!(rights("<rights"), []);
    }

    #[test]
    fn names_bands() {
        assert_eq!(band("2400").as_deref(), Some("2.4 GHz"));
        assert_eq!(band("5000").as_deref(), Some("5 GHz"));
        assert_eq!(band("6000").as_deref(), Some("6 GHz"));
        assert_eq!(band("unknown"), None);
    }
}
