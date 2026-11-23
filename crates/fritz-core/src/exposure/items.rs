//! The summary of an [`Exposure`]: one [`Item`] per [`Kind`] with its state
//! and a severity hint.

use serde::Serialize;

use super::{Exposure, Ftp, MyFritz, RemoteAccess, Usp};

/// What can be reached from the internet, in the order shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    RemoteAccess,
    Myfritz,
    DynamicDns,
    PortForwarding,
    ExposedHost,
    Upnp,
    Ftp,
    SharedLinks,
    Vpn,
    Tr069,
    Usp,
}

impl Kind {
    /// The name shown, like `Remote access`.
    pub fn label(self) -> &'static str {
        match self {
            Self::RemoteAccess => "Remote access",
            Self::Myfritz => "MyFRITZ",
            Self::DynamicDns => "Dynamic DNS",
            Self::PortForwarding => "Port forwarding",
            Self::ExposedHost => "Exposed host",
            Self::Upnp => "UPnP",
            Self::Ftp => "Storage via FTP",
            Self::SharedLinks => "Shared links",
            Self::Vpn => "VPN",
            Self::Tr069 => "TR-069",
            Self::Usp => "USP",
        }
    }
}

/// Whether something is reachable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    On,
    Off,
    /// The device does not report it, like a repeater or older firmware.
    Unknown,
}

/// How much attention an item needs.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    #[default]
    Info,
    Notice,
    Warning,
}

/// The summary of one [`Kind`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Item {
    pub kind: Kind,
    pub state: State,
    /// For lists, like port forwardings, the number of active entries.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub count: Option<usize>,
    pub severity: Severity,
    /// Why the item needs attention, or why it is unknown.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub hints: Vec<String>,
}

impl Item {
    fn new(kind: Kind, state: State) -> Self {
        Self {
            kind,
            state,
            count: None,
            severity: Severity::Info,
            hints: Vec::new(),
        }
    }

    /// An item that is on or off, or unknown.
    fn switch(kind: Kind, on: Option<bool>) -> Self {
        let state = match on {
            Some(true) => State::On,
            Some(false) => State::Off,
            None => State::Unknown,
        };
        Self::new(kind, state)
    }

    /// An item with a number of active entries, on unless it is 0.
    fn list(kind: Kind, count: Option<usize>) -> Self {
        Self {
            count,
            ..Self::switch(kind, count.map(|n| n > 0))
        }
    }

    /// The item with `severity` and `hint` if `when` holds.
    fn flag(mut self, when: bool, severity: Severity, hint: &str) -> Self {
        if when {
            self.severity = self.severity.max(severity);
            self.hints.push(hint.to_owned());
        }
        self
    }

    /// The item with `severity` and `hint` if it is on.
    fn when_on(self, severity: Severity, hint: &str) -> Self {
        let on = self.state == State::On;
        self.flag(on, severity, hint)
    }

    fn is_on(&self) -> bool {
        self.state == State::On
    }
}

/// The items of `exposure`; `web` tells whether the web interface could be
/// read.
pub(super) fn items(exposure: &Exposure, web: bool) -> Vec<Item> {
    let without_web = |item: Item| {
        let unknown = !web && item.state == State::Unknown;
        item.flag(
            unknown,
            Severity::Info,
            "needs a login to the web interface",
        )
    };
    let forwardings = exposure
        .port_forwardings
        .as_ref()
        .map(|all| all.iter().filter(|f| f.enabled).count());
    vec![
        remote_access(exposure.remote_access.as_ref()),
        myfritz(exposure.myfritz.as_ref()),
        Item::switch(
            Kind::DynamicDns,
            exposure.dynamic_dns.as_ref().map(|d| d.enabled),
        ),
        Item::list(Kind::PortForwarding, forwardings)
            .when_on(Severity::Notice, "ports open to hosts of the home network"),
        without_web(Item::switch(Kind::ExposedHost, exposure.exposed_host))
            .when_on(Severity::Warning, "all ports of one host are open"),
        Item::switch(Kind::Upnp, exposure.upnp)
            .when_on(Severity::Notice, "hosts may open ports themselves"),
        ftp(exposure.ftp.as_ref()),
        shared_links(exposure),
        without_web(Item::list(Kind::Vpn, exposure.vpn.as_ref().map(Vec::len))),
        Item::switch(Kind::Tr069, exposure.tr069.as_ref().map(|t| t.enabled))
            .when_on(Severity::Info, "the provider may manage the device"),
        usp(exposure.usp.as_ref()),
    ]
}

