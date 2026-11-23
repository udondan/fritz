//! What of the home network can be reached from the internet or managed
//! from outside: remote access, MyFRITZ, dynamic DNS, port forwardings, an
//! exposed host, UPnP, FTP from the internet, shared links, VPN, provider
//! management (TR-069) and USP controllers.
//!
//! Only getters are called, and the settings modules `forwardrules` and
//! `vpn` of the web interface are only read. No passwords or credentials
//! are read: login names come masked, and the URLs of shared links, which
//! grant access, are left out. What a device lacks is unknown, not an
//! error.

use serde::Serialize;
use serde_json::Value;

use crate::generic::Error;
use crate::ports::{self, Forwarding};
use crate::read::{Reader, Values};
use crate::session::Session;
use crate::settings;

mod items;

pub use items::{Item, Kind, Severity, State};

/// The most entries read of each list, as a guard against a device that
/// never ends a list.
const MAX_ENTRIES: u32 = 100;

/// Everything that can be reached from the internet, with a summary of
/// each [`Kind`] in [`items`](Self::items). Parts the device does not
/// report are `None`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Exposure {
    /// One item per [`Kind`], in order, with its state and severity.
    pub items: Vec<Item>,
    pub remote_access: Option<RemoteAccess>,
    pub myfritz: Option<MyFritz>,
    pub dynamic_dns: Option<DynamicDns>,
    /// The port forwardings, enabled or not.
    pub port_forwardings: Option<Vec<Forwarding>>,
    /// Whether all ports of one host are open to the internet.
    pub exposed_host: Option<bool>,
    /// Whether hosts may open ports themselves through UPnP.
    pub upnp: Option<bool>,
    pub ftp: Option<Ftp>,
    pub shared_links: Option<Vec<Link>>,
    pub vpn: Option<Vec<Vpn>>,
    pub tr069: Option<Tr069>,
    pub usp: Option<Usp>,
}

/// Remote access to the web interface of the device over HTTPS.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct RemoteAccess {
    pub enabled: bool,
    pub port: Option<u16>,
    /// Whether the certificate comes from Let's Encrypt rather than being
    /// signed by the device itself.
    pub lets_encrypt: Option<bool>,
}

/// The MyFRITZ account of the device and the services it shares.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct MyFritz {
    pub enabled: bool,
    pub registered: Option<bool>,
    /// The `MyFRITZ` address of the device. Identifying.
    pub address: Option<String>,
    pub services: Vec<Service>,
}

/// A service of the home network shared through `MyFRITZ`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Service {
    /// The name of the service. Identifying.
    pub name: Option<String>,
    pub enabled: bool,
    /// Like `https`.
    pub scheme: Option<String>,
    pub port: Option<u16>,
}

/// A dynamic DNS name of the device at a provider.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct DynamicDns {
    pub enabled: bool,
    /// The domain. Identifying.
    pub domain: Option<String>,
    pub provider: Option<String>,
}

/// Access to the storage of the device by FTP from the internet.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Ftp {
    pub enabled: bool,
    pub port: Option<u16>,
    /// Whether only connections with TLS are allowed.
    pub tls_only: Option<bool>,
}

/// A link that shares a file or folder of the storage. Its URL grants
/// access and is never read.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Link {
    pub valid: bool,
    /// The shared file or folder. Identifying.
    pub path: Option<String>,
    pub directory: bool,
    /// When the link expires, or `None` if it never does.
    pub expires: Option<String>,
    pub accesses: Option<u32>,
    /// The most accesses allowed, or `None` without a limit.
    pub access_limit: Option<u32>,
}

/// A VPN connection from the settings module `vpn`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Vpn {
    /// The name of the connection. Identifying.
    pub name: Option<String>,
}

/// Management of the device by the provider (TR-069).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Tr069 {
    /// Whether a management server is set.
    pub enabled: bool,
    /// The management server. Identifying.
    pub url: Option<String>,
    /// Whether the device contacts the server at intervals.
    pub periodic_inform: Option<bool>,
}

/// Controllers that may manage the device through USP.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Usp {
    /// Whether `MyFRITZ` may manage the device through USP.
    pub myfritz: Option<bool>,
    pub controllers: Vec<Controller>,
}

/// A USP controller.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Controller {
    pub enabled: bool,
    /// The host of the controller. Identifying.
    pub host: Option<String>,
    pub port: Option<u16>,
    pub tls: Option<bool>,
}

