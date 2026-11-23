//! The wireless networks of a device: their state, whether they are the
//! guest network, and, when asked for, their clients and details.
//!
//! Parts the device lacks, or that the user may not read, are left out
//! instead of failing the whole read.

use std::collections::HashMap;

use serde::Serialize;

use crate::generic::Error;
use crate::lists;
use crate::read::{Reader, Values};
use crate::status;

mod details;

pub use details::{Details, Hint, Night, Radio, Security, Statistics, Wps};

/// The most clients of a network read one call per client, as a guard
/// against a device that never ends the list.
pub const MAX_CLIENTS: u32 = 1000;

/// What to read besides the state of each network.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Views {
    /// The connected clients of each network.
    pub clients: bool,
    /// The security settings, radio, WPS, night time switch-off and
    /// statistics of each network, with hints on weak settings.
    pub details: bool,
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
    /// Whether the device reports this network as its guest network.
    pub guest: bool,
    /// The number of connected clients.
    pub client_count: Option<u32>,
    /// The connected clients, only if [asked for](Views::clients).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub clients: Option<Vec<Client>>,
    /// The details, only if [asked for](Views::details).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<Details>,
}

/// A network with the answers that its details are made of.
struct Read {
    wlan: Wlan,
    info: Values,
    ext: Option<Values>,
}

/// A client connected to a wireless network.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Client {
    /// The name in the host table.
    pub name: Option<String>,
    pub ip: Option<String>,
    pub mac: Option<String>,
    /// The signal strength, in percent.
    pub signal: Option<u8>,
    /// The speed of the connection, in Mbit/s.
    pub rate: Option<u32>,
}

/// A client in the list of the clients of all networks, with what tells
/// which network it is connected to.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Listed {
    client: Client,
    channel: Option<u32>,
    guest: Option<bool>,
}

/// Reads the wireless networks, and what `views` asks for.
///
/// # Errors
///
/// The errors of reading other than missing parts, see
/// [`Reader::try_read`], and [`Error::InvalidDocument`] for a list of
/// clients or a host list that cannot be parsed.
pub async fn wlans(reader: &mut Reader, views: Views) -> Result<Vec<Wlan>, Error> {
    let networks = networks(reader).await?;
    with_views(reader, networks, views).await
}

/// Like [`wlans`], only the networks that `network` names: by service, like
/// `WLANConfiguration2`, ignoring case, by its number, like `2`, or by
/// SSID. Several networks can share an SSID, one per band.
///
/// # Errors
///
/// The errors of [`wlans`].
pub async fn named(reader: &mut Reader, network: &str, views: Views) -> Result<Vec<Wlan>, Error> {
    let mut networks = networks(reader).await?;
    networks.retain(|n| is_named(&n.wlan, network));
    with_views(reader, networks, views).await
}

/// Whether `network` names `wlan`, see [`named`].
fn is_named(wlan: &Wlan, network: &str) -> bool {
    let number = wlan
        .service
        .get("WLANConfiguration".len()..)
        .filter(|_| wlan.service.starts_with("WLANConfiguration"));
    wlan.service.eq_ignore_ascii_case(network)
        || number == Some(network)
        || wlan.ssid.as_deref() == Some(network)
}

/// Reads the state of each network.
async fn networks(reader: &mut Reader) -> Result<Vec<Read>, Error> {
    let mut networks = Vec::new();
    for service in reader.services("WLANConfiguration") {
        let Some(info) = reader.try_read(&service, "GetInfo", &[]).await? else {
            continue;
        };
        let client_count = reader
            .try_read(&service, "GetTotalAssociations", &[])
            .await?
            .and_then(|v| v.number("NewTotalAssociations"));
        let ext = reader
            .try_read(&service, "X_AVM-DE_GetWLANExtInfo", &[])
            .await?;
        let guest = ext
            .as_ref()
            .and_then(|v| v.text("NewX_AVM-DE_APType"))
            .is_some_and(|kind| kind == "guest");
        let wlan = Wlan {
            band: info.get("NewX_AVM-DE_FrequencyBand").and_then(band),
            ssid: info.text("NewSSID"),
            channel: info.number("NewChannel"),
            standard: info.text("NewStandard"),
            enabled: info.flag("NewEnable"),
            status: info.text("NewStatus"),
            guest,
            client_count,
            clients: None,
            details: None,
            service,
        };
        networks.push(Read { wlan, info, ext });
    }
    Ok(networks)
}

