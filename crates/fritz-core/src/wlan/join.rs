//! What a device needs to join a wireless network, as in a QR code:
//! `WIFI:T:WPA;S:<ssid>;P:<password>;H:true;;`.
//!
//! The passphrase comes from `GetSecurityKeys`, a getter that the
//! [policy](crate::policy) classes as secret and the read path refuses.
//! [`join`] calls it on a narrow path of its own: only this action, only of a
//! `WLANConfiguration` service, without inputs, and only for networks that
//! need a passphrase. Of its answer, only `NewKeyPassphrase` is kept, in a
//! [`SecretString`]; the other keys are dropped at once. The passphrase is
//! never logged, and the errors never hold it. `fritz` offers this path in
//! `fritz wlan qr` only; the MCP server, `fritz tr064 call` and the capture
//! never reach it.

use std::fmt;

use secrecy::{ExposeSecret as _, SecretString};
use tr064::{Action, Service};

use super::{Read, is_named, networks};
use crate::generic;
use crate::read::Reader;

/// The only service whose passphrase is read.
const SERVICE: &str = "WLANConfiguration";
/// The only secret getter that is called.
const ACTION: &str = "GetSecurityKeys";
/// The only output of [`ACTION`] that is kept.
const OUTPUT: &str = "NewKeyPassphrase";
/// The name of the guest network, ignoring case, on every model.
pub const GUEST: &str = "guest";

/// Why the data to join a network could not be read.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Read(#[from] generic::Error),
    #[error("no wireless network is named {0:?}")]
    Unknown(String),
    #[error("the device has no guest network")]
    NoGuest,
    #[error("{service} has no SSID")]
    NoSsid { service: String },
    /// WEP, or a beacon type that is not known.
    #[error("{service} uses the beacon type {beacon_type:?}, which a QR code cannot describe")]
    Unsupported {
        service: String,
        beacon_type: String,
    },
    #[error("the device returned no passphrase for {service}")]
    NoPassphrase { service: String },
}

/// How a client authenticates, as a QR code names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Auth {
    /// WPA, WPA2, WPA3 or a mix of them, with a passphrase.
    Wpa,
    /// An open network, or Enhanced Open (OWE), without a passphrase.
    Open,
}

impl Auth {
    /// The type in a QR code: `WPA` or `nopass`.
    pub fn code(self) -> &'static str {
        match self {
            Self::Wpa => "WPA",
            Self::Open => "nopass",
        }
    }
}

impl fmt::Display for Auth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code())
    }
}

/// What a device needs to join a wireless network.
#[derive(Debug)]
pub struct Join {
    /// The service, like `WLANConfiguration3`.
    pub service: String,
    /// The SSID verbatim, for the code: redact it with
    /// [`redact::value`](crate::redact::value) before it is shown.
    pub ssid: String,
    pub auth: Auth,
    /// Whether the SSID is not advertised.
    pub hidden: bool,
    /// Whether the network is on, if the device tells.
    pub enabled: Option<bool>,
    /// Whether the device reports it as its guest network.
    pub guest: bool,
    /// The passphrase, for [`Auth::Wpa`].
    pub passphrase: Option<SecretString>,
}

impl Join {
    /// The payload of a QR code, like
    /// `WIFI:T:WPA;S:<ssid>;P:<password>;H:true;;`, with the SSID and the
    /// passphrase [escaped](escape). `H:true` only for a hidden SSID.
    pub fn payload(&self) -> SecretString {
        let mut text = String::with_capacity(128);
        text.push_str("WIFI:T:");
        text.push_str(self.auth.code());
        text.push_str(";S:");
        escape(&self.ssid, &mut text);
        text.push(';');
        if let Some(passphrase) = &self.passphrase {
            text.push_str("P:");
            escape(passphrase.expose_secret(), &mut text);
            text.push(';');
        }
        if self.hidden {
            text.push_str("H:true;");
        }
        text.push(';');
        SecretString::from(text)
    }
}

