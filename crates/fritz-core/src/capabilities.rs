//! What a device offers and what its user may do there.

use serde::Serialize;

use crate::generic::Error;
use crate::read::Reader;
use crate::status::{self, Right};

/// The features and the services that offer them. A device has a feature
/// if it has one of the services.
const FEATURES: [(&str, &[&str]); 17] = [
    ("dsl", &["WANDSLInterfaceConfig"]),
    ("mobile", &["X_AVM-DE_WANMobileConnection"]),
    ("wlan", &["WLANConfiguration"]),
    ("hosts", &["Hosts"]),
    ("port_forwarding", &["WANPPPConnection", "WANIPConnection"]),
    ("upnp", &["X_AVM-DE_UPnP"]),
    ("parental_controls", &["X_AVM-DE_HostFilter"]),
    ("telephony", &["X_VoIP", "X_AVM-DE_OnTel"]),
    ("dect", &["X_AVM-DE_Dect"]),
    ("answering_machine", &["X_AVM-DE_TAM"]),
    ("smarthome", &["X_AVM-DE_Homeauto"]),
    ("storage", &["X_AVM-DE_Storage"]),
    ("webdav", &["X_AVM-DE_WebDAVClient"]),
    ("powerline", &["X_AVM-DE_Homeplug"]),
    ("myfritz", &["X_AVM-DE_MyFritz"]),
    ("remote_access", &["X_AVM-DE_RemoteAccess"]),
    ("speedtest", &["X_AVM-DE_Speedtest"]),
];

/// What the device offers and what the user may do.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Capabilities {
    /// The features the device offers, like `wlan` or `dect`. The rights
    /// decide whether the user may use them.
    pub features: Vec<String>,
    /// What the user may do, like `BoxAdmin: readwrite`.
    pub rights: Vec<Right>,
    /// Whether changes need a confirmation on the device, like a button
    /// press.
    pub two_factor: Option<bool>,
    /// The number of services of the device.
    pub services: usize,
}

/// The features of a device with services of these names.
pub fn features<'a>(names: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    let names: Vec<&str> = names.into_iter().collect();
    FEATURES
        .iter()
        .filter(|(_, services)| services.iter().any(|s| names.contains(s)))
        .map(|(feature, _)| (*feature).to_owned())
        .collect()
}

/// Reads the capabilities.
///
/// # Errors
///
/// The errors of reading other than missing parts, see
/// [`Reader::try_read`].
pub async fn capabilities(reader: &mut Reader) -> Result<Capabilities, Error> {
    let description = reader.description();
    let features = features(description.services().map(tr064::Service::name));
    let services = description.services().count();
    let rights = reader
        .try_read("LANConfigSecurity1", "X_AVM-DE_GetCurrentUser", &[])
        .await?
        .and_then(|values| values.text("NewX_AVM-DE_CurrentUserRights"))
        .map_or_else(Vec::new, |xml| status::rights(&xml));
    let two_factor = reader
        .try_read("X_AVM-DE_Auth1", "GetInfo", &[])
        .await?
        .and_then(|values| values.flag("NewEnabled"));
    Ok(Capabilities {
        features,
        rights,
        two_factor,
        services,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_features_by_their_services() {
        assert_eq!(
            features(["WANIPConnection", "X_AVM-DE_Dect", "Other"]),
            ["port_forwarding", "dect"]
        );
        assert_eq!(features([]), Vec::<String>::new());
    }
}