impl Exposure {
    /// Replaces the values that identify the owner, like the `MyFRITZ`
    /// address, domains, names, shared paths, hosts and the addresses a
    /// forwarding is limited to, with `hidden`, for output that must not
    /// identify them.
    #[must_use]
    pub fn without_identifying(mut self, hidden: &str) -> Self {
        let hide = |value: &mut Option<String>| {
            if value.is_some() {
                *value = Some(hidden.to_owned());
            }
        };
        if let Some(myfritz) = &mut self.myfritz {
            hide(&mut myfritz.address);
            myfritz.services.iter_mut().for_each(|s| hide(&mut s.name));
        }
        if let Some(dns) = &mut self.dynamic_dns {
            hide(&mut dns.domain);
        }
        let forwardings = self.port_forwardings.iter_mut().flatten();
        forwardings.for_each(|f| hide(&mut f.remote_host));
        let links = self.shared_links.iter_mut().flatten();
        links.for_each(|l| hide(&mut l.path));
        self.vpn
            .iter_mut()
            .flatten()
            .for_each(|v| hide(&mut v.name));
        if let Some(tr069) = &mut self.tr069 {
            hide(&mut tr069.url);
        }
        let controllers = self.usp.iter_mut().flat_map(|u| &mut u.controllers);
        controllers.for_each(|c| hide(&mut c.host));
        self
    }

    /// Whether an item has the severity `warning`.
    pub fn has_warning(&self) -> bool {
        self.items.iter().any(|i| i.severity == Severity::Warning)
    }
}

/// Reads what can be reached from the internet: through TR-064, and the
/// exposed host and VPN through the web interface with `session`, which
/// it then logs out. Without a session, those two are unknown.
///
/// # Errors
///
/// The errors of reading other than missing parts, see
/// [`Reader::try_read`].
pub async fn exposure(reader: &mut Reader, session: Option<Session>) -> Result<Exposure, Error> {
    let web = session.is_some();
    let (exposed_host, vpn) = match session {
        Some(session) => {
            let read = (exposed_host(&session).await, vpn(&session).await);
            // The device ends the session itself after a while.
            let _ = session.logout().await;
            read
        }
        None => (None, None),
    };
    let ports = ports::ports(reader).await?;
    let has_ports = !reader.services("WANPPPConnection").is_empty()
        || !reader.services("WANIPConnection").is_empty();
    let mut exposure = Exposure {
        items: Vec::new(),
        remote_access: remote_access(reader).await?,
        myfritz: myfritz(reader).await?,
        dynamic_dns: dynamic_dns(reader).await?,
        port_forwardings: has_ports.then_some(ports.forwardings),
        exposed_host,
        upnp: ports.upnp,
        ftp: ftp(reader).await?,
        shared_links: shared_links(reader).await?,
        vpn,
        tr069: tr069(reader).await?,
        usp: usp(reader).await?,
    };
    exposure.items = items::items(&exposure, web);
    Ok(exposure)
}

async fn remote_access(reader: &mut Reader) -> Result<Option<RemoteAccess>, Error> {
    let info = reader
        .try_read("X_AVM-DE_RemoteAccess1", "GetInfo", &[])
        .await?;
    Ok(info.map(|info| RemoteAccess {
        enabled: info.flag("NewEnabled").unwrap_or_default(),
        port: port(&info, "NewPort"),
        lets_encrypt: info.flag("NewLetsEncryptEnabled"),
    }))
}

async fn dynamic_dns(reader: &mut Reader) -> Result<Option<DynamicDns>, Error> {
    let info = reader
        .try_read("X_AVM-DE_RemoteAccess1", "GetDDNSInfo", &[])
        .await?;
    Ok(info.map(|info| DynamicDns {
        enabled: info.flag("NewEnabled").unwrap_or_default(),
        domain: info.text("NewDomain"),
        provider: info.text("NewProviderName"),
    }))
}

