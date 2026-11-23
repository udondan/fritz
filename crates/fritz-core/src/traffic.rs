//! The traffic of the internet connection: the current rates, how much of
//! the line they use, the split by priority and multicast, and the byte and
//! packet counters, read with the getters of `WANCommonInterfaceConfig`
//! only.
//!
//! The online monitor reports bytes per second as lists, the latest value
//! first; these are shown as bits per second. The counters only grow, until
//! they wrap around or the connection restarts. [`Traffic::since`] turns two
//! reads into the averages in between, see [`grown`].

use serde::Serialize;

use crate::generic::Error;
use crate::interfaces::{Counters, grown, per_second};
use crate::read::{Reader, Values};

/// The service with the traffic of the internet connection.
const SERVICE: &str = "WANCommonInterfaceConfig";

/// The traffic of the internet connection.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Traffic {
    /// Whether the device has the service with the traffic. Repeaters have
    /// not.
    pub supported: bool,
    pub downstream: Flow,
    pub upstream: Flow,
}

/// The traffic in one direction. Values the device does not report are
/// `None`.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Flow {
    /// The latest rate the device measured, in bit/s.
    pub current_bps: Option<u64>,
    /// The highest rate the line allows now, in bit/s.
    pub max_bps: Option<u64>,
    /// The current rate in percent of the highest.
    pub utilization_percent: Option<f64>,
    /// The highest of the recent rates the device keeps, in bit/s.
    pub peak_bps: Option<u64>,
    /// The bytes since the connection started.
    pub total_bytes: Option<u64>,
    /// The packets since the connection started.
    pub total_packets: Option<u64>,
    /// Of the downstream, the latest rate of multicast, like IPTV, in bit/s.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub multicast_bps: Option<u64>,
    /// Of the upstream, the latest rates by priority class.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub priorities: Option<Priorities>,
}

/// The latest upstream rates by priority class, in bit/s.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct Priorities {
    pub realtime_bps: Option<u64>,
    pub high_bps: Option<u64>,
    pub default_bps: Option<u64>,
    pub low_bps: Option<u64>,
}

/// The names of the values of one direction.
struct Names {
    /// The rates of the online monitor in bytes/s, the latest first.
    monitor: &'static str,
    /// The highest rate of the online monitor in bytes/s.
    max: &'static str,
    /// The rates of `GetCommonLinkProperties` in bytes/s, the latest first.
    utilization: &'static str,
    /// The rate of the physical line in bit/s.
    layer1: &'static str,
}

const DOWNSTREAM: Names = Names {
    monitor: "Newds_current_bps",
    max: "Newmax_ds",
    utilization: "NewX_AVM-DE_DownstreamCurrentUtilization",
    layer1: "NewLayer1DownstreamMaxBitRate",
};

const UPSTREAM: Names = Names {
    monitor: "Newus_current_bps",
    max: "Newmax_us",
    utilization: "NewX_AVM-DE_UpstreamCurrentUtilization",
    layer1: "NewLayer1UpstreamMaxBitRate",
};

/// Reads the traffic of the internet connection. Actions the device lacks,
/// like the online monitor on older firmware, leave their values `None`.
///
/// # Errors
///
/// The errors of reading other than missing parts, see
/// [`Reader::try_read`].
pub async fn traffic(reader: &mut Reader) -> Result<Traffic, Error> {
    let Some(service) = reader.services(SERVICE).into_iter().next() else {
        return Ok(Traffic::default());
    };
    let index = [("NewSyncGroupIndex", "0")];
    let monitor = reader.try_read(&service, "X_AVM-DE_GetOnlineMonitor", &index);
    let monitor = monitor.await?.unwrap_or_default();
    let link = reader.try_read(&service, "GetCommonLinkProperties", &[]);
    let link = link.await?.unwrap_or_default();
    let mut totals = Vec::new();
    for action in [
        "GetTotalBytesSent",
        "GetTotalBytesReceived",
        "GetTotalPacketsSent",
        "GetTotalPacketsReceived",
    ] {
        totals.extend(reader.try_read(&service, action, &[]).await?);
    }
    Ok(from_values(&monitor, &link, &totals))
}

