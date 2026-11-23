//! `fritz traffic`: the traffic of the internet connection, once or with
//! `--watch` as one row of averages per interval.

use std::io::Write;

use anyhow::Context as _;
use fritz_core::read::Reader;
use fritz_core::traffic::{self, Flow, Priorities, Rate, Sample, Traffic, u64_to_f64};

use crate::context::Context;
use crate::interfaces::{bit_rate, bytes};
use crate::output::{Format, show};
use crate::status::{self, pairs, table};
use crate::watch;

/// Prints the traffic, or with `--watch` the averages of every interval.
/// On a device without the traffic, prints why and stops.
pub(crate) async fn traffic(
    context: &Context,
    options: watch::Options,
    format: Format,
    out: &mut impl Write,
) -> anyhow::Result<()> {
    let mut reader = status::reader(context).await?;
    let traffic = read(&mut reader).await?;
    if !options.watch || !traffic.supported {
        return show(&traffic, format, out, snapshot_text);
    }
    // The first read of the watch is the one above.
    let mut first = Some(traffic);
    let read = async || match first.take() {
        Some(traffic) => Ok(traffic),
        None => read(&mut reader).await,
    };
    watch::run(options, read, format, out).await
}

impl watch::Reading for Traffic {
    type Sample = Sample;

    const BLANK_LINE: bool = false;

    fn since(&self, before: &Self, seconds: f64) -> Sample {
        Traffic::since(self, before, seconds)
    }

    fn text(sample: &Sample, out: &mut impl Write) -> anyhow::Result<()> {
        sample_text(sample, out)
    }
}

async fn read(reader: &mut Reader) -> anyhow::Result<Traffic> {
    traffic::traffic(reader)
        .await
        .context("cannot read the traffic")
}

/// A rate in bit/s, or nothing.
fn rate(bps: Option<u64>) -> String {
    bps.map_or_else(String::new, |b| bit_rate(u64_to_f64(b)))
}

/// A share of the highest rate, like `41.2% of 300.0 Mbit/s`.
fn used(percent: Option<f64>, max: Option<u64>) -> String {
    match (percent, max) {
        (Some(percent), Some(_)) => format!("{percent:.1}% of {}", rate(max)),
        (None, Some(_)) => format!("- of {}", rate(max)),
        (_, None) => String::new(),
    }
}

/// What a device without the traffic says.
const UNSUPPORTED: &str = "The device does not report the traffic of an internet connection.";

fn snapshot_text(traffic: &Traffic, out: &mut impl Write) -> anyhow::Result<()> {
    if !traffic.supported {
        writeln!(out, "{UNSUPPORTED}")?;
        return Ok(());
    }
    let row = |name: &str, f: &Flow| {
        vec![
            name.to_owned(),
            rate(f.current_bps),
            used(f.utilization_percent, f.max_bps),
            rate(f.peak_bps),
            f.total_bytes.map_or_else(String::new, bytes),
            f.total_packets.map_or_else(String::new, |p| p.to_string()),
        ]
    };
    let rows = [
        row("DOWN", &traffic.downstream),
        row("UP", &traffic.upstream),
    ];
    let headers = ["", "NOW", "USED", "PEAK", "TOTAL", "PACKETS"];
    table(&headers, rows.into_iter(), out)?;
    let split = [
        ("Upstream", traffic.upstream.priorities.map(priorities)),
        (
            "Multicast",
            traffic.downstream.multicast_bps.map(|m| rate(Some(m))),
        ),
    ];
    if split.iter().any(|(_, value)| value.is_some()) {
        writeln!(out)?;
        pairs(&split, out)?;
    }
    Ok(())
}

/// The upstream by priority class, like `real time 8.0 kbit/s, default …`.
fn priorities(p: Priorities) -> String {
    [
        ("real time", p.realtime_bps),
        ("high", p.high_bps),
        ("default", p.default_bps),
        ("low", p.low_bps),
    ]
    .into_iter()
    .filter_map(|(name, bps)| bps.map(|b| format!("{name} {}", rate(Some(b)))))
    .collect::<Vec<_>>()
    .join(", ")
}

/// One row per sample: the averages, else the latest rates, and how much
/// of the line they use, like `↓ 124.3 Mbit/s 41.4%  ↑ 12.8 Mbit/s 32.0%`.
fn sample_text(sample: &Sample, out: &mut impl Write) -> anyhow::Result<()> {
    let direction = |r: &Rate| {
        let bps = r.average_bps.or(r.current_bps.map(u64_to_f64));
        let bps = bps.map_or_else(|| "-".to_owned(), bit_rate);
        let percent = r
            .utilization_percent
            .map_or_else(String::new, |p| format!("{p:.1}%"));
        format!("{bps:>12} {percent:>6}")
    };
    let (down, up) = (direction(&sample.downstream), direction(&sample.upstream));
    let row = format!("↓ {down}  ↑ {up}");
    writeln!(out, "{}", row.trim_end())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::status::tests::text;

    #[test]
    fn prints_the_traffic_as_text() {
        let traffic = Traffic {
            supported: true,
            downstream: Flow {
                current_bps: Some(124_300_000),
                max_bps: Some(300_000_000),
                utilization_percent: Some(41.4),
                peak_bps: Some(200_000_000),
                total_bytes: Some(18_200_000_000),
                total_packets: Some(12_345),
                multicast_bps: Some(0),
                priorities: None,
            },
            upstream: Flow {
                current_bps: Some(12_800),
                max_bps: Some(40_000_000),
                priorities: Some(Priorities {
                    realtime_bps: Some(8000),
                    default_bps: Some(4800),
                    ..Priorities::default()
                }),
                ..Flow::default()
            },
        };
        let output = text(|out| snapshot_text(&traffic, out));
        assert_eq!(
            output,
            "      NOW           USED                   PEAK          TOTAL    PACKETS\n\
             DOWN  124.3 Mbit/s  41.4% of 300.0 Mbit/s  200.0 Mbit/s  18.2 GB  12345\n\
             UP    12.8 kbit/s   - of 40.0 Mbit/s\n\
             \n\
             Upstream   real time 8.0 kbit/s, default 4.8 kbit/s\n\
             Multicast  0 bit/s\n"
        );
        let bare = Traffic {
            supported: true,
            ..Traffic::default()
        };
        let output = text(|out| snapshot_text(&bare, out));
        assert_eq!(output, "      NOW  USED  PEAK  TOTAL  PACKETS\nDOWN\nUP\n");
        let output = text(|out| snapshot_text(&Traffic::default(), out));
        assert_eq!(
            output,
            "The device does not report the traffic of an internet connection.\n"
        );
    }

    #[test]
    fn prints_a_row_per_sample() {
        let sample = Sample {
            seconds: 2.0,
            downstream: Rate {
                average_bps: Some(124_300_000.0),
                current_bps: Some(1),
                utilization_percent: Some(41.43),
                ..Rate::default()
            },
            upstream: Rate {
                current_bps: Some(12_800),
                ..Rate::default()
            },
        };
        let output = text(|out| sample_text(&sample, out));
        assert_eq!(output, "↓ 124.3 Mbit/s  41.4%  ↑  12.8 kbit/s\n");
        let output = text(|out| sample_text(&Sample::default(), out));
        assert_eq!(output, "↓            -         ↑            -\n");
    }
}