/// Reads what a device needs to join the network that `network` names: the
/// guest network by [`GUEST`], ignoring case, also if a network has it as
/// its SSID, or else by service, number or SSID like
/// [`named`](super::named). Of several networks with the name, the first
/// one. Reads the passphrase only for a network that needs one.
///
/// # Errors
///
/// The errors of reading, [`Error::Unknown`] or [`Error::NoGuest`] if no
/// network fits, [`Error::NoSsid`], [`Error::Unsupported`] for WEP and
/// unknown beacon types, and [`Error::NoPassphrase`].
pub async fn join(reader: &mut Reader, network: &str) -> Result<Join, Error> {
    let mut networks = networks(reader).await?.into_iter();
    let Read { wlan, info, .. } = if network.eq_ignore_ascii_case(GUEST) {
        networks.find(|n| n.wlan.guest).ok_or(Error::NoGuest)?
    } else {
        networks
            .find(|n| is_named(&n.wlan, network))
            .ok_or_else(|| Error::Unknown(network.to_owned()))?
    };
    let service = wlan.service;
    // The read hides values like `?sid=…`, which an SSID may hold too; the
    // code needs it verbatim.
    let ssid = reader
        .output(&service, "GetInfo", &[])
        .await?
        .into_iter()
        .find(|(name, value)| name == "NewSSID" && !value.is_empty());
    let Some((_, ssid)) = ssid else {
        return Err(Error::NoSsid { service });
    };
    let beacon_type = info.text("NewBeaconType").unwrap_or_default();
    let basic = info.text("NewBasicEncryptionModes");
    let Some(auth) = auth(&beacon_type, basic.as_deref()) else {
        return Err(Error::Unsupported {
            service,
            beacon_type,
        });
    };
    let hidden = reader
        .try_read(&service, "GetBeaconAdvertisement", &[])
        .await?
        .and_then(|v| v.flag("NewBeaconAdvertisementEnabled"))
        .is_some_and(|advertised| !advertised);
    let passphrase = match auth {
        Auth::Wpa => Some(passphrase(reader, &service).await?),
        Auth::Open => None,
    };
    Ok(Join {
        service,
        ssid,
        auth,
        hidden,
        enabled: wlan.enabled,
        guest: wlan.guest,
        passphrase,
    })
}

/// How a client authenticates with a beacon type: WPA, WPA2, WPA3 and their
/// mixes with a passphrase, open networks and OWE without. `None` for WEP
/// and beacon types that are not known.
fn auth(beacon_type: &str, basic_encryption: Option<&str>) -> Option<Auth> {
    match beacon_type {
        "Basic" if basic_encryption.is_some_and(|b| b.contains("WEP")) => None,
        "None" | "Basic" | "OWE" | "OWETrans" => Some(Auth::Open),
        "WPA" | "11i" | "WPAand11i" | "11iandWPA3" | "WPA3" => Some(Auth::Wpa),
        _ => None,
    }
}

/// Whether the narrow path may call `action` of `service`: only
/// [`ACTION`] of a [`SERVICE`], without inputs.
fn allowed(service: &Service, action: &Action) -> bool {
    service.name() == SERVICE && action.name == ACTION && action.inputs().next().is_none()
}

/// The passphrase of the network of `reference`: the one call of a secret
/// getter, see the [module](self). The other keys of the answer are
/// dropped, and wiped from memory, at once.
async fn passphrase(reader: &mut Reader, reference: &str) -> Result<SecretString, Error> {
    let (service, scpd) = reader.service(reference).await?;
    let action = generic::action(&service, scpd, ACTION)?;
    let class = crate::policy::classify(&service, action);
    let refused = generic::Error::Refused {
        action: action.name.clone(),
        class,
    };
    allowed(&service, action).then_some(()).ok_or(refused)?;
    let output = reader
        .client()
        .call(&service, ACTION, &[])
        .await
        .map_err(generic::Error::from)?;
    let mut passphrase = None;
    for (name, value) in output {
        let value = SecretString::from(value);
        if name == OUTPUT && !value.expose_secret().is_empty() {
            passphrase = Some(value);
        }
    }
    passphrase.ok_or_else(|| Error::NoPassphrase {
        service: reference.to_owned(),
    })
}

