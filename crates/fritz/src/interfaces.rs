//! `fritz interfaces`: the counters of the LAN interfaces and the wireless
//! networks, and the LAN ports, once or with `--watch` as rates.

use std::io::Write;

use anyhow::Context as _;
use fritz_core::interfaces::{self, Counters, Interfaces, Port, Rate, Sample};
use fritz_core::read::Reader;

use crate::context::Context;
use crate::output::{Format, show};
use crate::status::{self, pairs, table, yes_no};
use crate::watch;

/// Prints the interfaces and their counters, or with `--watch` their rates
/// in every interval. Only a single read logs in to the web interface, for
/// the LAN ports.
pub(crate) async fn interfaces(
    context: &Context,
    options: watch::Options,
    format: Format,
    out: &mut impl Write,
) -> anyhow::Result<()> {
    let mut reader = status::reader(context).await?;
    if !options.watch {
        let interfaces = snapshot(context, &mut reader).await?;
        return show(&interfaces, format, out, snapshot_text);
    }
    watch::run(options, async || read(&mut reader).await, format, out).await
}

impl watch::Reading for Interfaces {
    type Sample = Sample;

    fn since(&self, before: &Self, seconds: f64) -> Sample {
        Interfaces::since(self, before, seconds)
    }

    fn text(sample: &Sample, out: &mut impl Write) -> anyhow::Result<()> {
        sample_text(sample, out)
    }
}

/// Reads the interfaces with their counters, and the LAN ports. The ports
/// need a login to the web interface; if it fails, the reason is in
/// [`Interfaces::ports_unavailable`].
pub(crate) async fn snapshot(context: &Context, reader: &mut Reader) -> anyhow::Result<Interfaces> {
    let mut interfaces = read(reader).await?;
    match ports(context).await {
        Ok(ports) => interfaces.ports = ports,
        Err(error) => interfaces.ports_unavailable = Some(format!("{error:#}")),
    }
    Ok(interfaces)
}

async fn read(reader: &mut Reader) -> anyhow::Result<Interfaces> {
    interfaces::interfaces(reader)
        .await
        .context("cannot read the interfaces")
}

/// Reads the LAN ports through the web interface, and logs out again.
async fn ports(context: &Context) -> anyhow::Result<Vec<Port>> {
    let session = context
        .session()
        .await
        .context("cannot log in to the web interface")?;
    let ports = interfaces::ports(&session).await;
    // Ends the session even if reading failed.
    let logout = session.logout().await;
    let ports = ports.context("cannot read the LAN ports")?;
    logout.context("cannot log out of the web interface")?;
    Ok(ports)
}

fn snapshot_text(interfaces: &Interfaces, out: &mut impl Write) -> anyhow::Result<()> {
    let number = |value: Option<u64>| value.map_or_else(String::new, count);
    let row =
        |service: &str, name: Option<&String>, status: Option<&String>, link, c: &Counters| {
            vec![
                service.to_owned(),
                name.cloned().unwrap_or_default(),
                status.cloned().unwrap_or_default(),
                link,
                c.bytes_received.map_or_else(String::new, bytes),
                c.bytes_sent.map_or_else(String::new, bytes),
                number(c.packets_received),
                number(c.packets_sent),
                both(c.errors_received, c.errors_sent, count),
                both(c.discarded_received, c.discarded_sent, count),
            ]
        };
    let lan = interfaces.lan.iter().map(|l| {
        let speed = l.speed.as_deref().map(|s| match s.parse::<u32>() {
            Ok(mbits) => format!("{mbits} Mbit/s"),
            Err(_) => s.to_owned(),
        });
        let duplex = l.duplex.as_ref().map(|d| format!("{d} duplex"));
        let link = [speed, duplex].into_iter().flatten().collect::<Vec<_>>();
        row(
            &l.service,
            None,
            l.status.as_ref(),
            link.join(", "),
            &l.counters,
        )
    });
    let wlan = interfaces.wlan.iter().map(|w| {
        let channel = w.channel.map(|c| format!("channel {c}"));
        let link = [w.band.clone(), channel]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>();
        row(
            &w.service,
            w.ssid.as_ref(),
            w.status.as_ref(),
            link.join(", "),
            &w.counters,
        )
    });
    let headers = [
        "INTERFACE",
        "NAME",
        "STATUS",
        "LINK",
        "RECEIVED",
        "SENT",
        "PACKETS RECEIVED",
        "PACKETS SENT",
        "ERRORS",
        "DISCARDED",
    ];
    table(&headers, lan.chain(wlan), out)?;
    if !interfaces.ports.is_empty() {
        writeln!(out)?;
        port_table(&interfaces.ports, out)?;
    }
    if let Some(reason) = &interfaces.ports_unavailable {
        writeln!(out)?;
        let note = [("LAN ports", Some(format!("unavailable: {reason}")))];
        pairs(&note, out)?;
    }
    Ok(())
}