/// The traffic in the answers of the online monitor, of
/// `GetCommonLinkProperties` and of the counters.
fn from_values(monitor: &Values, link: &Values, totals: &[Values]) -> Traffic {
    let counters = Counters::from_values(totals);
    let mut downstream = flow(monitor, link, &DOWNSTREAM);
    downstream.total_bytes = counters.bytes_received;
    downstream.total_packets = counters.packets_received;
    downstream.multicast_bps = latest(monitor, "Newmc_current_bps");
    let mut upstream = flow(monitor, link, &UPSTREAM);
    upstream.total_bytes = counters.bytes_sent;
    upstream.total_packets = counters.packets_sent;
    let priorities = Priorities {
        realtime_bps: latest(monitor, "Newprio_realtime_bps"),
        high_bps: latest(monitor, "Newprio_high_bps"),
        default_bps: latest(monitor, "Newprio_default_bps"),
        low_bps: latest(monitor, "Newprio_low_bps"),
    };
    upstream.priorities = (priorities != Priorities::default()).then_some(priorities);
    Traffic {
        supported: true,
        downstream,
        upstream,
    }
}

/// The rates of one direction: from the online monitor, else from
/// `GetCommonLinkProperties`.
fn flow(monitor: &Values, link: &Values, names: &Names) -> Flow {
    let rates = history(monitor, names.monitor);
    let rates = if rates.is_empty() {
        history(link, names.utilization)
    } else {
        rates
    };
    let current_bps = rates.first().copied();
    let max_bps = monitor
        .number(names.max)
        .map(bits)
        .or_else(|| link.number(names.layer1));
    Flow {
        current_bps,
        max_bps,
        utilization_percent: share(current_bps.map(u64_to_f64), max_bps),
        peak_bps: rates.into_iter().max(),
        ..Flow::default()
    }
}

/// The latest value of a list of rates in bytes/s, in bit/s.
fn latest(values: &Values, name: &str) -> Option<u64> {
    history(values, name).first().copied()
}

/// A list of rates in bytes/s, the latest first, like `40712,218205`, in
/// bit/s. Values that are not numbers are left out.
fn history(values: &Values, name: &str) -> Vec<u64> {
    values.get(name).map_or_else(Vec::new, |list| {
        list.split(',')
            .filter_map(|v| v.trim().parse().ok())
            .map(bits)
            .collect()
    })
}

fn bits(bytes: u64) -> u64 {
    bytes.saturating_mul(8)
}

/// A rate in bit/s as a float, to compute and show it.
#[must_use]
#[expect(clippy::cast_precision_loss, reason = "rates need no exact counts")]
pub fn u64_to_f64(value: u64) -> f64 {
    value as f64
}

/// `rate` in percent of `max`, with one decimal.
fn share(rate: Option<f64>, max: Option<u64>) -> Option<f64> {
    let max = u64_to_f64(max.filter(|m| *m > 0)?);
    Some((rate? * 1000.0 / max).round() / 10.0)
}

/// The traffic between two reads.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Sample {
    /// The seconds between the reads.
    pub seconds: f64,
    pub downstream: Rate,
    pub upstream: Rate,
}

/// The traffic in one direction between two reads.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Rate {
    /// The average from the byte counters, in bit/s. `None` without
    /// counters, or if one was reset in between, like after a reconnect.
    pub average_bps: Option<f64>,
    /// The average of packets per second.
    pub packets_per_second: Option<f64>,
    /// The latest rate the device measured at the second read, in bit/s.
    pub current_bps: Option<u64>,
    /// The highest rate the line allows, in bit/s.
    pub max_bps: Option<u64>,
    /// The average, else the latest rate, in percent of the highest.
    pub utilization_percent: Option<f64>,
    /// The bytes in between.
    pub bytes: Option<u64>,
}

impl Rate {
    fn new(now: &Flow, then: &Flow, seconds: f64) -> Self {
        let grown = |now: Option<u64>, then: Option<u64>| grown(then?, now?);
        let bytes = grown(now.total_bytes, then.total_bytes);
        let average_bps = bytes.map(|b| per_second(b, seconds) * 8.0);
        let packets = grown(now.total_packets, then.total_packets);
        let rate = average_bps.or(now.current_bps.map(u64_to_f64));
        Self {
            average_bps,
            packets_per_second: packets.map(|p| per_second(p, seconds)),
            current_bps: now.current_bps,
            max_bps: now.max_bps,
            utilization_percent: share(rate, now.max_bps),
            bytes,
        }
    }
}