async fn myfritz(reader: &mut Reader) -> Result<Option<MyFritz>, Error> {
    const SERVICE: &str = "X_AVM-DE_MyFritz1";
    let Some(info) = reader.try_read(SERVICE, "GetInfo", &[]).await? else {
        return Ok(None);
    };
    let services = reader
        .entries(
            SERVICE,
            ("GetNumberOfServices", "NewNumberOfServices"),
            ("GetServiceByIndex", "NewIndex"),
            MAX_ENTRIES,
        )
        .await?
        .map_or_else(Vec::new, |(_, entries)| entries);
    Ok(Some(MyFritz {
        enabled: info.flag("NewEnabled").unwrap_or_default(),
        registered: info.flag("NewDeviceRegistered"),
        address: info.text("NewDynDNSName"),
        services: services
            .iter()
            .map(|entry| Service {
                name: entry.text("NewName"),
                enabled: entry.flag("NewEnabled").unwrap_or_default(),
                scheme: entry.text("NewScheme"),
                port: port(entry, "NewPort"),
            })
            .collect(),
    }))
}

async fn ftp(reader: &mut Reader) -> Result<Option<Ftp>, Error> {
    let info = reader.try_read("X_AVM-DE_Storage1", "GetInfo", &[]).await?;
    Ok(info.map(|info| Ftp {
        enabled: info.flag("NewFTPWANEnable").unwrap_or_default(),
        port: port(&info, "NewFTPWANPort"),
        tls_only: info.flag("NewFTPWANSSLOnly"),
    }))
}

async fn shared_links(reader: &mut Reader) -> Result<Option<Vec<Link>>, Error> {
    let entries = reader
        .entries(
            "X_AVM-DE_Filelinks1",
            ("GetNumberOfFilelinkEntries", "NewNumberOfEntries"),
            ("GetGenericFilelinkEntry", "NewIndex"),
            MAX_ENTRIES,
        )
        .await?;
    Ok(entries.map(|(_, entries)| entries.iter().map(link).collect()))
}

/// A shared link from the answer of `GetGenericFilelinkEntry`, without its
/// URL. `NewExpire` is the number of days the link was made for, 0 for no
/// end.
fn link(entry: &Values) -> Link {
    let expires = entry.number::<u32>("NewExpire").unwrap_or_default() != 0;
    Link {
        valid: entry.flag("NewValid").unwrap_or_default(),
        path: entry.text("NewPath"),
        directory: entry.flag("NewIsDirectory").unwrap_or_default(),
        expires: entry.text("NewExpireDate").filter(|_| expires),
        accesses: entry.number("NewAccessCount"),
        access_limit: entry
            .number("NewAccessCountLimit")
            .filter(|&n: &u32| n != 0),
    }
}

async fn tr069(reader: &mut Reader) -> Result<Option<Tr069>, Error> {
    let info = reader.try_read("ManagementServer1", "GetInfo", &[]).await?;
    Ok(info.map(|info| {
        let url = info.text("NewURL");
        Tr069 {
            enabled: url.is_some(),
            url,
            periodic_inform: info.flag("NewPeriodicInformEnable"),
        }
    }))
}

async fn usp(reader: &mut Reader) -> Result<Option<Usp>, Error> {
    const SERVICE: &str = "X_AVM-DE_USPController1";
    let Some((_, entries)) = reader
        .entries(
            SERVICE,
            (
                "GetUSPControllerNumberOfEntries",
                "NewUSPControllerNumberOfEntries",
            ),
            ("GetUSPControllerByIndex", "NewIndex"),
            MAX_ENTRIES,
        )
        .await?
    else {
        return Ok(None);
    };
    let myfritz = reader
        .try_read(SERVICE, "GetUSPMyFRITZEnable", &[])
        .await?
        .and_then(|values| values.flag("NewUSPMyFRITZEnabled"));
    Ok(Some(Usp {
        myfritz,
        controllers: entries
            .iter()
            .map(|entry| Controller {
                enabled: entry.flag("NewEnable").unwrap_or_default(),
                host: entry.text("NewHostname"),
                port: port(entry, "NewPort"),
                tls: entry.flag("NewUseTLS"),
            })
            .collect(),
    }))
}

/// A port, unless it is 0.
fn port(values: &Values, name: &str) -> Option<u16> {
    values.number(name).filter(|&p| p != 0)
}

/// Whether a host is exposed, from the settings module `forwardrules`.
async fn exposed_host(session: &Session) -> Option<bool> {
    let module = settings::module(session, "forwardrules").await.ok()?;
    flag(module.get("use_exposed_host")?)
}

/// A flag of the web interface, `"1"` or `true`.
fn flag(value: &Value) -> Option<bool> {
    match value {
        Value::String(flag) => Some(flag == "1"),
        Value::Bool(flag) => Some(*flag),
        _ => None,
    }
}

