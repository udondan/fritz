//! `fritz exposure`: what of the home network can be reached from the
//! internet or managed from outside.

use std::io::Write;

use anyhow::Context as _;
use fritz_core::exposure::{self, Exposure, Item, Kind, Severity, State};
use fritz_core::ports::Forwarding;

use super::{clean, on_off, reader, table};
use crate::context::Context;
use crate::output::{Format, show};

/// The exit code of `fritz exposure --strict` when an item has the
/// severity `warning`.
pub(crate) const WARNING: i32 = 3;

/// Prints what can be reached from the internet. Returns whether an item
/// has the severity `warning`.
pub(crate) async fn exposure(
    context: &Context,
    format: Format,
    out: &mut impl Write,
) -> anyhow::Result<bool> {
    let mut reader = reader(context).await?;
    // The exposed host and VPN need the web interface; without a login
    // they are unknown.
    let session = context.session().await.ok();
    let exposure = exposure::exposure(&mut reader, session)
        .await
        .context("cannot read what is reachable from the internet")?;
    show(&exposure, format, out, text)?;
    Ok(exposure.has_warning())
}

/// The items as a table, then their hints.
fn text(exposure: &Exposure, out: &mut impl Write) -> anyhow::Result<()> {
    let rows = exposure.items.iter().map(|item| {
        vec![
            item.kind.label().to_owned(),
            state(item),
            severity(item.severity).to_owned(),
            details(exposure, item.kind).join(", "),
        ]
    });
    table(&["ITEM", "STATE", "LEVEL", "DETAILS"], rows, out)?;
    let hints: Vec<&Item> = exposure
        .items
        .iter()
        .filter(|i| !i.hints.is_empty())
        .collect();
    if !hints.is_empty() {
        writeln!(out)?;
    }
    for item in hints {
        let line = format!("{}: {}", item.kind.label(), item.hints.join(", "));
        writeln!(out, "{:7}  {}", severity(item.severity), clean(&line))?;
    }
    Ok(())
}

/// The state, or for lists the number of active entries.
fn state(item: &Item) -> String {
    let list = matches!(
        item.kind,
        Kind::PortForwarding | Kind::SharedLinks | Kind::Vpn | Kind::Usp
    );
    match (item.state, item.count) {
        (State::Unknown, _) => "unknown".to_owned(),
        (_, Some(n)) if list && n > 0 => n.to_string(),
        (State::Off, _) if list => "none".to_owned(),
        (state, _) => on_off(state == State::On),
    }
}

fn severity(severity: Severity) -> &'static str {
    match severity {
        Severity::Info => "info",
        Severity::Notice => "notice",
        Severity::Warning => "warning",
    }
}

/// What is known about an item, like the port of the remote access.
fn details(exposure: &Exposure, kind: Kind) -> Vec<String> {
    match kind {
        Kind::RemoteAccess => remote_access(exposure),
        Kind::Myfritz => myfritz(exposure),
        Kind::DynamicDns => dynamic_dns(exposure),
        Kind::PortForwarding => forwardings(exposure),
        Kind::Ftp => ftp(exposure),
        Kind::SharedLinks => links(exposure),
        Kind::Vpn => vpn(exposure),
        Kind::Tr069 => tr069(exposure),
        Kind::Usp => usp(exposure),
        Kind::ExposedHost | Kind::Upnp => Vec::new(),
    }
}

fn remote_access(exposure: &Exposure) -> Vec<String> {
    let Some(access) = exposure.remote_access.as_ref().filter(|a| a.enabled) else {
        return Vec::new();
    };
    let mut details = Vec::new();
    details.extend(access.port.map(|p| format!("HTTPS port {p}")));
    details.extend(
        access
            .lets_encrypt
            .map(|on| format!("Let's Encrypt {}", on_off(on))),
    );
    details
}

fn myfritz(exposure: &Exposure) -> Vec<String> {
    let Some(myfritz) = exposure.myfritz.as_ref().filter(|m| m.enabled) else {
        return Vec::new();
    };
    let mut details = Vec::new();
    details.extend(myfritz.registered.map(|registered| {
        if registered {
            "registered"
        } else {
            "not registered"
        }
        .to_owned()
    }));
    details.extend(myfritz.address.clone());
    let services: Vec<String> = myfritz
        .services
        .iter()
        .filter(|s| s.enabled)
        .map(|s| {
            let name = s.name.as_deref().unwrap_or("service");
            match (&s.scheme, s.port) {
                (Some(scheme), Some(port)) => format!("{name} {scheme}/{port}"),
                (_, Some(port)) => format!("{name} {port}"),
                _ => name.to_owned(),
            }
        })
        .collect();
    details.push(match services.len() {
        0 => "no services shared".to_owned(),
        n => format!("{n} shared: {}", services.join(" ")),
    });
    details
}

