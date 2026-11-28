//! `fritz ports`, `telephony`, `capabilities` and `settings`: what the
//! device offers and how it is set up, as text, JSON or YAML.

use std::io::Write;

use anyhow::Context as _;
use fritz_core::capabilities::{self, Capabilities};
use fritz_core::ports::{self, Ports};
use fritz_core::settings::{self, Settings};
use fritz_core::telephony::{self, Telephony};

use crate::context::Context;
use crate::output::{Format, show};
use crate::status::{on_off, pairs, reader, table, yes_no};

/// Prints the port forwardings.
pub(crate) async fn ports(
    context: &Context,
    format: Format,
    out: &mut impl Write,
) -> anyhow::Result<()> {
    let mut reader = reader(context).await?;
    let ports = ports::ports(&mut reader)
        .await
        .context("cannot read the port forwardings")?;
    show(&ports, format, out, ports_text)
}

fn ports_text(ports: &Ports, out: &mut impl Write) -> anyhow::Result<()> {
    pairs(&[("UPnP", ports.upnp.map(on_off))], out)?;
    if ports.forwardings.is_empty() {
        writeln!(out, "No port forwardings.")?;
        return Ok(());
    }
    let number = |value: Option<u16>| value.map_or_else(String::new, |v| v.to_string());
    let rows = ports.forwardings.iter().map(|f| {
        let client = f.internal_client.clone().unwrap_or_default();
        vec![
            f.protocol.clone().unwrap_or_default(),
            number(f.external_port),
            format!("{client}:{}", number(f.internal_port)),
            yes_no(f.enabled),
            f.description.clone().unwrap_or_default(),
        ]
    });
    let headers = ["PROTOCOL", "PORT", "TO", "ENABLED", "DESCRIPTION"];
    table(&headers, rows, out)
}

/// Prints the telephony.
pub(crate) async fn telephony(
    context: &Context,
    format: Format,
    out: &mut impl Write,
) -> anyhow::Result<()> {
    let mut reader = reader(context).await?;
    let telephony = telephony::telephony(&mut reader)
        .await
        .context("cannot read the telephony")?;
    show(&telephony, format, out, telephony_tables)
}

/// A table for each part the device has, with a blank line between them.
fn telephony_tables(telephony: &Telephony, out: &mut impl Write) -> anyhow::Result<()> {
    let text = |value: &Option<String>| value.clone().unwrap_or_default();
    let mut tables: Vec<(Vec<&str>, Vec<Vec<String>>)> = Vec::new();
    let mut add = |headers: Vec<&'static str>, rows: Vec<Vec<String>>| {
        if !rows.is_empty() {
            tables.push((headers, rows));
        }
    };
    add(
        vec!["NUMBER", "TYPE", "NAME"],
        telephony
            .numbers
            .iter()
            .map(|n| vec![text(&n.number), text(&n.kind), text(&n.name)])
            .collect(),
    );
    add(
        vec!["ACCOUNT", "NUMBER", "REGISTRAR", "STATUS"],
        telephony
            .accounts
            .iter()
            .map(|a| {
                vec![
                    a.index.to_string(),
                    a.number.clone(),
                    text(&a.registrar),
                    text(&a.status),
                ]
            })
            .collect(),
    );
    add(
        vec!["DEFLECTION", "NUMBER", "TO", "MODE", "ENABLED"],
        telephony
            .deflections
            .iter()
            .map(|d| {
                vec![
                    text(&d.kind),
                    text(&d.number),
                    text(&d.to),
                    text(&d.mode),
                    yes_no(d.enabled),
                ]
            })
            .collect(),
    );
    add(
        vec!["ANSWERING MACHINE", "NAME", "ENABLED"],
        telephony
            .answering_machines
            .iter()
            .map(|m| vec![m.index.to_string(), text(&m.name), yes_no(m.enabled)])
            .collect(),
    );
    add(
        vec!["HANDSET", "NAME", "MODEL", "ACTIVE", "UPDATE"],
        telephony
            .handsets
            .iter()
            .map(|h| {
                vec![
                    text(&h.id),
                    text(&h.name),
                    text(&h.model),
                    yes_no(h.active),
                    h.update_available.map_or_else(String::new, yes_no),
                ]
            })
            .collect(),
    );
    if tables.is_empty() {
        writeln!(out, "No telephony.")?;
    }
    for (i, (headers, rows)) in tables.into_iter().enumerate() {
        if i > 0 {
            writeln!(out)?;
        }
        table(&headers, rows.into_iter(), out)?;
    }
    Ok(())
}

/// Prints the capabilities.
pub(crate) async fn capabilities(
    context: &Context,
    format: Format,
    out: &mut impl Write,
) -> anyhow::Result<()> {
    let mut reader = reader(context).await?;
    let capabilities = capabilities::capabilities(&mut reader)
        .await
        .context("cannot read the capabilities")?;
    show(&capabilities, format, out, |c, out| {
        pairs(&capability_pairs(c), out)
    })
}

fn capability_pairs(capabilities: &Capabilities) -> Vec<(&'static str, Option<String>)> {
    let rights: Vec<String> = capabilities
        .rights
        .iter()
        .map(|r| format!("{}: {}", r.area, r.access))
        .collect();
    let joined = |items: Vec<String>| Some(items.join(", ")).filter(|i| !i.is_empty());
    vec![
        ("Features", joined(capabilities.features.clone())),
        ("Rights", joined(rights)),
        ("Two-factor", capabilities.two_factor.map(on_off)),
        ("Services", Some(capabilities.services.to_string())),
    ]
}