fn port_table(ports: &[Port], out: &mut impl Write) -> anyhow::Result<()> {
    let mbits = |value: Option<u32>| {
        value
            .filter(|v| *v > 0)
            .map_or_else(String::new, |v| format!("{v} Mbit/s"))
    };
    let rows = ports.iter().map(|p| {
        vec![
            p.name.clone().unwrap_or_default(),
            p.connected.map_or_else(String::new, yes_no),
            mbits(p.speed),
            mbits(p.max_speed),
            p.energy_saving.clone().unwrap_or_default(),
        ]
    });
    let headers = ["PORT", "CONNECTED", "SPEED", "MAXIMUM", "ENERGY SAVING"];
    table(&headers, rows, out)
}

fn sample_text(sample: &Sample, out: &mut impl Write) -> anyhow::Result<()> {
    let bits = |value: Option<f64>| value.map_or_else(String::new, |v| bit_rate(v * 8.0));
    let packets = |value: Option<f64>| value.map_or_else(String::new, |v| format!("{v:.1}/s"));
    let grown = |v: u64| format!("+{v}");
    let rows = sample.interfaces.iter().map(|r: &Rate| {
        let c = &r.changes;
        vec![
            r.service.clone(),
            r.ssid.clone().unwrap_or_default(),
            r.status.clone().unwrap_or_default(),
            bits(r.bytes_received_per_second),
            bits(r.bytes_sent_per_second),
            packets(r.packets_received_per_second),
            packets(r.packets_sent_per_second),
            both(c.errors_received, c.errors_sent, grown),
            both(c.discarded_received, c.discarded_sent, grown),
        ]
    });
    let headers = [
        "INTERFACE",
        "NAME",
        "STATUS",
        "RECEIVED",
        "SENT",
        "PACKETS RECEIVED",
        "PACKETS SENT",
        "ERRORS",
        "DISCARDED",
    ];
    table(&headers, rows, out)
}

fn count(value: u64) -> String {
    value.to_string()
}

/// A received and a sent value, like `↓ 3  ↑ 0`.
fn both(received: Option<u64>, sent: Option<u64>, show: impl Fn(u64) -> String) -> String {
    match (received.map(&show), sent.map(&show)) {
        (Some(received), Some(sent)) => format!("↓ {received}  ↑ {sent}"),
        (Some(received), None) => format!("↓ {received}"),
        (None, Some(sent)) => format!("↑ {sent}"),
        (None, None) => String::new(),
    }
}

/// Bytes with a unit, like `876.4 GB`.
#[expect(clippy::cast_precision_loss, reason = "one decimal is enough")]
pub(crate) fn bytes(count: u64) -> String {
    scaled(count as f64, "B")
}

/// Bits per second with a unit, like `1.2 Mbit/s`.
pub(crate) fn bit_rate(bits: f64) -> String {
    scaled(bits, "bit/s")
}

/// A value with a decimal prefix, like `1.2 M` for 1 234 567.
fn scaled(value: f64, unit: &str) -> String {
    let mut value = value;
    for prefix in ["", "k", "M", "G", "T"] {
        if value < 1000.0 {
            return if prefix.is_empty() {
                format!("{value:.0} {unit}")
            } else {
                format!("{value:.1} {prefix}{unit}")
            };
        }
        value /= 1000.0;
    }
    format!("{value:.1} P{unit}")
}

#[cfg(test)]
mod tests {
    use fritz_core::interfaces::{Lan, Wireless};

    use super::*;
    use crate::status::tests::text;

