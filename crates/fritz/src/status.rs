//! `fritz overview`, `hosts`, `mesh`, `wlan`, `internet` and `smarthome`:
//! the state of the device as text or JSON.
//!
//! Values from the device are untrusted: control characters in them are
//! replaced in text output, so a host name cannot steer the terminal.

use std::io::Write;

use anyhow::Context as _;
use fritz_core::lists::Mesh;
use fritz_core::read::Reader;
use fritz_core::smarthome::{self, Device, Target, Unit};
use fritz_core::status::{self, Connection, Host, Hosts, Internet, Overview, Wlan};
use serde::Serialize;

use crate::context::Context;

/// How to print the result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub(crate) enum Format {
    Text,
    Json,
}

impl Format {
    /// Text for a terminal, else JSON.
    pub(crate) fn for_terminal(terminal: bool) -> Self {
        if terminal { Self::Text } else { Self::Json }
    }
}

async fn reader(context: &Context) -> anyhow::Result<Reader> {
    let client = context.client()?;
    Reader::new(client)
        .await
        .context("cannot read the device description")
}

/// Prints the overview.
pub(crate) async fn overview(
    context: &Context,
    format: Format,
    out: &mut impl Write,
) -> anyhow::Result<()> {
    let mut reader = reader(context).await?;
    let overview = status::overview(&mut reader)
        .await
        .context("cannot read the overview")?;
    match format {
        Format::Json => json(&overview, out),
        Format::Text => pairs(&overview_pairs(&overview), out),
    }
}

fn overview_pairs(overview: &Overview) -> Vec<(&'static str, Option<String>)> {
    let rights: Vec<String> = overview
        .rights
        .iter()
        .map(|r| format!("{}: {}", r.area, r.access))
        .collect();
    vec![
        ("Model", overview.model.clone()),
        ("Firmware", overview.firmware.clone()),
        ("Uptime", overview.uptime.map(duration)),
        (
            "Update",
            overview
                .update_available
                .map(|a| if a { "available" } else { "none" }.to_owned()),
        ),
        ("Two-factor", overview.two_factor.map(on_off)),
        ("Internet", overview.internet.as_ref().map(summary)),
        ("Rights", Some(rights.join(", ")).filter(|r| !r.is_empty())),
    ]
}

/// The connection in one line, like
/// `Connected over DSL (link Up), 192.0.2.1, for 2h 5m`.
fn summary(connection: &Connection) -> String {
    let status = connection.status.as_deref().unwrap_or("unknown");
    let part = |value: &Option<String>, prefix: &str, suffix: &str| {
        value
            .as_ref()
            .map_or_else(String::new, |v| format!("{prefix}{v}{suffix}"))
    };
    let over = part(&connection.access_type, " over ", "");
    let link = part(&connection.link, " (link ", ")");
    let address = part(&connection.external_address, ", ", "");
    let uptime = part(&connection.uptime.map(duration), ", for ", "");
    format!("{status}{over}{link}{address}{uptime}")
}

/// Prints the internet connection.
pub(crate) async fn internet(
    context: &Context,
    format: Format,
    out: &mut impl Write,
) -> anyhow::Result<()> {
    let mut reader = reader(context).await?;
    let internet = status::internet(&mut reader)
        .await
        .context("cannot read the internet connection")?;
    match format {
        Format::Json => json(&internet, out),
        Format::Text => pairs(&internet_pairs(&internet), out),
    }
}