/// Prints the settings of `modules`, or of the common ones.
pub(crate) async fn settings(
    context: &Context,
    modules: &[String],
    format: Format,
    out: &mut impl Write,
) -> anyhow::Result<()> {
    let session = context
        .session()
        .await
        .context("cannot log in to the web interface")?;
    let settings = settings::settings(&session, modules).await;
    // Ends the session even if reading failed.
    session
        .logout()
        .await
        .context("cannot log out of the web interface")?;
    show(&settings, format, out, settings_text)
}

/// One line per value, with its path, like `ipv6.mtu  1492`.
fn settings_text(settings: &Settings, out: &mut impl Write) -> anyhow::Result<()> {
    let mut lines = Vec::new();
    for (name, value) in &settings.modules {
        flatten(name, value, &mut lines);
    }
    for (name, reason) in &settings.unavailable {
        lines.push((name.clone(), format!("unavailable: {reason}")));
    }
    let width = lines.iter().map(|(p, _)| p.len()).max().unwrap_or_default();
    for (path, value) in lines {
        writeln!(out, "{path:width$}  {value}")?;
    }
    Ok(())
}

fn flatten(path: &str, value: &serde_json::Value, lines: &mut Vec<(String, String)>) {
    use serde_json::Value;
    match value {
        Value::Object(object) if !object.is_empty() => {
            for (name, value) in object {
                flatten(&format!("{path}.{name}"), value, lines);
            }
        }
        Value::Array(items) if !items.is_empty() => {
            for (i, value) in items.iter().enumerate() {
                flatten(&format!("{path}[{i}]"), value, lines);
            }
        }
        Value::String(text) => lines.push((path.to_owned(), text.clone())),
        other => lines.push((path.to_owned(), other.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use fritz_core::ports::Forwarding;
    use fritz_core::status::Right;
    use fritz_core::telephony::{Account, AnsweringMachine, Deflection, Handset, Number};

    use super::*;
    use crate::status::tests::text;

    #[test]
    fn prints_ports() {
        let ports = Ports {
            upnp: Some(true),
            forwardings: vec![Forwarding {
                service: "WANPPPConnection1".to_owned(),
                enabled: true,
                protocol: Some("TCP".to_owned()),
                external_port: Some(2222),
                internal_client: Some("192.0.2.1".to_owned()),
                internal_port: Some(22),
                description: Some("ssh".to_owned()),
                ..Forwarding::default()
            }],
            listed: true,
        };
        assert_eq!(
            text(|out| ports_text(&ports, out)),
            "UPnP  on\n\
             PROTOCOL  PORT  TO            ENABLED  DESCRIPTION\n\
             TCP       2222  192.0.2.1:22  yes      ssh\n"
        );
        assert_eq!(
            text(|out| ports_text(&Ports::default(), out)),
            "No port forwardings.\n"
        );
    }

    #[test]
    fn prints_the_telephony() {
        let telephony = Telephony {
            numbers: vec![Number {
                number: Some("phone-1".to_owned()),
                kind: Some("eVoIP".to_owned()),
                name: None,
            }],
            accounts: vec![Account {
                index: 1,
                number: "phone-1".to_owned(),
                registrar: None,
                status: Some("registered".to_owned()),
            }],
            deflections: vec![Deflection {
                enabled: true,
                kind: Some("fromNumber".to_owned()),
                ..Deflection::default()
            }],
            answering_machines: vec![AnsweringMachine {
                index: 0,
                name: Some("name-4".to_owned()),
                enabled: false,
            }],
            handsets: vec![Handset {
                id: Some("1".to_owned()),
                model: Some("FRITZ!Fon C5".to_owned()),
                update_available: Some(false),
                ..Handset::default()
            }],
        };
        assert_eq!(
            text(|out| telephony_tables(&telephony, out)),
            "NUMBER   TYPE   NAME\n\
             phone-1  eVoIP\n\
             \n\
             ACCOUNT  NUMBER   REGISTRAR  STATUS\n\
             1        phone-1             registered\n\
             \n\
             DEFLECTION  NUMBER  TO  MODE  ENABLED\n\
             fromNumber                    yes\n\
             \n\
             ANSWERING MACHINE  NAME    ENABLED\n\
             0                  name-4  no\n\
             \n\
             HANDSET  NAME  MODEL         ACTIVE  UPDATE\n\
             1              FRITZ!Fon C5  no      no\n"
        );
        assert_eq!(
            text(|out| telephony_tables(&Telephony::default(), out)),
            "No telephony.\n"
        );
    }

    #[test]
    fn prints_settings_one_value_per_line() {
        let mut settings = Settings::default();
        settings.modules.insert(
            "ipv6".to_owned(),
            serde_json::json!({"mtu": 1492, "on": true, "dns": ["::1"], "none": [], "route": {}}),
        );
        settings
            .unavailable
            .insert("vpn".to_owned(), "HTTP status 403".to_owned());
        assert_eq!(
            text(|out| settings_text(&settings, out)),
            "ipv6.dns[0]  ::1\n\
             ipv6.mtu     1492\n\
             ipv6.none    []\n\
             ipv6.on      true\n\
             ipv6.route   {}\n\
             vpn          unavailable: HTTP status 403\n"
        );
    }

    #[test]
    fn prints_capabilities() {
        let capabilities = Capabilities {
            features: vec!["wlan".to_owned(), "dect".to_owned()],
            rights: vec![Right {
                area: "BoxAdmin".to_owned(),
                access: "readwrite".to_owned(),
            }],
            two_factor: None,
            services: 37,
        };
        assert_eq!(
            text(|out| pairs(&capability_pairs(&capabilities), out)),
            "Features    wlan, dect\n\
             Rights      BoxAdmin: readwrite\n\
             Services    37\n"
        );
    }
}