    #[test]
    fn scales_units() {
        assert_eq!(bytes(999), "999 B");
        assert_eq!(bytes(876_448_304_265), "876.4 GB");
        assert_eq!(bytes(2_000_000_000_000_000), "2.0 PB");
        assert_eq!(bit_rate(1_234_567.0), "1.2 Mbit/s");
        assert_eq!(bit_rate(0.4), "0 bit/s");
    }

    #[test]
    fn prints_received_and_sent() {
        assert_eq!(both(Some(3), Some(0), count), "↓ 3  ↑ 0");
        assert_eq!(both(Some(3), None, count), "↓ 3");
        assert_eq!(both(None, Some(1), |v| format!("+{v}")), "↑ +1");
        assert_eq!(both(None, None, count), "");
    }

    #[test]
    fn prints_the_interfaces_as_text() {
        let interfaces = Interfaces {
            lan: vec![
                Lan {
                    service: "LANEthernetInterfaceConfig1".to_owned(),
                    status: Some("Up".to_owned()),
                    speed: Some("1000".to_owned()),
                    duplex: Some("Full".to_owned()),
                    counters: Counters {
                        bytes_received: Some(1500),
                        packets_received: Some(3),
                        errors_received: Some(1),
                        errors_sent: Some(0),
                        ..Counters::default()
                    },
                    ..Lan::default()
                },
                Lan {
                    service: "LANEthernetInterfaceConfig2".to_owned(),
                    speed: Some("Auto".to_owned()),
                    ..Lan::default()
                },
            ],
            wlan: vec![Wireless {
                service: "WLANConfiguration1".to_owned(),
                ssid: Some("wlan-1".to_owned()),
                band: Some("5 GHz".to_owned()),
                channel: Some(36),
                ..Wireless::default()
            }],
            ports: vec![
                Port {
                    name: Some("LAN 1".to_owned()),
                    connected: Some(true),
                    speed: Some(1000),
                    max_speed: Some(2500),
                    energy_saving: Some("on".to_owned()),
                    mode: None,
                },
                Port {
                    name: Some("LAN 2".to_owned()),
                    speed: Some(0),
                    ..Port::default()
                },
            ],
            ports_unavailable: None,
        };
        let output = text(|out| snapshot_text(&interfaces, out));
        assert_eq!(
            output,
            "INTERFACE                    NAME    STATUS  LINK                      RECEIVED  SENT  \
             PACKETS RECEIVED  PACKETS SENT  ERRORS    DISCARDED\n\
             LANEthernetInterfaceConfig1          Up      1000 Mbit/s, Full duplex  1.5 kB          \
             3                               ↓ 1  ↑ 0\n\
             LANEthernetInterfaceConfig2                  Auto\n\
             WLANConfiguration1           wlan-1          5 GHz, channel 36\n\
             \n\
             PORT   CONNECTED  SPEED        MAXIMUM      ENERGY SAVING\n\
             LAN 1  yes        1000 Mbit/s  2500 Mbit/s  on\n\
             LAN 2\n"
        );
        let unavailable = Interfaces {
            ports_unavailable: Some("cannot log in".to_owned()),
            ..Interfaces::default()
        };
        let output = text(|out| snapshot_text(&unavailable, out));
        assert!(
            output.ends_with("\nLAN ports  unavailable: cannot log in\n"),
            "{output}"
        );
    }

    #[test]
    fn prints_samples_as_text() {
        let sample = Sample {
            seconds: 5.0,
            interfaces: vec![
                Rate {
                    service: "WLANConfiguration1".to_owned(),
                    ssid: Some("wlan-1".to_owned()),
                    status: Some("Up".to_owned()),
                    changes: Counters {
                        discarded_received: Some(2),
                        ..Counters::default()
                    },
                    bytes_received_per_second: Some(150_000.0),
                    bytes_sent_per_second: Some(10.0),
                    packets_received_per_second: Some(12.25),
                    packets_sent_per_second: None,
                },
                Rate::default(),
            ],
        };
        let output = text(|out| sample_text(&sample, out));
        assert_eq!(
            output,
            "INTERFACE           NAME    STATUS  RECEIVED    SENT      PACKETS RECEIVED  \
             PACKETS SENT  ERRORS  DISCARDED\n\
             WLANConfiguration1  wlan-1  Up      1.2 Mbit/s  80 bit/s  12.2/s            \
             \x20                     ↓ +2\n\n"
        );
    }
}