fn internet_pairs(internet: &Internet) -> Vec<(&'static str, Option<String>)> {
    let connection = &internet.connection;
    let mut pairs = vec![
        ("Status", connection.status.clone()),
        ("Access", connection.access_type.clone()),
        ("Link", connection.link.clone()),
        ("Provider", internet.provider.clone()),
        ("Address", connection.external_address.clone()),
        (
            "DNS servers",
            Some(internet.dns_servers.join(", ")).filter(|d| !d.is_empty()),
        ),
        ("Connected for", connection.uptime.map(duration)),
        (
            "Link speed",
            rates(internet.downstream_max, internet.upstream_max),
        ),
    ];
    if let Some(dsl) = &internet.dsl {
        pairs.extend([
            ("DSL", dsl.status.clone()),
            ("DSL rate", rates(dsl.downstream_rate, dsl.upstream_rate)),
            ("DSL maximum", rates(dsl.downstream_max, dsl.upstream_max)),
            (
                "Noise margin",
                decibels(dsl.downstream_noise_margin, dsl.upstream_noise_margin),
            ),
            (
                "Attenuation",
                decibels(dsl.downstream_attenuation, dsl.upstream_attenuation),
            ),
        ]);
    }
    pairs
}

/// Rates in kbit/s, like `↓ 100 Mbit/s ↑ 40 Mbit/s`.
fn rates(down: Option<u64>, up: Option<u64>) -> Option<String> {
    both(down.map(rate), up.map(rate))
}

/// A rate in kbit/s, like `100.0 Mbit/s` or `800 kbit/s`.
fn rate(kbits: u64) -> String {
    if kbits >= 1000 {
        format!("{:.1} Mbit/s", kbits_to_mbits(kbits))
    } else {
        format!("{kbits} kbit/s")
    }
}

#[expect(clippy::cast_precision_loss, reason = "rates are far below 2^52")]
fn kbits_to_mbits(kbits: u64) -> f64 {
    kbits as f64 / 1000.0
}

fn decibels(down: Option<f64>, up: Option<f64>) -> Option<String> {
    let decibel = |value: f64| format!("{value:.1} dB");
    both(down.map(decibel), up.map(decibel))
}

/// A downstream and an upstream value, like `↓ 7.0 dB ↑ 8.0 dB`.
fn both(down: Option<String>, up: Option<String>) -> Option<String> {
    match (down, up) {
        (Some(down), Some(up)) => Some(format!("↓ {down}  ↑ {up}")),
        (Some(down), None) => Some(format!("↓ {down}")),
        (None, Some(up)) => Some(format!("↑ {up}")),
        (None, None) => None,
    }
}

/// Prints the hosts, all or only the active ones.
pub(crate) async fn hosts(
    context: &Context,
    active: bool,
    format: Format,
    out: &mut impl Write,
) -> anyhow::Result<()> {
    let mut reader = reader(context).await?;
    let mut hosts = status::hosts(&mut reader)
        .await
        .context("cannot read the hosts")?;
    eprint!("{}", shortfall(&hosts));
    if active {
        hosts.hosts.retain(|h| h.active);
    }
    match format {
        Format::Json => json(&hosts, out),
        Format::Text => host_table(&hosts, out),
    }
}

/// A line that tells how many hosts were not read, or nothing.
fn shortfall(hosts: &Hosts) -> String {
    let read = hosts.hosts.len();
    if read < hosts.reported as usize {
        format!(
            "the device reported {} hosts, read {read}\n",
            hosts.reported
        )
    } else {
        String::new()
    }
}

fn host_table(hosts: &Hosts, out: &mut impl Write) -> anyhow::Result<()> {
    let rows = hosts.hosts.iter().map(|h: &Host| {
        vec![
            h.name.clone().unwrap_or_default(),
            h.ip.clone().unwrap_or_default(),
            h.mac.clone().unwrap_or_default(),
            yes_no(h.active),
            h.interface.clone().unwrap_or_default(),
        ]
    });
    table(&["NAME", "IP", "MAC", "ACTIVE", "INTERFACE"], rows, out)
}

/// Prints the mesh: its nodes, and the links between all nodes.
pub(crate) async fn mesh(
    context: &Context,
    format: Format,
    out: &mut impl Write,
) -> anyhow::Result<()> {
    let mut reader = reader(context).await?;
    let mesh = status::mesh(&mut reader)
        .await
        .context("cannot read the mesh")?
        .context("the device has no mesh list, or the user may not read it")?;
    match format {
        Format::Json => json(&mesh, out),
        Format::Text => mesh_tables(&mesh, out),
    }
}