fn remote_access(access: Option<&RemoteAccess>) -> Item {
    let item = Item::switch(Kind::RemoteAccess, access.map(|a| a.enabled));
    let self_signed = item.is_on() && access.and_then(|a| a.lets_encrypt) == Some(false);
    item.when_on(
        Severity::Notice,
        "web interface reachable from the internet",
    )
    .flag(
        self_signed,
        Severity::Notice,
        "certificate signed by the device itself",
    )
}

fn myfritz(myfritz: Option<&MyFritz>) -> Item {
    let mut item = Item::switch(Kind::Myfritz, myfritz.map(|m| m.enabled));
    if let Some(myfritz) = myfritz.filter(|_| item.is_on()) {
        let shared = myfritz.services.iter().filter(|s| s.enabled).count();
        item.count = Some(shared);
        item = item.flag(
            shared > 0,
            Severity::Notice,
            "services of the home network shared",
        );
    }
    item
}

fn ftp(ftp: Option<&Ftp>) -> Item {
    let item = Item::switch(Kind::Ftp, ftp.map(|f| f.enabled));
    let plain = item.is_on() && ftp.and_then(|f| f.tls_only) != Some(true);
    item.when_on(Severity::Notice, "storage reachable from the internet")
        .flag(plain, Severity::Warning, "FTP without TLS allowed")
}

fn shared_links(exposure: &Exposure) -> Item {
    let valid: Option<Vec<_>> = exposure
        .shared_links
        .as_ref()
        .map(|links| links.iter().filter(|l| l.valid).collect());
    let forever = valid
        .iter()
        .flatten()
        .any(|link| link.expires.is_none() && link.access_limit.is_none());
    Item::list(Kind::SharedLinks, valid.as_ref().map(Vec::len))
        .when_on(Severity::Notice, "files of the storage shared")
        .flag(forever, Severity::Notice, "links without expiry")
}

fn usp(usp: Option<&Usp>) -> Item {
    let enabled: Option<Vec<_>> = usp.map(|u| u.controllers.iter().filter(|c| c.enabled).collect());
    let mut item = Item::list(Kind::Usp, enabled.as_ref().map(Vec::len))
        .when_on(Severity::Notice, "controllers may manage the device");
    let plain = enabled.iter().flatten().any(|c| c.tls == Some(false));
    if usp.and_then(|u| u.myfritz) == Some(true) {
        item.state = State::On;
        item = item.flag(true, Severity::Notice, "MyFRITZ may manage the device");
    }
    item.flag(plain, Severity::Warning, "controller without TLS")
}

#[cfg(test)]
mod tests {
    use super::super::{Controller, DynamicDns, Link, Service, Tr069, Vpn};
    use super::*;
    use crate::ports::Forwarding;

    fn summary(items: &[Item]) -> Vec<(Kind, State, Option<usize>, Severity)> {
        items
            .iter()
            .map(|i| (i.kind, i.state, i.count, i.severity))
            .collect()
    }

    #[test]
    fn everything_off_is_info() {
        let exposure = Exposure {
            remote_access: Some(RemoteAccess::default()),
            myfritz: Some(MyFritz::default()),
            dynamic_dns: Some(DynamicDns::default()),
            port_forwardings: Some(vec![Forwarding::default()]),
            exposed_host: Some(false),
            upnp: Some(false),
            ftp: Some(Ftp::default()),
            shared_links: Some(vec![Link::default()]),
            vpn: Some(Vec::new()),
            tr069: Some(Tr069::default()),
            usp: Some(Usp::default()),
            ..Exposure::default()
        };
        let items = items(&exposure, true);
        assert_eq!(items.len(), 11);
        for item in &items {
            assert_eq!(item.state, State::Off, "{item:?}");
            assert_eq!(item.severity, Severity::Info, "{item:?}");
            assert!(item.hints.is_empty(), "{item:?}");
        }
    }