impl Traffic {
    /// The traffic since `earlier`, read `seconds` before.
    #[must_use]
    pub fn since(&self, earlier: &Self, seconds: f64) -> Sample {
        Sample {
            seconds,
            downstream: Rate::new(&self.downstream, &earlier.downstream, seconds),
            upstream: Rate::new(&self.upstream, &earlier.upstream, seconds),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::read::values;

    fn monitor() -> Values {
        values(&[
            ("Newds_current_bps", "1000,4000,x"),
            ("Newus_current_bps", "100, 300"),
            ("Newmax_ds", "2000000"),
            ("Newmax_us", "0"),
            ("Newmc_current_bps", "250,0"),
            ("Newprio_realtime_bps", "10"),
            ("Newprio_default_bps", "90"),
        ])
    }

    fn link() -> Values {
        values(&[
            ("NewX_AVM-DE_DownstreamCurrentUtilization", "7,8"),
            ("NewX_AVM-DE_UpstreamCurrentUtilization", "5"),
            ("NewLayer1DownstreamMaxBitRate", "100000000"),
            ("NewLayer1UpstreamMaxBitRate", "40000000"),
        ])
    }

    #[test]
    fn reads_the_online_monitor_first() {
        let totals = [
            values(&[("NewTotalBytesReceived", "5000000000")]),
            values(&[("NewTotalPacketsSent", "12")]),
        ];
        let traffic = from_values(&monitor(), &link(), &totals);
        assert!(traffic.supported);
        let down = &traffic.downstream;
        assert_eq!(down.current_bps, Some(8000));
        assert_eq!(down.peak_bps, Some(32_000));
        assert_eq!(down.max_bps, Some(16_000_000));
        assert_eq!(down.utilization_percent, Some(0.1));
        assert_eq!(down.multicast_bps, Some(2000));
        assert_eq!(down.total_bytes, Some(5_000_000_000));
        assert_eq!(down.priorities, None);
        let up = &traffic.upstream;
        assert_eq!(up.current_bps, Some(800));
        assert_eq!(up.max_bps, Some(0));
        assert_eq!(up.utilization_percent, None);
        assert_eq!(up.total_packets, Some(12));
        assert_eq!(up.total_bytes, None);
        let priorities = up.priorities.unwrap();
        assert_eq!(priorities.realtime_bps, Some(80));
        assert_eq!(priorities.default_bps, Some(720));
        assert_eq!(priorities.high_bps, None);
    }

    #[test]
    fn falls_back_to_the_link_properties() {
        let traffic = from_values(&Values::default(), &link(), &[]);
        let down = &traffic.downstream;
        assert_eq!(down.current_bps, Some(56));
        assert_eq!(down.peak_bps, Some(64));
        assert_eq!(down.max_bps, Some(100_000_000));
        assert_eq!(down.multicast_bps, None);
        assert_eq!(traffic.upstream.priorities, None);
        let json = serde_json::to_value(&traffic).unwrap();
        // Without the split, its fields are left out.
        assert!(json["downstream"].get("multicast_bps").is_none(), "{json}");
        assert!(json["upstream"].get("priorities").is_none(), "{json}");
        assert!(json["upstream"]["total_bytes"].is_null(), "{json}");
        let nothing = from_values(&Values::default(), &Values::default(), &[]);
        assert_eq!(nothing.downstream, Flow::default());
    }

    #[test]
    fn averages_the_counters_between_reads() {
        let earlier = Traffic {
            supported: true,
            downstream: Flow {
                total_bytes: Some(u64::from(u32::MAX) - 999),
                total_packets: Some(10),
                ..Flow::default()
            },
            upstream: Flow {
                total_bytes: Some(1 << 40),
                ..Flow::default()
            },
        };
        let now = Traffic {
            supported: true,
            downstream: Flow {
                current_bps: Some(1000),
                max_bps: Some(100_000),
                total_bytes: Some(1000),
                total_packets: Some(30),
                ..Flow::default()
            },
            upstream: Flow {
                current_bps: Some(500),
                max_bps: Some(1000),
                total_bytes: Some(5),
                ..Flow::default()
            },
        };
        let sample = now.since(&earlier, 2.0);
        assert_eq!(sample.seconds, 2.0);
        // The 32-bit counter wrapped around: 2000 bytes in 2 seconds.
        let down = &sample.downstream;
        assert_eq!(down.bytes, Some(2000));
        assert_eq!(down.average_bps, Some(8000.0));
        assert_eq!(down.packets_per_second, Some(10.0));
        assert_eq!(down.utilization_percent, Some(8.0));
        assert_eq!(down.current_bps, Some(1000));
        // The 64-bit counter fell: no average, the latest rate instead.
        let up = &sample.upstream;
        assert_eq!(up.bytes, None);
        assert_eq!(up.average_bps, None);
        assert_eq!(up.packets_per_second, None);
        assert_eq!(up.utilization_percent, Some(50.0));
    }
}