/// The nodes of the mesh, then the links. The rates go down from `FROM` to
/// `TO`, and up the other way.
fn mesh_tables(mesh: &Mesh, out: &mut impl Write) -> anyhow::Result<()> {
    let text = |value: &Option<String>| value.clone().unwrap_or_default();
    let nodes = mesh
        .nodes
        .iter()
        .filter(|n| n.meshed)
        .map(|n| vec![text(&n.name), text(&n.role), text(&n.model), text(&n.mac)]);
    table(&["NODE", "ROLE", "MODEL", "MAC"], nodes, out)?;
    writeln!(out)?;
    let links = mesh.links.iter().map(|l| {
        vec![
            l.from.clone(),
            l.to.clone(),
            text(&l.kind),
            text(&l.interface),
            text(&l.state),
            rates(l.rate_tx, l.rate_rx).unwrap_or_default(),
        ]
    });
    let headers = ["FROM", "TO", "TYPE", "INTERFACE", "STATE", "RATE"];
    table(&headers, links, out)
}

/// Prints the wireless networks.
pub(crate) async fn wlan(
    context: &Context,
    format: Format,
    out: &mut impl Write,
) -> anyhow::Result<()> {
    let mut reader = reader(context).await?;
    let wlans = status::wlans(&mut reader)
        .await
        .context("cannot read the wireless networks")?;
    match format {
        Format::Json => json(&wlans, out),
        Format::Text => wlan_table(&wlans, out),
    }
}

fn wlan_table(wlans: &[Wlan], out: &mut impl Write) -> anyhow::Result<()> {
    let text = |value: Option<String>| value.unwrap_or_default();
    let number = |value: Option<u32>| value.map_or_else(String::new, |v| v.to_string());
    let rows = wlans.iter().map(|w| {
        vec![
            w.service.clone(),
            text(w.ssid.clone()),
            text(w.band.clone()),
            number(w.channel),
            text(w.standard.clone()),
            text(w.status.clone()),
            number(w.clients),
        ]
    });
    let headers = [
        "SERVICE", "SSID", "BAND", "CHANNEL", "STANDARD", "STATUS", "CLIENTS",
    ];
    table(&headers, rows, out)
}

/// Prints the Smart Home devices. Logs in to the web interface for this,
/// and out again.
pub(crate) async fn smarthome(
    context: &Context,
    format: Format,
    out: &mut impl Write,
) -> anyhow::Result<()> {
    let session = context
        .session()
        .await
        .context("cannot log in to the web interface")?;
    let devices = smarthome::devices(&session).await;
    // Ends the session even if reading failed.
    let logout = session.logout().await;
    let devices = devices.context("cannot read the Smart Home devices")?;
    logout.context("cannot log out of the web interface")?;
    match format {
        Format::Json => json(&devices, out),
        Format::Text => smarthome_table(&devices, out),
    }
}

/// One line per unit, so devices with several units, like a plug with a
/// switch and a thermometer in one unit, or a remote with buttons, get
/// several lines.
fn smarthome_table(devices: &[Device], out: &mut impl Write) -> anyhow::Result<()> {
    let empty = [Unit::default()];
    let rows = devices.iter().flat_map(|device| {
        let units = if device.units.is_empty() {
            &empty[..]
        } else {
            &device.units[..]
        };
        units.iter().map(move |unit| {
            let name = if unit.name.is_empty() || unit.name == device.name {
                device.name.clone()
            } else {
                format!("{} / {}", device.name, unit.name)
            };
            let battery = device.battery.map(|b| {
                let low = if device.battery_low == Some(true) {
                    " low"
                } else {
                    ""
                };
                format!("{b}%{low}")
            });
            vec![
                name,
                device.category.clone().unwrap_or_default(),
                yes_no(device.connected),
                unit.on.map(on_off).unwrap_or_default(),
                unit.temperature.map(celsius).unwrap_or_default(),
                unit.target.as_ref().map(target).unwrap_or_default(),
                unit.power.map(|w| format!("{w:.1} W")).unwrap_or_default(),
                unit.energy.map(|wh| format!("{wh} Wh")).unwrap_or_default(),
                battery.unwrap_or_default(),
                other(unit),
            ]
        })
    });
    let headers = [
        "NAME",
        "CATEGORY",
        "CONNECTED",
        "SWITCH",
        "TEMPERATURE",
        "TARGET",
        "POWER",
        "ENERGY",
        "BATTERY",
        "OTHER",
    ];
    table(&headers, rows, out)
}

