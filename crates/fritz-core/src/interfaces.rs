//! The counters of the LAN interfaces and the wireless networks, and the
//! LAN ports of the web interface, through actions and requests that only
//! read.
//!
//! The counters only grow, until they wrap around or the device restarts.
//! [`Interfaces::since`] turns two reads into the changes and rates in
//! between.

use serde::Serialize;
use serde_json::Value;

use crate::generic::Error;
use crate::read::{Reader, Values};
use crate::session::{self, Session};
use crate::settings;

/// The LAN interfaces, the LAN ports and the wireless networks.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Interfaces {
    pub lan: Vec<Lan>,
    /// The LAN ports, from the web interface. They have no counters.
    pub ports: Vec<Port>,
    /// Why the LAN ports could not be read.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ports_unavailable: Option<String>,
    pub wlan: Vec<Wireless>,
}

/// A LAN interface and its counters.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Lan {
    /// The service, like `LANEthernetInterfaceConfig1`.
    pub service: String,
    pub enabled: Option<bool>,
    /// Like `Up` or `NoLink`.
    pub status: Option<String>,
    /// The speed, like `Auto` or `1000`.
    pub speed: Option<String>,
    /// Like `Auto`, `Half` or `Full`.
    pub duplex: Option<String>,
    #[serde(flatten)]
    pub counters: Counters,
}

/// A port of the LAN, from the module `eth_ports` of the web interface.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Port {
    /// Like `LAN 1`.
    pub name: Option<String>,
    pub connected: Option<bool>,
    /// The speed of the link in Mbit/s, 0 without a link.
    pub speed: Option<u32>,
    /// The highest speed of the port in Mbit/s.
    pub max_speed: Option<u32>,
    /// Energy Efficient Ethernet, like `on` or `off`.
    pub energy_saving: Option<String>,
    /// Like `default` or `green`.
    pub mode: Option<String>,
}

/// A wireless network and its counters.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Wireless {
    /// The service, like `WLANConfiguration1`.
    pub service: String,
    pub ssid: Option<String>,
    /// Like `2.4 GHz`.
    pub band: Option<String>,
    pub channel: Option<u32>,
    /// Like `Up` or `Disabled`.
    pub status: Option<String>,
    #[serde(flatten)]
    pub counters: Counters,
}

/// The counters of an interface, where the device reports them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct Counters {
    pub bytes_sent: Option<u64>,
    pub bytes_received: Option<u64>,
    pub packets_sent: Option<u64>,
    pub packets_received: Option<u64>,
    pub errors_sent: Option<u64>,
    pub errors_received: Option<u64>,
    pub discarded_sent: Option<u64>,
    pub discarded_received: Option<u64>,
}

impl Counters {
    /// The counters in the answers of statistics actions. The first answer
    /// that has a counter wins.
    fn from_values(answers: &[Values]) -> Self {
        let find = |names: &[&str]| {
            answers
                .iter()
                .find_map(|values| names.iter().find_map(|name| values.number(name)))
        };
        Self {
            bytes_sent: find(&["NewBytesSent", "NewTotalBytesSent"]),
            bytes_received: find(&["NewBytesReceived", "NewTotalBytesReceived"]),
            packets_sent: find(&["NewPacketsSent", "NewTotalPacketsSent"]),
            packets_received: find(&["NewPacketsReceived", "NewTotalPacketsReceived"]),
            errors_sent: find(&["NewErrorsSent"]),
            errors_received: find(&["NewErrorsReceived"]),
            discarded_sent: find(&["NewDiscardPacketsSent"]),
            discarded_received: find(&["NewDiscardPacketsReceived"]),
        }
    }

    /// How much each counter grew since `earlier`, see [`grown`].
    #[must_use]
    pub fn since(&self, earlier: &Self) -> Self {
        let grown = |now: Option<u64>, then: Option<u64>| grown(then?, now?);
        Self {
            bytes_sent: grown(self.bytes_sent, earlier.bytes_sent),
            bytes_received: grown(self.bytes_received, earlier.bytes_received),
            packets_sent: grown(self.packets_sent, earlier.packets_sent),
            packets_received: grown(self.packets_received, earlier.packets_received),
            errors_sent: grown(self.errors_sent, earlier.errors_sent),
            errors_received: grown(self.errors_received, earlier.errors_received),
            discarded_sent: grown(self.discarded_sent, earlier.discarded_sent),
            discarded_received: grown(self.discarded_received, earlier.discarded_received),
        }
    }
}

/// How much a counter grew from `then` to `now`. A counter that fell
/// wrapped around at 2^32 if it fit into 32 bits, else the device reset it,
/// like after a restart, and the growth is unknown.
pub fn grown(then: u64, now: u64) -> Option<u64> {
    if now >= then {
        Some(now - then)
    } else if u32::try_from(then).is_ok() {
        Some((1 << 32) - then + now)
    } else {
        None
    }
}