fn dynamic_dns(exposure: &Exposure) -> Vec<String> {
    let Some(dns) = exposure.dynamic_dns.as_ref().filter(|d| d.enabled) else {
        return Vec::new();
    };
    dns.domain.iter().chain(&dns.provider).cloned().collect()
}

fn forwardings(exposure: &Exposure) -> Vec<String> {
    let all = exposure.port_forwardings.iter().flatten();
    all.filter(|f| f.enabled).map(forwarding).collect()
}

/// A forwarding like `nas 443/tcp to 192.168.178.10:443`.
fn forwarding(forwarding: &Forwarding) -> String {
    let number = |port: Option<u16>| port.map_or_else(|| "?".to_owned(), |p| p.to_string());
    let protocol = forwarding
        .protocol
        .as_deref()
        .unwrap_or("?")
        .to_ascii_lowercase();
    let mut text = format!("{}/{protocol}", number(forwarding.external_port));
    if let Some(description) = &forwarding.description {
        text = format!("{description} {text}");
    }
    if let Some(client) = &forwarding.internal_client {
        text = format!("{text} to {client}:{}", number(forwarding.internal_port));
    }
    text
}

fn ftp(exposure: &Exposure) -> Vec<String> {
    let Some(ftp) = exposure.ftp.as_ref().filter(|f| f.enabled) else {
        return Vec::new();
    };
    let mut details = Vec::new();
    details.extend(ftp.port.map(|p| format!("port {p}")));
    details.extend(
        ftp.tls_only
            .map(|only| if only { "TLS only" } else { "TLS optional" }.to_owned()),
    );
    details
}

fn links(exposure: &Exposure) -> Vec<String> {
    let valid = exposure.shared_links.iter().flatten().filter(|l| l.valid);
    valid
        .map(|link| {
            let expiry = link
                .expires
                .as_ref()
                .map_or_else(|| "no expiry".to_owned(), |date| format!("expires {date}"));
            match &link.path {
                Some(path) => format!("{path} {expiry}"),
                None => expiry,
            }
        })
        .collect()
}

fn vpn(exposure: &Exposure) -> Vec<String> {
    let connections = exposure.vpn.iter().flatten();
    connections.filter_map(|c| c.name.clone()).collect()
}

fn tr069(exposure: &Exposure) -> Vec<String> {
    let Some(tr069) = exposure.tr069.as_ref().filter(|t| t.enabled) else {
        return Vec::new();
    };
    let mut details = vec!["provider management".to_owned()];
    details.extend(tr069.url.clone());
    details.extend(
        tr069
            .periodic_inform
            .map(|on| format!("periodic inform {}", on_off(on))),
    );
    details
}

fn usp(exposure: &Exposure) -> Vec<String> {
    let Some(usp) = &exposure.usp else {
        return Vec::new();
    };
    let mut details: Vec<String> = usp
        .controllers
        .iter()
        .filter(|c| c.enabled)
        .map(|c| {
            let host = c.host.as_deref().unwrap_or("controller");
            let tls = if c.tls == Some(true) { "TLS" } else { "no TLS" };
            match c.port {
                Some(port) => format!("{host}:{port} {tls}"),
                None => format!("{host} {tls}"),
            }
        })
        .collect();
    if usp.myfritz == Some(true) {
        details.push("MyFRITZ".to_owned());
    }
    details
}

#[cfg(test)]
mod tests {
    use fritz_core::exposure::{
        Controller, DynamicDns, Ftp, Link, MyFritz, RemoteAccess, Service, Tr069, Usp, Vpn,
    };

    use super::*;

    fn text(exposure: &Exposure) -> String {
        let mut out = Vec::new();
        super::text(exposure, &mut out).unwrap();
        String::from_utf8(out).unwrap()
    }

    fn some(text: &str) -> Option<String> {
        Option::from(text.to_owned())
    }

    fn item(kind: Kind, state: State, count: Option<usize>, severity: Severity) -> Item {
        Item {
            kind,
            state,
            count,
            severity,
            hints: Vec::new(),
        }
    }

    fn items_on() -> Vec<Item> {
        let mut items = vec![
            item(Kind::RemoteAccess, State::On, None, Severity::Notice),
            item(Kind::Myfritz, State::On, Some(1), Severity::Notice),
            item(Kind::DynamicDns, State::On, None, Severity::Info),
            item(Kind::PortForwarding, State::On, Some(2), Severity::Notice),
            item(Kind::ExposedHost, State::On, None, Severity::Warning),
            item(Kind::Upnp, State::Unknown, None, Severity::Info),
            item(Kind::Ftp, State::On, None, Severity::Warning),
            item(Kind::SharedLinks, State::On, Some(2), Severity::Notice),
            item(Kind::Vpn, State::On, Some(2), Severity::Info),
            item(Kind::Tr069, State::On, None, Severity::Info),
            item(Kind::Usp, State::On, Some(2), Severity::Warning),
        ];
        items[4].hints = vec!["all ports of one host are open".to_owned()];
        items
    }