/// The VPN connections in the settings module `vpn`.
async fn vpn(session: &Session) -> Option<Vec<Vpn>> {
    let module = settings::module(session, "vpn").await.ok()?;
    Some(connections(&module))
}

/// The VPN connections of the settings module `vpn`: the objects with a
/// name in its lists, at any depth.
fn connections(value: &Value) -> Vec<Vpn> {
    match value {
        Value::Object(object) => object.values().flat_map(connections).collect(),
        Value::Array(entries) => entries
            .iter()
            .flat_map(|entry| match entry.get("name") {
                Some(Value::String(name)) => vec![Vpn {
                    name: Some(name.clone()).filter(|n| !n.is_empty()),
                }],
                _ => connections(entry),
            })
            .collect(),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::read::values;

    #[test]
    fn reads_flags_of_the_web_interface() {
        assert_eq!(flag(&json!("1")), Some(true));
        assert_eq!(flag(&json!("0")), Some(false));
        assert_eq!(flag(&json!(true)), Some(true));
        assert_eq!(flag(&json!(1)), None);
    }

    #[test]
    fn reads_shared_links_without_their_url() {
        let entry = values(&[
            ("NewValid", "1"),
            ("NewPath", "/photos"),
            ("NewIsDirectory", "1"),
            ("NewUrl", "https://link.example/secret"),
            ("NewExpire", "7"),
            ("NewExpireDate", "2026-12-01T00:00:00"),
            ("NewAccessCount", "2"),
            ("NewAccessCountLimit", "0"),
        ]);
        let link = link(&entry);
        assert_eq!(
            link,
            Link {
                valid: true,
                path: Some("/photos".to_owned()),
                directory: true,
                expires: Some("2026-12-01T00:00:00".to_owned()),
                accesses: Some(2),
                access_limit: None,
            }
        );
        let forever = values(&[("NewExpire", "0"), ("NewExpireDate", "x")]);
        assert_eq!(super::link(&forever).expires, None);
    }

    #[test]
    fn finds_vpn_connections_at_any_depth() {
        let module = json!({
            "enabled": "1",
            "wireguard": {"connections": [{"name": "phone"}, {"name": ""}]},
            "ipsec": [{"name": "office", "user": [{"name": "nested"}]}],
            "other": [1, "x", {"id": 3}],
        });
        let names: Vec<Option<String>> = connections(&module).into_iter().map(|c| c.name).collect();
        assert_eq!(
            names,
            [Some("office".to_owned()), Some("phone".to_owned()), None]
        );
        assert_eq!(connections(&json!("text")), []);
    }

    #[test]
    fn hides_what_identifies_the_owner() {
        let named = |name: &str| Some(name.to_owned());
        let exposure = Exposure {
            myfritz: Some(MyFritz {
                address: named("box.myfritz.example"),
                services: vec![Service {
                    name: named("nas"),
                    ..Service::default()
                }],
                ..MyFritz::default()
            }),
            dynamic_dns: Some(DynamicDns {
                domain: named("home.example"),
                ..DynamicDns::default()
            }),
            port_forwardings: Some(vec![Forwarding {
                remote_host: named("198.51.100.7"),
                description: named("nas"),
                ..Forwarding::default()
            }]),
            shared_links: Some(vec![Link {
                path: named("/photos"),
                ..Link::default()
            }]),
            vpn: Some(vec![Vpn {
                name: named("phone"),
            }]),
            tr069: Some(Tr069 {
                url: named("https://acs.example"),
                ..Tr069::default()
            }),
            usp: Some(Usp {
                myfritz: None,
                controllers: vec![Controller {
                    host: named("usp.example"),
                    ..Controller::default()
                }],
            }),
            ..Exposure::default()
        };
        let hidden = exposure.without_identifying("x");
        let json = serde_json::to_string(&hidden).unwrap();
        for value in [
            "box.myfritz.example",
            "home.example",
            "198.51.100.7",
            "/photos",
            "phone",
            "acs.example",
            "usp.example",
        ] {
            assert!(!json.contains(value), "{value} in {json}");
        }
        // Not identifying, as in `fritz_ports`.
        assert!(json.contains("\"description\":\"nas\""), "{json}");
        // Missing values stay missing.
        let empty = Exposure::default().without_identifying("x");
        assert_eq!(empty, Exposure::default());
    }
}