fn celsius(value: f64) -> String {
    format!("{value:.1} °C")
}

fn target(target: &Target) -> String {
    match target {
        Target::Celsius(value) => celsius(*value),
        Target::Mode(mode) => mode.clone(),
    }
}

/// The values without a column of their own, like `level 25%`.
fn other(unit: &Unit) -> String {
    let mut values = Vec::new();
    values.extend(unit.voltage.map(|v| format!("{v:.1} V")));
    values.extend(unit.level.map(|l| format!("level {l}%")));
    values.extend(unit.humidity.map(|h| format!("humidity {h}%")));
    values.extend(unit.alerts.iter().map(|a| format!("alert {a}")));
    values.join(", ")
}

fn json(value: &impl Serialize, out: &mut impl Write) -> anyhow::Result<()> {
    serde_json::to_writer_pretty(&mut *out, value)?;
    writeln!(out)?;
    Ok(())
}

/// Prints the pairs that have a value, one per line, with aligned values.
fn pairs(pairs: &[(&str, Option<String>)], out: &mut impl Write) -> anyhow::Result<()> {
    let width = pairs.iter().map(|(n, _)| n.len()).max().unwrap_or_default();
    for (name, value) in pairs {
        if let Some(value) = value {
            writeln!(out, "{name:width$}  {}", clean(value))?;
        }
    }
    Ok(())
}

/// Prints a table with aligned columns.
fn table(
    headers: &[&str],
    rows: impl Iterator<Item = Vec<String>>,
    out: &mut impl Write,
) -> anyhow::Result<()> {
    let rows: Vec<Vec<String>> = rows
        .map(|row| row.iter().map(|cell| clean(cell)).collect())
        .collect();
    let mut widths: Vec<usize> = headers.iter().map(|h| h.chars().count()).collect();
    for row in &rows {
        for (width, cell) in widths.iter_mut().zip(row) {
            *width = (*width).max(cell.chars().count());
        }
    }
    let header: Vec<String> = headers.iter().map(|h| (*h).to_owned()).collect();
    for row in std::iter::once(&header).chain(&rows) {
        let cells: Vec<String> = row
            .iter()
            .zip(&widths)
            .map(|(cell, width)| format!("{cell:width$}"))
            .collect();
        writeln!(out, "{}", cells.join("  ").trim_end())?;
    }
    Ok(())
}

/// The text with control characters replaced, so that it cannot steer the
/// terminal.
fn clean(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_control() { '\u{FFFD}' } else { c })
        .collect()
}

/// Seconds as days, hours and minutes, like `2d 3h 4m`.
fn duration(seconds: u64) -> String {
    let (days, rest) = (seconds / 86_400, seconds % 86_400);
    let (hours, minutes) = (rest / 3600, rest % 3600 / 60);
    match (days, hours) {
        (0, 0) => format!("{minutes}m"),
        (0, _) => format!("{hours}h {minutes}m"),
        _ => format!("{days}d {hours}h {minutes}m"),
    }
}

fn on_off(on: bool) -> String {
    if on { "on" } else { "off" }.to_owned()
}