    fn myfritz_on() -> MyFritz {
        MyFritz {
            enabled: true,
            registered: Some(true),
            address: some("box.example"),
            services: vec![
                Service {
                    name: some("nas"),
                    enabled: true,
                    scheme: some("https"),
                    port: Some(443),
                },
                Service {
                    name: None,
                    enabled: true,
                    scheme: None,
                    port: Some(22),
                },
                Service {
                    name: some("cam"),
                    enabled: true,
                    ..Service::default()
                },
                Service::default(),
            ],
        }
    }

    fn usp_on() -> Usp {
        Usp {
            myfritz: Some(true),
            controllers: vec![
                Controller {
                    enabled: true,
                    host: some("usp.example"),
                    port: Some(443),
                    tls: Some(true),
                },
                Controller {
                    enabled: true,
                    ..Controller::default()
                },
            ],
        }
    }

    #[test]
    fn prints_everything_on() {
        let forwarding = |description: Option<String>, client: Option<String>| Forwarding {
            enabled: true,
            protocol: some("TCP"),
            external_port: Some(443),
            internal_client: client,
            internal_port: Some(443),
            description,
            ..Forwarding::default()
        };
        let exposure = Exposure {
            items: items_on(),
            remote_access: Some(RemoteAccess {
                enabled: true,
                port: Some(44321),
                lets_encrypt: Some(true),
            }),
            myfritz: Some(myfritz_on()),
            dynamic_dns: Some(DynamicDns {
                enabled: true,
                domain: some("home.example"),
                provider: some("dyn"),
            }),
            port_forwardings: Some(vec![
                forwarding(some("nas"), some("192.168.178.10")),
                Forwarding {
                    protocol: None,
                    external_port: None,
                    ..forwarding(None, None)
                },
                Forwarding::default(),
            ]),
            exposed_host: Some(true),
            upnp: None,
            ftp: Some(Ftp {
                enabled: true,
                port: Some(21),
                tls_only: Some(false),
            }),
            shared_links: Some(vec![
                Link {
                    valid: true,
                    path: some("/photos"),
                    expires: some("2026-12-01"),
                    ..Link::default()
                },
                Link {
                    valid: true,
                    ..Link::default()
                },
            ]),
            vpn: Some(vec![
                Vpn {
                    name: some("phone"),
                },
                Vpn { name: None },
            ]),
            tr069: Some(Tr069 {
                enabled: true,
                url: some("https://acs.example"),
                periodic_inform: Some(true),
            }),
            usp: Some(usp_on()),
        };
        assert_eq!(
            text(&exposure),
            "ITEM             STATE    LEVEL    DETAILS\n\
             Remote access    on       notice   HTTPS port 44321, Let's Encrypt on\n\
             MyFRITZ          on       notice   registered, box.example, \
             3 shared: nas https/443 service 22 cam\n\
             Dynamic DNS      on       info     home.example, dyn\n\
             Port forwarding  2        notice   nas 443/tcp to 192.168.178.10:443, ?/?\n\
             Exposed host     on       warning\n\
             UPnP             unknown  info\n\
             Storage via FTP  on       warning  port 21, TLS optional\n\
             Shared links     2        notice   /photos expires 2026-12-01, no expiry\n\
             VPN              2        info     phone\n\
             TR-069           on       info     provider management, https://acs.example, \
             periodic inform on\n\
             USP              2        warning  usp.example:443 TLS, controller no TLS, MyFRITZ\n\
             \n\
             warning  Exposed host: all ports of one host are open\n"
        );
    }

    #[test]
    fn prints_everything_off() {
        let off = |kind| item(kind, State::Off, None, Severity::Info);
        let exposure = Exposure {
            items: vec![
                off(Kind::RemoteAccess),
                off(Kind::Myfritz),
                item(Kind::PortForwarding, State::Off, Some(0), Severity::Info),
                off(Kind::Vpn),
                item(Kind::Usp, State::On, Some(0), Severity::Notice),
            ],
            myfritz: Some(MyFritz {
                enabled: true,
                registered: Some(false),
                ..MyFritz::default()
            }),
            usp: Some(Usp::default()),
            ..Exposure::default()
        };
        assert_eq!(
            text(&exposure),
            "ITEM             STATE  LEVEL   DETAILS\n\
             Remote access    off    info\n\
             MyFRITZ          off    info    not registered, no services shared\n\
             Port forwarding  none   info\n\
             VPN              none   info\n\
             USP              on     notice\n"
        );
    }
}