/// The networks with what `views` asks for.
async fn with_views(
    reader: &mut Reader,
    networks: Vec<Read>,
    views: Views,
) -> Result<Vec<Wlan>, Error> {
    let mut wlans = Vec::new();
    for Read {
        mut wlan,
        info,
        ext,
    } in networks
    {
        if views.details {
            let details = details::read(reader, &wlan.service, &info, ext.as_ref()).await?;
            wlan.details = Some(details);
        }
        wlans.push(wlan);
    }
    if views.clients {
        add_clients(reader, &mut wlans).await?;
    }
    if wlans
        .iter()
        .any(|w| w.clients.as_ref().is_some_and(|c| !c.is_empty()))
    {
        let names = names(&status::hosts(reader).await?.hosts);
        for client in wlans
            .iter_mut()
            .flat_map(|w| w.clients.iter_mut().flatten())
        {
            client.name = client
                .mac
                .as_deref()
                .and_then(|mac| names.get(&lists::normal_mac(mac)))
                .cloned();
        }
    }
    Ok(wlans)
}

/// Adds the clients to each network. The device serves one list of the
/// clients of all its networks, the same for each service; it is read once,
/// and a client belongs to the guest network by its guest flag, or else to
/// the network on its channel. For a network whose share of the list holds
/// another number of clients than the device reported, or without a list,
/// the clients are read one call per client, up to [`MAX_CLIENTS`].
async fn add_clients(reader: &mut Reader, wlans: &mut [Wlan]) -> Result<(), Error> {
    let mut list = None;
    for wlan in wlans.iter() {
        if let Some(xml) = reader
            .document(
                &wlan.service,
                "X_AVM-DE_GetWLANDeviceListPath",
                "NewX_AVM-DE_WLANDeviceListPath",
            )
            .await?
        {
            list = Some(client_list(&xml)?);
            break;
        }
    }
    for wlan in wlans.iter_mut() {
        let listed = list
            .as_deref()
            .map(|list| of_network(list, wlan))
            .filter(|clients| {
                wlan.client_count
                    .is_none_or(|count| count as usize == clients.len())
            });
        wlan.clients = Some(match listed {
            Some(clients) => clients,
            None => one_per_call(reader, &wlan.service).await?,
        });
    }
    Ok(())
}

/// The clients in the `list` that are connected to `wlan`.
fn of_network(list: &[Listed], wlan: &Wlan) -> Vec<Client> {
    list.iter()
        .filter(|l| {
            if wlan.guest {
                l.guest == Some(true)
            } else {
                l.guest != Some(true) && l.channel.is_some() && l.channel == wlan.channel
            }
        })
        .map(|l| l.client.clone())
        .collect()
}

/// Reads the clients of a network one call per client.
async fn one_per_call(reader: &mut Reader, service: &str) -> Result<Vec<Client>, Error> {
    let entries = reader
        .entries(
            service,
            ("GetTotalAssociations", "NewTotalAssociations"),
            ("GetGenericAssociatedDeviceInfo", "NewAssociatedDeviceIndex"),
            MAX_CLIENTS,
        )
        .await?;
    Ok(entries.map_or_else(Vec::new, |(_, entries)| {
        entries.iter().map(client).collect()
    }))
}

/// Parses the list of the clients of all networks, like
/// `<List><TotalAssociations>…</TotalAssociations><Item>…</Item></List>`,
/// where each item has the fields `AssociatedDeviceIndex`,
/// `AssociatedDeviceMACAddress`, `AssociatedDeviceIPAddress`,
/// `AssociatedDeviceAuthState`, `X_AVM-DE_Speed`, `X_AVM-DE_SignalStrength`,
/// `AssociatedDeviceChannel`, `X_AVM-DE_ChannelWidth` and
/// `AssociatedDeviceGuest`.
fn client_list(xml: &str) -> Result<Vec<Listed>, Error> {
    let items = lists::items(xml, "WLAN device list", "List", "Item")?;
    Ok(items
        .iter()
        .map(|item| Listed {
            client: Client {
                name: None,
                ip: item.text("AssociatedDeviceIPAddress"),
                mac: item.text("AssociatedDeviceMACAddress"),
                signal: item.number("X_AVM-DE_SignalStrength"),
                rate: item.number("X_AVM-DE_Speed"),
            },
            channel: item.number("AssociatedDeviceChannel"),
            guest: item.flag("AssociatedDeviceGuest"),
        })
        .collect())
}

/// A client from the answer of `GetGenericAssociatedDeviceInfo`.
fn client(values: &Values) -> Client {
    Client {
        name: None,
        ip: values.text("NewAssociatedDeviceIPAddress"),
        mac: values.text("NewAssociatedDeviceMACAddress"),
        signal: values.number("NewX_AVM-DE_SignalStrength"),
        rate: values.number("NewX_AVM-DE_Speed"),
    }
}