/// What changed on the interfaces between two reads.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Sample {
    /// The seconds between the reads.
    pub seconds: f64,
    pub interfaces: Vec<Rate>,
}

/// What changed on an interface between two reads.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Rate {
    /// The service, like `WLANConfiguration1`.
    pub service: String,
    /// The SSID of a wireless network.
    pub ssid: Option<String>,
    pub status: Option<String>,
    /// How much the counters grew, see [`grown`].
    pub changes: Counters,
    pub bytes_sent_per_second: Option<f64>,
    pub bytes_received_per_second: Option<f64>,
    pub packets_sent_per_second: Option<f64>,
    pub packets_received_per_second: Option<f64>,
}

impl Rate {
    fn new(service: &str, now: &Counters, earlier: &Counters, seconds: f64) -> Self {
        let changes = now.since(earlier);
        let rate = |count: Option<u64>| count.map(|c| per_second(c, seconds));
        Self {
            service: service.to_owned(),
            bytes_sent_per_second: rate(changes.bytes_sent),
            bytes_received_per_second: rate(changes.bytes_received),
            packets_sent_per_second: rate(changes.packets_sent),
            packets_received_per_second: rate(changes.packets_received),
            changes,
            ..Self::default()
        }
    }
}

#[expect(clippy::cast_precision_loss, reason = "rates need no exact counts")]
fn per_second(count: u64, seconds: f64) -> f64 {
    if seconds > 0.0 {
        count as f64 / seconds
    } else {
        0.0
    }
}

impl Interfaces {
    /// What changed since `earlier`, read `seconds` before. Interfaces that
    /// were not in both reads are left out.
    #[must_use]
    pub fn since(&self, earlier: &Self, seconds: f64) -> Sample {
        let lan = self.lan.iter().filter_map(|now| {
            let then = earlier.lan.iter().find(|l| l.service == now.service)?;
            Some(Rate {
                status: now.status.clone(),
                ..Rate::new(&now.service, &now.counters, &then.counters, seconds)
            })
        });
        let wlan = self.wlan.iter().filter_map(|now| {
            let then = earlier.wlan.iter().find(|w| w.service == now.service)?;
            Some(Rate {
                ssid: now.ssid.clone(),
                status: now.status.clone(),
                ..Rate::new(&now.service, &now.counters, &then.counters, seconds)
            })
        });
        Sample {
            seconds,
            interfaces: lan.chain(wlan).collect(),
        }
    }
}

/// Reads the LAN interfaces and the wireless networks with their counters,
/// without the ports.
///
/// # Errors
///
/// The errors of reading other than missing parts, see
/// [`Reader::try_read`].
pub async fn interfaces(reader: &mut Reader) -> Result<Interfaces, Error> {
    let mut lan = Vec::new();
    for service in reader.services("LANEthernetInterfaceConfig") {
        let info = reader.try_read(&service, "GetInfo", &[]).await?;
        let statistics = reader.try_read(&service, "GetStatistics", &[]).await?;
        if info.is_none() && statistics.is_none() {
            continue;
        }
        let info = info.unwrap_or_default();
        lan.push(Lan {
            enabled: info.flag("NewEnable"),
            status: info.text("NewStatus"),
            speed: info.text("NewMaxBitRate"),
            duplex: info.text("NewDuplexMode"),
            counters: Counters::from_values(&statistics.into_iter().collect::<Vec<_>>()),
            service,
        });
    }
    let mut wlan = Vec::new();
    for service in reader.services("WLANConfiguration") {
        let Some(info) = reader.try_read(&service, "GetInfo", &[]).await? else {
            continue;
        };
        let mut answers = Vec::new();
        for action in ["GetStatistics", "GetPacketStatistics"] {
            answers.extend(reader.try_read(&service, action, &[]).await?);
        }
        wlan.push(Wireless {
            ssid: info.text("NewSSID"),
            band: info
                .get("NewX_AVM-DE_FrequencyBand")
                .and_then(crate::wlan::band),
            channel: info.number("NewChannel"),
            status: info.text("NewStatus"),
            counters: Counters::from_values(&answers),
            service,
        });
    }
    Ok(Interfaces {
        lan,
        wlan,
        ..Interfaces::default()
    })
}

/// Reads the LAN ports from the module `eth_ports` of the web interface.
///
/// # Errors
///
/// The errors of [`settings::module`].
pub async fn ports(session: &Session) -> Result<Vec<Port>, session::Error> {
    Ok(parse_ports(&settings::module(session, "eth_ports").await?))
}

