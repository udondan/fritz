//! The port forwardings of a device and whether hosts may open ports
//! themselves through UPnP.

use serde::Serialize;

use crate::generic::Error;
use crate::read::Reader;

/// The most forwardings read per connection service, as a guard against a
/// device that never ends the list.
pub const MAX_FORWARDINGS: u32 = 1000;

/// The connection services that hold port forwardings.
const SERVICES: [&str; 2] = ["WANPPPConnection1", "WANIPConnection1"];

/// The port forwardings.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Ports {
    /// Whether the device answers UPnP, so hosts may find it and open
    /// ports themselves.
    pub upnp: Option<bool>,
    pub forwardings: Vec<Forwarding>,
    /// Whether a list of forwardings could be read: without one, like
    /// for missing rights, [`forwardings`](Self::forwardings) is empty but
    /// tells nothing.
    #[serde(skip)]
    pub listed: bool,
}

/// A port forwarding from the internet to a host.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Forwarding {
    /// The connection service, like `WANPPPConnection1`.
    pub service: String,
    pub enabled: bool,
    /// `TCP` or `UDP`.
    pub protocol: Option<String>,
    pub external_port: Option<u16>,
    pub internal_client: Option<String>,
    pub internal_port: Option<u16>,
    pub description: Option<String>,
    /// The only host on the internet the forwarding is for, if any.
    pub remote_host: Option<String>,
    /// Seconds until the forwarding ends, if it does.
    pub lease_duration: Option<u32>,
}

/// Reads the port forwardings of all connection services.
///
/// # Errors
///
/// The errors of reading other than missing parts, see
/// [`Reader::try_read`].
pub async fn ports(reader: &mut Reader) -> Result<Ports, Error> {
    let upnp = reader
        .try_read("X_AVM-DE_UPnP1", "GetInfo", &[])
        .await?
        .and_then(|values| values.flag("NewEnable"));
    let mut forwardings = Vec::new();
    let mut listed = false;
    for service in SERVICES {
        let Some((_, entries)) = reader
            .entries(
                service,
                (
                    "GetPortMappingNumberOfEntries",
                    "NewPortMappingNumberOfEntries",
                ),
                ("GetGenericPortMappingEntry", "NewPortMappingIndex"),
                MAX_FORWARDINGS,
            )
            .await?
        else {
            continue;
        };
        listed = true;
        forwardings.extend(entries.iter().map(|entry| Forwarding {
            service: service.to_owned(),
            enabled: entry.flag("NewEnabled").unwrap_or_default(),
            protocol: entry.text("NewProtocol"),
            external_port: entry.number("NewExternalPort").filter(|&p| p != 0),
            internal_client: entry.text("NewInternalClient"),
            internal_port: entry.number("NewInternalPort").filter(|&p| p != 0),
            description: entry.text("NewPortMappingDescription"),
            remote_host: entry.text("NewRemoteHost"),
            lease_duration: entry.number("NewLeaseDuration").filter(|&d| d != 0),
        }));
    }
    Ok(Ports {
        upnp,
        forwardings,
        listed,
    })
}