/// Appends `text` to `out` with `\`, `;`, `,`, `:` and `"` escaped by a
/// backslash, as the fields of a QR code need.
pub fn escape(text: &str, out: &mut String) {
    for c in text.chars() {
        if matches!(c, '\\' | ';' | ',' | ':' | '"') {
            out.push('\\');
        }
        out.push(c);
    }
}

#[cfg(test)]
mod tests {
    use tr064::{Argument, Direction};

    use super::*;

    fn escaped(text: &str) -> String {
        let mut out = String::new();
        escape(text, &mut out);
        out
    }

    fn join(auth: Auth, ssid: &str, passphrase: Option<&str>, hidden: bool) -> Join {
        Join {
            service: "WLANConfiguration1".to_owned(),
            ssid: ssid.to_owned(),
            auth,
            hidden,
            enabled: Some(true),
            guest: false,
            passphrase: passphrase.map(|p| SecretString::from(p.to_owned())),
        }
    }

    #[test]
    fn escapes_special_characters() {
        assert_eq!(escaped("plain"), "plain");
        assert_eq!(escaped(r#"a\b;c,d:e"f"#), r#"a\\b\;c\,d\:e\"f"#);
        assert_eq!(escaped("äöü 🙂'"), "äöü 🙂'");
    }

    #[test]
    fn builds_payloads() {
        let wpa = join(Auth::Wpa, "wlan;1", Some("fake:pass"), false);
        assert_eq!(
            wpa.payload().expose_secret(),
            r"WIFI:T:WPA;S:wlan\;1;P:fake\:pass;;"
        );
        let hidden = join(Auth::Wpa, "hidden", Some("fake"), true);
        assert_eq!(
            hidden.payload().expose_secret(),
            "WIFI:T:WPA;S:hidden;P:fake;H:true;;"
        );
        let open = join(Auth::Open, "open", None, false);
        assert_eq!(open.payload().expose_secret(), "WIFI:T:nopass;S:open;;");
        assert!(!format!("{wpa:?}").contains("fake"));
    }

    #[test]
    fn tells_the_auth_by_the_beacon_type() {
        for beacon in ["WPA", "11i", "WPAand11i", "11iandWPA3", "WPA3"] {
            assert_eq!(auth(beacon, None), Some(Auth::Wpa), "{beacon}");
        }
        for beacon in ["None", "OWE", "OWETrans"] {
            assert_eq!(auth(beacon, None), Some(Auth::Open), "{beacon}");
        }
        assert_eq!(auth("Basic", Some("None")), Some(Auth::Open));
        assert_eq!(auth("Basic", Some("WEPEncryption")), None);
        assert_eq!(auth("Other", None), None);
        assert_eq!(auth("", None), None);
        assert_eq!(Auth::Wpa.to_string(), "WPA");
        assert_eq!(Auth::Open.to_string(), "nopass");
    }

    #[test]
    fn allows_only_the_passphrase_getter() {
        let service = |name: &str| Service {
            service_type: format!("urn:dslforum-org:service:{name}:1"),
            service_id: format!("urn:{name}-com:serviceId:{name}1"),
            control_url: "/upnp/control/x".to_owned(),
            event_sub_url: None,
            scpd_url: "/x.xml".to_owned(),
        };
        let action = |name: &str, inputs: &[&str]| Action {
            name: name.to_owned(),
            arguments: inputs
                .iter()
                .map(|input| Argument {
                    name: (*input).to_owned(),
                    direction: Direction::In,
                    related_state_variable: "X".to_owned(),
                })
                .collect(),
        };
        let wlan = service("WLANConfiguration");
        assert!(allowed(&wlan, &action(ACTION, &[])));
        assert!(!allowed(&wlan, &action(ACTION, &["NewIndex"])));
        assert!(!allowed(&wlan, &action("GetInfo", &[])));
        assert!(!allowed(
            &service("DeviceConfig"),
            &action("X_AVM-DE_GetConfigFile", &[])
        ));
        assert!(!allowed(&service("X_VoIP"), &action(ACTION, &[])));
    }
}