/// The ports in the module `eth_ports`, like
/// `{"eth": [{"label": "LAN 1", "carrier": "1", "speed": "1000", …}]}`.
fn parse_ports(module: &Value) -> Vec<Port> {
    let Some(ports) = module.get("eth").and_then(Value::as_array) else {
        return Vec::new();
    };
    let text = |port: &Value, name| {
        port.get(name)
            .and_then(Value::as_str)
            .filter(|t| !t.is_empty())
            .map(str::to_owned)
    };
    let number = |port: &Value, name| text(port, name)?.trim().parse().ok();
    ports
        .iter()
        .map(|port| Port {
            name: text(port, "label"),
            connected: text(port, "carrier").map(|c| c == "1"),
            speed: number(port, "speed"),
            max_speed: number(port, "maxspeed"),
            energy_saving: text(port, "eee_mode"),
            mode: text(port, "mode"),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn counts_growth_across_wrap_arounds() {
        assert_eq!(grown(5, 7), Some(2));
        assert_eq!(grown(7, 7), Some(0));
        // A 32-bit counter wrapped around.
        assert_eq!(grown(u64::from(u32::MAX) - 1, 3), Some(5));
        // A 64-bit counter fell: the device restarted.
        assert_eq!(grown(1 << 40, 3), None);
    }

    fn values(pairs: &[(&str, &str)]) -> Values {
        Values(
            pairs
                .iter()
                .map(|(n, v)| ((*n).to_owned(), (*v).to_owned()))
                .collect(),
        )
    }

    #[test]
    fn reads_counters_from_several_answers() {
        let first = values(&[("NewTotalPacketsSent", "10"), ("NewErrorsSent", "x")]);
        let second = values(&[
            ("NewTotalPacketsSent", "20"),
            ("NewErrorsSent", "2"),
            ("NewDiscardPacketsReceived", "3"),
        ]);
        let counters = Counters::from_values(&[first, second]);
        assert_eq!(counters.packets_sent, Some(10));
        assert_eq!(counters.errors_sent, Some(2));
        assert_eq!(counters.discarded_received, Some(3));
        assert_eq!(counters.bytes_sent, None);
    }

    #[test]
    fn computes_rates_between_reads() {
        let earlier = Interfaces {
            lan: vec![Lan {
                service: "LANEthernetInterfaceConfig1".to_owned(),
                counters: Counters {
                    bytes_sent: Some(1000),
                    bytes_received: Some(u64::from(u32::MAX)),
                    errors_received: Some(4),
                    ..Counters::default()
                },
                ..Lan::default()
            }],
            wlan: vec![Wireless {
                service: "WLANConfiguration1".to_owned(),
                ..Wireless::default()
            }],
            ..Interfaces::default()
        };
        let mut now = earlier.clone();
        now.lan[0].status = Some("Up".to_owned());
        now.lan[0].counters.bytes_sent = Some(3000);
        now.lan[0].counters.bytes_received = Some(9);
        now.lan[0].counters.errors_received = Some(5);
        now.wlan[0].ssid = Some("wlan-1".to_owned());
        now.wlan.push(Wireless {
            service: "WLANConfiguration2".to_owned(),
            ..Wireless::default()
        });
        let sample = now.since(&earlier, 2.0);
        assert_eq!(sample.seconds, 2.0);
        assert_eq!(sample.interfaces.len(), 2, "{sample:?}");
        let lan = &sample.interfaces[0];
        assert_eq!(lan.status.as_deref(), Some("Up"));
        assert_eq!(lan.changes.bytes_sent, Some(2000));
        assert_eq!(lan.changes.bytes_received, Some(10));
        assert_eq!(lan.changes.errors_received, Some(1));
        assert_eq!(lan.bytes_sent_per_second, Some(1000.0));
        assert_eq!(lan.bytes_received_per_second, Some(5.0));
        assert_eq!(lan.packets_sent_per_second, None);
        assert_eq!(sample.interfaces[1].ssid.as_deref(), Some("wlan-1"));
        assert_eq!(per_second(10, 0.0), 0.0);
    }

    #[test]
    fn parses_ports() {
        let module = json!({ "eth": [
            { "UID": "eth0", "carrier": "1", "eee_mode": "on", "label": "LAN 1",
              "maxspeed": "2500", "mode": "default", "speed": "1000" },
            { "carrier": "0", "label": "", "speed": "x" },
        ]});
        assert_eq!(
            parse_ports(&module),
            [
                Port {
                    name: Some("LAN 1".to_owned()),
                    connected: Some(true),
                    speed: Some(1000),
                    max_speed: Some(2500),
                    energy_saving: Some("on".to_owned()),
                    mode: Some("default".to_owned()),
                },
                Port {
                    connected: Some(false),
                    ..Port::default()
                },
            ]
        );
        assert_eq!(parse_ports(&json!({})), []);
    }
}