/// The names of the hosts by MAC address, in the form of
/// [`lists::normal_mac`].
fn names(hosts: &[status::Host]) -> HashMap<String, String> {
    hosts
        .iter()
        .filter_map(|h| Some((lists::normal_mac(h.mac.as_deref()?), h.name.clone()?)))
        .collect()
}

/// The frequency band in MHz as text, like `2400` as `2.4 GHz`.
pub(crate) fn band(megahertz: &str) -> Option<String> {
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
    fn finds_networks_by_name() {
        let wlan = Wlan {
            service: "WLANConfiguration2".to_owned(),
            ssid: Some("wlan-1".to_owned()),
            ..Wlan::default()
        };
        for name in ["WLANConfiguration2", "wlanconfiguration2", "2", "wlan-1"] {
            assert!(is_named(&wlan, name), "{name}");
        }
        for name in ["1", "WLANConfiguration1", "wlan", ""] {
            assert!(!is_named(&wlan, name), "{name}");
        }
        let other = Wlan {
            service: "2".to_owned(),
            ..Wlan::default()
        };
        assert!(is_named(&other, "2"));
        assert!(!is_named(&Wlan::default(), "1"));
    }

    #[test]
    fn names_bands() {
        assert_eq!(band("2400").as_deref(), Some("2.4 GHz"));
        assert_eq!(band("5000").as_deref(), Some("5 GHz"));
        assert_eq!(band("6000").as_deref(), Some("6 GHz"));
        assert_eq!(band("unknown"), None);
    }

    #[test]
    fn parses_the_list_of_clients() {
        let xml = "<List><TotalAssociations>1</TotalAssociations><Item>\
                   <AssociatedDeviceIndex>0</AssociatedDeviceIndex>\
                   <AssociatedDeviceMACAddress>AA:BB:CC:00:00:01</AssociatedDeviceMACAddress>\
                   <AssociatedDeviceIPAddress></AssociatedDeviceIPAddress>\
                   <X_AVM-DE_Speed>72</X_AVM-DE_Speed>\
                   <X_AVM-DE_SignalStrength>71</X_AVM-DE_SignalStrength>\
                   <AssociatedDeviceChannel>6</AssociatedDeviceChannel>\
                   <AssociatedDeviceGuest>1</AssociatedDeviceGuest>\
                   </Item></List>";
        assert_eq!(
            client_list(xml).unwrap(),
            [Listed {
                client: Client {
                    mac: Some("AA:BB:CC:00:00:01".to_owned()),
                    signal: Some(71),
                    rate: Some(72),
                    ..Client::default()
                },
                channel: Some(6),
                guest: Some(true),
            }]
        );
        assert!(matches!(
            client_list("<Other/>"),
            Err(Error::InvalidDocument { .. })
        ));
    }

    #[test]
    fn assigns_clients_to_networks() {
        let listed = |mac: &str, channel: Option<u32>, guest: Option<bool>| Listed {
            client: Client {
                mac: Some(mac.to_owned()),
                ..Client::default()
            },
            channel,
            guest,
        };
        let list = [
            listed("1", Some(6), Some(false)),
            listed("2", Some(48), Some(false)),
            listed("3", Some(6), Some(true)),
            listed("4", Some(6), None),
            listed("5", None, Some(false)),
        ];
        let wlan = |channel: Option<u32>, guest: bool| Wlan {
            channel,
            guest,
            ..Wlan::default()
        };
        let macs = |wlan: &Wlan| -> Vec<String> {
            of_network(&list, wlan)
                .into_iter()
                .filter_map(|c| c.mac)
                .collect()
        };
        assert_eq!(macs(&wlan(Some(6), false)), ["1", "4"]);
        assert_eq!(macs(&wlan(Some(48), false)), ["2"]);
        assert_eq!(macs(&wlan(Some(6), true)), ["3"]);
        assert_eq!(macs(&wlan(None, false)), [] as [String; 0]);
    }

    #[test]
    fn finds_names_by_mac() {
        let host = |name: Option<&str>, mac: Option<&str>| status::Host {
            name: name.map(str::to_owned),
            mac: mac.map(str::to_owned),
            ..status::Host::default()
        };
        let names = names(&[
            host(Some("device-1"), Some("aa:bb:cc:00:00:01")),
            host(None, Some("AA:BB:CC:00:00:02")),
            host(Some("device-3"), None),
        ]);
        assert_eq!(names.len(), 1);
        assert_eq!(names["AABBCC000001"], "device-1");
    }
}
