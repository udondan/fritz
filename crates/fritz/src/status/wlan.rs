//! `fritz wlan`: the wireless networks, with `--clients` the clients of
//! each, and with `--details` their security settings and more.

use std::io::Write;

use anyhow::{Context as _, bail};
use fritz_core::wlan::{self, Client, Details, Views, Wlan};

use super::{clean, on_off, pairs, reader, table};
use crate::context::Context;
use crate::output::{Format, show};

/// Prints the wireless networks, or the ones `network` names, with what
/// `views` asks for. Fails if `network` names none.
pub(crate) async fn wlan(
    context: &Context,
    network: Option<&str>,
    views: Views,
    format: Format,
    out: &mut impl Write,
) -> anyhow::Result<()> {
    let mut reader = reader(context).await?;
    let wlans = match network {
        Some(network) => wlan::named(&mut reader, network, views).await,
        None => wlan::wlans(&mut reader, views).await,
    }
    .context("cannot read the wireless networks")?;
    if let Some(network) = network
        && wlans.is_empty()
    {
        bail!("no wireless network is named {network:?}");
    }
    if views.clients || views.details {
        show(wlans.as_slice(), format, out, networks)
    } else {
        show(wlans.as_slice(), format, out, wlan_table)
    }
}

/// One line per network.
pub(super) fn wlan_table(wlans: &[Wlan], out: &mut impl Write) -> anyhow::Result<()> {
    let text = |value: Option<String>| value.unwrap_or_default();
    let rows = wlans.iter().map(|w| {
        vec![
            w.service.clone(),
            text(w.ssid.clone()),
            guest(w).to_owned(),
            text(w.band.clone()),
            number(w.channel),
            text(w.standard.clone()),
            text(w.status.clone()),
            number(w.client_count),
        ]
    });
    let headers = [
        "SERVICE", "SSID", "TYPE", "BAND", "CHANNEL", "STANDARD", "STATUS", "CLIENTS",
    ];
    table(&headers, rows, out)
}

/// A line per network, like
/// `WLANConfiguration1  wlan-1  2.4 GHz  ch 6  be  Up  3 clients`, each
/// followed by its details and the table of its clients.
pub(super) fn networks(wlans: &[Wlan], out: &mut impl Write) -> anyhow::Result<()> {
    for (i, wlan) in wlans.iter().enumerate() {
        if i > 0 {
            writeln!(out)?;
        }
        writeln!(out, "{}", clean(&network(wlan)))?;
        if let Some(details) = &wlan.details {
            let mut lines = Vec::new();
            let shown: Vec<_> = detail_pairs(details)
                .into_iter()
                .filter(|(_, value)| value.is_some())
                .collect();
            pairs(&shown, &mut lines)?;
            indent(&lines, out)?;
        }
        let clients = wlan.clients.as_deref().unwrap_or_default();
        if !clients.is_empty() {
            let mut rows = Vec::new();
            client_table(clients, &mut rows)?;
            indent(&rows, out)?;
        }
    }
    Ok(())
}

/// Writes the lines of `text` indented.
fn indent(text: &[u8], out: &mut impl Write) -> anyhow::Result<()> {
    for line in std::str::from_utf8(text)?.lines() {
        writeln!(out, "  {line}")?;
    }
    Ok(())
}

/// The details worth a line: the encryption, channel, BSSID and packets,
/// and the rest only where it differs from the defaults, like a hidden
/// SSID, WPS that is on or a night time schedule.
fn detail_pairs(details: &Details) -> Vec<(&'static str, Option<String>)> {
    let security = &details.security;
    let radio = &details.radio;
    let channel = radio.channel.map(|c| match radio.auto_channel {
        Some(true) => format!("{c}, automatic"),
        Some(false) => format!("{c}, fixed"),
        None => c.to_string(),
    });
    let wps = &details.wps;
    let wps = (wps.enabled == Some(true)).then(|| {
        [Some("on"), wps.mode.as_deref(), wps.status.as_deref()]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(", ")
    });
    let statistics = &details.statistics;
    let packets = match (statistics.packets_sent, statistics.packets_received) {
        (None, None) => None,
        (sent, received) => {
            let count = |n: Option<u64>| n.map_or_else(|| "?".to_owned(), |n| n.to_string());
            Some(format!(
                "{} sent, {} received",
                count(sent),
                count(received)
            ))
        }
    };
    let hints: Vec<String> = details.hints.iter().map(ToString::to_string).collect();
    vec![
        (
            "Security",
            security
                .encryption
                .clone()
                .or_else(|| security.beacon_type.clone()),
        ),
        (
            "Hidden SSID",
            (security.hidden == Some(true)).then(|| "yes".to_owned()),
        ),
        (
            "MAC filter",
            (security.mac_filter == Some(true)).then(|| on_off(true)),
        ),
        ("Channel", channel),
        ("BSSID", radio.bssid.clone()),
        ("WPS", wps),
        ("Night time", details.night.schedule.clone()),
        ("Packets", packets),
        ("Hints", (!hints.is_empty()).then(|| hints.join(", "))),
    ]
}

/// The line of a network.
fn network(wlan: &Wlan) -> String {
    let count = match wlan.client_count {
        Some(1) => Some("1 client".to_owned()),
        Some(n) => Some(format!("{n} clients")),
        None => None,
    };
    let parts = [
        Some(wlan.service.clone()),
        wlan.ssid.clone(),
        Some(guest(wlan).to_owned()),
        wlan.band.clone(),
        wlan.channel.map(|c| format!("ch {c}")),
        wlan.standard.clone(),
        wlan.status.clone(),
        count,
    ];
    let parts: Vec<String> = parts
        .into_iter()
        .flatten()
        .filter(|p| !p.is_empty())
        .collect();
    parts.join("  ")
}

