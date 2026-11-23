//! `fritz wlan`: the wireless networks, and with `--clients` the clients of
//! each.

use std::io::Write;

use anyhow::Context as _;
use fritz_core::wlan::{self, Client, Views, Wlan};

use super::{clean, reader, table};
use crate::context::Context;
use crate::output::{Format, show};

/// Prints the wireless networks, with what `views` asks for.
pub(crate) async fn wlan(
    context: &Context,
    views: Views,
    format: Format,
    out: &mut impl Write,
) -> anyhow::Result<()> {
    let mut reader = reader(context).await?;
    let wlans = wlan::wlans(&mut reader, views)
        .await
        .context("cannot read the wireless networks")?;
    if views.clients {
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

/// A line per network, each followed by the table of its clients, like
/// `WLANConfiguration1  wlan-1  2.4 GHz  ch 6  be  Up  3 clients`.
pub(super) fn networks(wlans: &[Wlan], out: &mut impl Write) -> anyhow::Result<()> {
    for (i, wlan) in wlans.iter().enumerate() {
        if i > 0 {
            writeln!(out)?;
        }
        writeln!(out, "{}", clean(&network(wlan)))?;
        let clients = wlan.clients.as_deref().unwrap_or_default();
        if !clients.is_empty() {
            let mut rows = Vec::new();
            client_table(clients, &mut rows)?;
            for line in String::from_utf8(rows)?.lines() {
                writeln!(out, "  {line}")?;
            }
        }
    }
    Ok(())
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
}