fn yes_no(yes: bool) -> String {
    if yes { "yes" } else { "no" }.to_owned()
}

#[cfg(test)]
mod tests {
    use fritz_core::lists::{Link, Node};
    use fritz_core::status::{Dsl, Right};

    use super::*;

    fn text(write: impl FnOnce(&mut Vec<u8>) -> anyhow::Result<()>) -> String {
        let mut out = Vec::new();
        write(&mut out).unwrap();
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn prints_text_only_to_terminals() {
        assert_eq!(Format::for_terminal(true), Format::Text);
        assert_eq!(Format::for_terminal(false), Format::Json);
    }

    #[test]
    fn formats_durations() {
        assert_eq!(duration(59), "0m");
        assert_eq!(duration(3 * 3600 + 120), "3h 2m");
        assert_eq!(duration(2 * 86_400 + 3600 + 60), "2d 1h 1m");
    }

    #[test]
    fn formats_rates_and_decibels() {
        assert_eq!(
            rates(Some(276_310), Some(800)).as_deref(),
            Some("↓ 276.3 Mbit/s  ↑ 800 kbit/s")
        );
        assert_eq!(rates(Some(1000), None).as_deref(), Some("↓ 1.0 Mbit/s"));
        assert_eq!(rates(None, Some(1)).as_deref(), Some("↑ 1 kbit/s"));
        assert_eq!(rates(None, None), None);
        assert_eq!(
            decibels(Some(7.0), Some(8.5)).as_deref(),
            Some("↓ 7.0 dB  ↑ 8.5 dB")
        );
    }

    #[test]
    fn replaces_control_characters() {
        assert_eq!(clean("a\u{1b}[2Jb\n"), "a\u{FFFD}[2Jb\u{FFFD}");
        let output = text(|out| {
            table(
                &["A", "B"],
                [vec!["\u{7}".to_owned(), String::new()]].into_iter(),
                out,
            )
        });
        assert_eq!(output, "A  B\n\u{FFFD}\n");
    }

    #[test]
    fn prints_smart_home_devices_as_text() {
        let devices = [
            Device {
                ain: "1".to_owned(),
                name: "Heating".to_owned(),
                product: None,
                category: Some("thermostat".to_owned()),
                firmware: None,
                connected: true,
                battery: Some(9),
                battery_low: Some(true),
                units: vec![Unit {
                    name: "Valve".to_owned(),
                    temperature: Some(20.25),
                    target: Some(Target::Mode("off".to_owned())),
                    ..Unit::default()
                }],
            },
            Device {
                ain: "2".to_owned(),
                name: "Plug".to_owned(),
                product: None,
                category: None,
                firmware: None,
                connected: false,
                battery: Some(100),
                battery_low: None,
                units: vec![Unit {
                    name: "Plug".to_owned(),
                    on: Some(true),
                    target: Some(Target::Celsius(21.0)),
                    power: Some(1.25),
                    energy: Some(12),
                    voltage: Some(230.1),
                    level: Some(5),
                    humidity: Some(40),
                    alerts: vec!["open".to_owned()],
                    ..Unit::default()
                }],
            },
            Device {
                ain: "3".to_owned(),
                name: "Remote".to_owned(),
                product: None,
                category: None,
                firmware: None,
                connected: true,
                battery: None,
                battery_low: None,
                units: Vec::new(),
            },
        ];
        let output = text(|out| smarthome_table(&devices, out));
        assert_eq!(
            output,
            "NAME             CATEGORY    CONNECTED  SWITCH  TEMPERATURE  TARGET   POWER  ENERGY  BATTERY  OTHER\n\
             Heating / Valve  thermostat  yes                20.2 °C      off                     9% low\n\
             Plug                         no         on                   21.0 °C  1.2 W  12 Wh   100%     \
             230.1 V, level 5%, humidity 40%, alert open\n\
             Remote                       yes\n"
        );
    }

    #[test]
    fn prints_the_overview_as_text() {
        let mut overview = Overview {
            model: Some("FRITZ!Box 7690".to_owned()),
            uptime: Some(90),
            update_available: Some(true),
            two_factor: Some(true),
            rights: vec![Right {
                area: "NAS".to_owned(),
                access: "read".to_owned(),
            }],
            internet: Some(Connection::default()),
            ..Overview::default()
        };
        let output = text(|out| pairs(&overview_pairs(&overview), out));
        assert_eq!(
            output,
            "Model       FRITZ!Box 7690\n\
             Uptime      1m\n\
             Update      available\n\
             Two-factor  on\n\
             Internet    unknown\n\
             Rights      NAS: read\n"
        );
        overview.update_available = Some(false);
        overview.two_factor = Some(false);
        overview.rights.clear();
        let output = text(|out| pairs(&overview_pairs(&overview), out));
        assert!(
            output.contains("Update      none\nTwo-factor  off\n"),
            "{output}"
        );
        assert!(!output.contains("Rights"), "{output}");
    }

    #[test]
    fn prints_the_internet_connection_as_text() {
        let mut internet = Internet {
            connection: Connection {
                access_type: Some("DSL".to_owned()),
                link: Some("Up".to_owned()),
                status: Some("Connected".to_owned()),
                external_address: Some("192.0.2.1".to_owned()),
                uptime: Some(3600),
            },
            dsl: Some(Dsl {
                downstream_noise_margin: Some(7.0),
                ..Dsl::default()
            }),
            ..Internet::default()
        };
        assert_eq!(
            summary(&internet.connection),
            "Connected over DSL (link Up), 192.0.2.1, for 1h 0m"
        );
        let output = text(|out| pairs(&internet_pairs(&internet), out));
        assert!(output.contains("Noise margin   ↓ 7.0 dB\n"), "{output}");
        assert!(!output.contains("DNS servers"), "{output}");
        internet.dsl = None;
        let output = text(|out| pairs(&internet_pairs(&internet), out));
        assert!(!output.contains("Noise"), "{output}");
    }

    #[test]
    fn prints_tables() {
        let hosts = Hosts {
            reported: 2,
            hosts: vec![
                Host {
                    name: Some("device-1".to_owned()),
                    active: true,
                    ..Host::default()
                },
                Host::default(),
            ],
        };
        assert_eq!(shortfall(&hosts), "");
        let output = text(|out| host_table(&hosts, out));
        assert_eq!(
            output,
            "NAME      IP  MAC  ACTIVE  INTERFACE\n\
             device-1           yes\n\
             \x20                  no\n"
        );
        let fewer = Hosts {
            reported: 3,
            ..hosts
        };
        assert_eq!(shortfall(&fewer), "the device reported 3 hosts, read 2\n");
        let mesh = Mesh {
            nodes: vec![
                Node {
                    name: Some("box".to_owned()),
                    role: Some("master".to_owned()),
                    meshed: true,
                    ..Node::default()
                },
                Node {
                    name: Some("host".to_owned()),
                    ..Node::default()
                },
            ],
            links: vec![Link {
                from: "box".to_owned(),
                to: "host".to_owned(),
                rate_rx: Some(800),
                rate_tx: Some(2000),
                ..Link::default()
            }],
        };
        let output = text(|out| mesh_tables(&mesh, out));
        assert_eq!(
            output,
            "NODE  ROLE    MODEL  MAC\n\
             box   master\n\
             \n\
             FROM  TO    TYPE  INTERFACE  STATE  RATE\n\
             box   host                          ↓ 2.0 Mbit/s  ↑ 800 kbit/s\n"
        );
        let wlans = [Wlan {
            service: "WLANConfiguration1".to_owned(),
            channel: Some(6),
            ..Wlan::default()
        }];
        let output = text(|out| wlan_table(&wlans, out));
        assert!(
            output.ends_with("WLANConfiguration1              6\n"),
            "{output}"
        );
    }
}