fn client_table(clients: &[Client], out: &mut impl Write) -> anyhow::Result<()> {
    let text = |value: &Option<String>| value.clone().unwrap_or_default();
    let rows = clients.iter().map(|c| {
        vec![
            text(&c.name),
            text(&c.ip),
            text(&c.mac),
            c.signal.map_or_else(String::new, |s| format!("{s} %")),
            c.rate.map_or_else(String::new, |r| format!("{r} Mbit/s")),
        ]
    });
    let headers = ["NAME", "IP", "MAC", "SIGNAL", "RATE"];
    table(&headers, rows, out)
}

/// `guest` for the guest network.
fn guest(wlan: &Wlan) -> &'static str {
    if wlan.guest { "guest" } else { "" }
}

fn number(value: Option<u32>) -> String {
    value.map_or_else(String::new, |v| v.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::status::tests::text;

    #[test]
    fn prints_networks_and_their_clients() {
        let wlans = [
            Wlan {
                service: "WLANConfiguration1".to_owned(),
                ssid: Some("wlan-1".to_owned()),
                band: Some("2.4 GHz".to_owned()),
                channel: Some(6),
                status: Some("Up".to_owned()),
                client_count: Some(1),
                clients: Some(vec![Client {
                    name: Some("device-\u{1b}1".to_owned()),
                    mac: Some("AA:BB:CC:00:00:01".to_owned()),
                    signal: Some(71),
                    rate: Some(72),
                    ..Client::default()
                }]),
                ..Wlan::default()
            },
            Wlan {
                service: "WLANConfiguration3".to_owned(),
                ssid: Some("wlan-\u{7}2".to_owned()),
                guest: true,
                client_count: Some(0),
                clients: Some(Vec::new()),
                ..Wlan::default()
            },
            Wlan {
                service: "WLANConfiguration4".to_owned(),
                ..Wlan::default()
            },
        ];
        assert_eq!(
            text(|out| networks(&wlans, out)),
            "WLANConfiguration1  wlan-1  2.4 GHz  ch 6  Up  1 client\n\
             \x20 NAME       IP  MAC                SIGNAL  RATE\n\
             \x20 device-\u{FFFD}1      AA:BB:CC:00:00:01  71 %    72 Mbit/s\n\
             \n\
             WLANConfiguration3  wlan-\u{FFFD}2  guest  0 clients\n\
             \n\
             WLANConfiguration4\n"
        );
        let output = text(|out| wlan_table(&wlans, out));
        assert!(
            output.contains("\nWLANConfiguration3  wlan-\u{FFFD}2  guest"),
            "{output}"
        );
    }

    #[test]
    fn prints_the_details_that_differ_from_the_defaults() {
        use fritz_core::wlan::{Hint, Night, Radio, Security, Statistics, Wps};

        let wlan = |details| Wlan {
            service: "WLANConfiguration1".to_owned(),
            details: Some(details),
            ..Wlan::default()
        };
        let weak = Details {
            security: Security {
                beacon_type: Some("WPAand11i".to_owned()),
                encryption: Some("WPA/WPA2 mixed, TKIP and AES".to_owned()),
                hidden: Some(true),
                mac_filter: Some(true),
                ..Security::default()
            },
            radio: Radio {
                channel: Some(6),
                auto_channel: Some(true),
                bssid: Some("AA:BB:CC:00:00:01".to_owned()),
                ..Radio::default()
            },
            wps: Wps {
                enabled: Some(true),
                mode: Some("pbc".to_owned()),
                status: Some("active".to_owned()),
            },
            night: Night {
                schedule: Some("00:00-\u{1b}06:00".to_owned()),
                no_forced_off: Some(true),
            },
            statistics: Statistics {
                packets_sent: Some(10),
                packets_received: None,
            },
            hints: vec![Hint::TkipAllowed, Hint::WpsEnabled, Hint::HiddenSsid],
        };
        assert_eq!(
            text(|out| networks(&[wlan(weak)], out)),
            "WLANConfiguration1\n\
             \x20 Security     WPA/WPA2 mixed, TKIP and AES\n\
             \x20 Hidden SSID  yes\n\
             \x20 MAC filter   on\n\
             \x20 Channel      6, automatic\n\
             \x20 BSSID        AA:BB:CC:00:00:01\n\
             \x20 WPS          on, pbc, active\n\
             \x20 Night time   00:00-\u{FFFD}06:00\n\
             \x20 Packets      10 sent, ? received\n\
             \x20 Hints        TKIP allowed, WPS enabled, hidden SSID\n"
        );
        let plain = Details {
            security: Security {
                beacon_type: Some("Other".to_owned()),
                hidden: Some(false),
                ..Security::default()
            },
            radio: Radio {
                channel: Some(48),
                auto_channel: Some(false),
                ..Radio::default()
            },
            wps: Wps {
                enabled: Some(false),
                ..Wps::default()
            },
            ..Details::default()
        };
        let mut unknown = plain.clone();
        unknown.radio.auto_channel = None;
        assert_eq!(
            text(|out| networks(&[wlan(plain), wlan(unknown)], out)),
            "WLANConfiguration1\n\
             \x20 Security  Other\n\
             \x20 Channel   48, fixed\n\
             \n\
             WLANConfiguration1\n\
             \x20 Security  Other\n\
             \x20 Channel   48\n"
        );
    }
}