    #[test]
    fn missing_parts_are_unknown() {
        let items = items(&Exposure::default(), false);
        for item in &items {
            assert_eq!(item.state, State::Unknown, "{item:?}");
            assert_eq!(item.severity, Severity::Info, "{item:?}");
        }
        let web: Vec<Kind> = items
            .iter()
            .filter(|i| !i.hints.is_empty())
            .map(|i| i.kind)
            .collect();
        assert_eq!(web, [Kind::ExposedHost, Kind::Vpn]);
        assert_eq!(items[4].hints, ["needs a login to the web interface"]);
    }

    #[test]
    fn rates_everything_on() {
        use Severity::{Info, Notice, Warning};
        use State::On;
        let on = Exposure {
            remote_access: Some(RemoteAccess {
                enabled: true,
                port: Some(443),
                lets_encrypt: Some(false),
            }),
            myfritz: Some(MyFritz {
                enabled: true,
                services: vec![Service {
                    enabled: true,
                    ..Service::default()
                }],
                ..MyFritz::default()
            }),
            dynamic_dns: Some(DynamicDns {
                enabled: true,
                ..DynamicDns::default()
            }),
            port_forwardings: Some(vec![
                Forwarding {
                    enabled: true,
                    ..Forwarding::default()
                },
                Forwarding::default(),
            ]),
            exposed_host: Some(true),
            upnp: Some(true),
            ftp: Some(Ftp {
                enabled: true,
                port: None,
                tls_only: Some(false),
            }),
            shared_links: Some(vec![Link {
                valid: true,
                ..Link::default()
            }]),
            vpn: Some(vec![Vpn::default(), Vpn::default()]),
            tr069: Some(Tr069 {
                enabled: true,
                ..Tr069::default()
            }),
            usp: Some(Usp {
                myfritz: Some(true),
                controllers: vec![Controller {
                    enabled: true,
                    tls: Some(false),
                    ..Controller::default()
                }],
            }),
            ..Exposure::default()
        };
        let items = items(&on, true);
        assert_eq!(
            summary(&items),
            [
                (Kind::RemoteAccess, On, None, Notice),
                (Kind::Myfritz, On, Some(1), Notice),
                (Kind::DynamicDns, On, None, Info),
                (Kind::PortForwarding, On, Some(1), Notice),
                (Kind::ExposedHost, On, None, Warning),
                (Kind::Upnp, On, None, Notice),
                (Kind::Ftp, On, None, Warning),
                (Kind::SharedLinks, On, Some(1), Notice),
                (Kind::Vpn, On, Some(2), Info),
                (Kind::Tr069, On, None, Info),
                (Kind::Usp, On, Some(1), Warning),
            ]
        );
        assert_eq!(
            items[0].hints,
            [
                "web interface reachable from the internet",
                "certificate signed by the device itself"
            ]
        );
        assert_eq!(
            items[7].hints,
            ["files of the storage shared", "links without expiry"]
        );
        assert_eq!(
            items[10].hints,
            [
                "controllers may manage the device",
                "MyFRITZ may manage the device",
                "controller without TLS"
            ]
        );
        let exposure = Exposure { items, ..on };
        assert!(exposure.has_warning());
        assert!(!Exposure::default().has_warning());
    }

    #[test]
    fn rates_safer_settings_lower() {
        let exposure = Exposure {
            remote_access: Some(RemoteAccess {
                enabled: true,
                port: None,
                lets_encrypt: Some(true),
            }),
            myfritz: Some(MyFritz {
                enabled: true,
                ..MyFritz::default()
            }),
            ftp: Some(Ftp {
                enabled: true,
                port: None,
                tls_only: Some(true),
            }),
            shared_links: Some(vec![Link {
                valid: true,
                expires: Some("2026-12-01T00:00:00".to_owned()),
                ..Link::default()
            }]),
            usp: Some(Usp {
                myfritz: Some(false),
                controllers: vec![Controller {
                    enabled: true,
                    tls: Some(true),
                    ..Controller::default()
                }],
            }),
            ..Exposure::default()
        };
        let items = items(&exposure, true);
        assert_eq!(
            items[0].hints,
            ["web interface reachable from the internet"]
        );
        assert_eq!(
            (items[1].state, items[1].count, items[1].severity),
            (State::On, Some(0), Severity::Info)
        );
        assert_eq!(items[6].severity, Severity::Notice);
        assert_eq!(items[7].hints, ["files of the storage shared"]);
        assert_eq!(items[10].severity, Severity::Notice);
    }
}
