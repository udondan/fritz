//! `fritz smarthome <device> --history`: the series a Smart Home device
//! keeps, in short as text and whole as JSON or YAML.

use std::io::Write;

use anyhow::Context as _;
use clap::Args;
use clap::builder::PossibleValuesParser;
use fritz_core::session::Session;
use fritz_core::smarthome::history::{self, History, Range, Series};

use super::table;
use crate::context::Context;
use crate::output::{Format, show};

/// The arguments of `fritz smarthome`.
#[derive(Debug, Args)]
pub(crate) struct SmarthomeArgs {
    /// The device, by name or AIN, whose history `--history` shows.
    #[arg(requires = "history")]
    device: Option<String>,
    /// The power, voltage, energy, temperature and humidity the device
    /// keeps: as text in short, as JSON or YAML with all values, the newest
    /// first, their interval in seconds and the time of the newest value.
    #[arg(long, requires = "device")]
    history: bool,
    /// Only the series over the last hours, the energy per day, or the
    /// energy per month.
    #[arg(long, requires = "history", value_parser = PossibleValuesParser::new(["hours", "days", "months"]))]
    range: Option<String>,
}

impl SmarthomeArgs {
    /// The device and the range of the history, if it is asked for.
    pub(crate) fn history(&self) -> Option<(&str, Option<&str>)> {
        self.device
            .as_deref()
            .map(|device| (device, self.range.as_deref()))
    }
}

/// The range named `name`, like `days`.
///
/// # Errors
///
/// If no range has the name.
pub(crate) fn range(name: Option<&str>) -> anyhow::Result<Option<Range>> {
    name.map(str::parse).transpose().map_err(anyhow::Error::msg)
}

/// Reads the history of `device` in `session`, and logs out.
///
/// # Errors
///
/// An unknown device, and the errors of reading.
pub(crate) async fn read(
    session: Session,
    device: &str,
    range: Option<Range>,
) -> anyhow::Result<History> {
    let found = history::history(&session, device, range).await;
    // Ends the session even if reading failed.
    let logout = session.logout().await;
    let found = found.context("cannot read the Smart Home history")?;
    logout.context("cannot log out of the web interface")?;
    found.with_context(|| format!("no Smart Home device has the name or AIN {device:?}"))
}

/// Prints the history of `device`.
pub(crate) async fn history(
    context: &Context,
    device: &str,
    range: Option<&str>,
    format: Format,
    out: &mut impl Write,
) -> anyhow::Result<()> {
    let range = self::range(range)?;
    let session = context
        .session()
        .await
        .context("cannot log in to the web interface")?;
    let history = read(session, device, range).await?;
    show(&history, format, out, history_table)
}

/// One line per series, with the newest value, the spread and a trend.
fn history_table(history: &History, out: &mut impl Write) -> anyhow::Result<()> {
    let (name, ain) = (super::clean(&history.name), super::clean(&history.ain));
    writeln!(out, "{name} ({ain})")?;
    if history.series.is_empty() {
        writeln!(out, "No history.")?;
        return Ok(());
    }
    let rows = history.series.iter().map(|series| {
        let value = |v: f64| format!("{v:.1} {}", series.unit);
        let spread = series.summary.spread;
        vec![
            series.kind.to_owned(),
            range_name(series.range).to_owned(),
            interval(series.interval),
            format!(
                "{}/{}",
                series.summary.count,
                series.summary.count + series.summary.gaps
            ),
            series
                .values
                .first()
                .copied()
                .flatten()
                .map_or_default(value),
            series.summary.sum.map_or_default(value),
            spread.map_or_default(|s| value(s.min)),
            spread.map_or_default(|s| value(s.avg)),
            spread.map_or_default(|s| value(s.max)),
            sparkline(series),
        ]
    });
    let headers = [
        "SERIES", "RANGE", "INTERVAL", "VALUES", "LATEST", "SUM", "MIN", "AVG", "MAX", "TREND",
    ];
    table(&headers, rows, out)
}

fn range_name(range: Range) -> &'static str {
    match range {
        Range::Hours => "hours",
        Range::Days => "days",
        Range::Months => "months",
    }
}

/// Seconds in the largest unit that divides them, like `15 min`.
fn interval(seconds: u64) -> String {
    let (size, unit) = [(86_400, "d"), (3600, "h"), (60, "min")]
        .into_iter()
        .find(|(size, _)| seconds >= *size && seconds.is_multiple_of(*size))
        .unwrap_or((1, "s"));
    format!("{} {unit}", seconds / size)
}

/// The widest trend, in characters.
const TREND: usize = 24;

/// The values from the oldest to the newest as bars, the average of
/// neighbors in one bar if there are many, and a space for gaps.
fn sparkline(series: &Series) -> String {
    const BARS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    let values: Vec<Option<f64>> = series.values.iter().rev().copied().collect();
    let per_bar = values.len().div_ceil(TREND).max(1);
    let bars: Vec<Option<f64>> = values
        .chunks(per_bar)
        .map(|chunk| {
            let present: Vec<f64> = chunk.iter().flatten().copied().collect();
            #[expect(clippy::cast_precision_loss, reason = "few values")]
            let count = present.len() as f64;
            (!present.is_empty()).then(|| present.iter().sum::<f64>() / count)
        })
        .collect();
    let Some(spread) = series.summary.spread else {
        return String::new();
    };
    let span = spread.max - spread.min;
    bars.iter()
        .map(|bar| {
            bar.map_or(' ', |v| {
                let share = if span > 0.0 {
                    (v - spread.min) / span
                } else {
                    0.0
                };
                #[expect(
                    clippy::cast_possible_truncation,
                    clippy::cast_sign_loss,
                    reason = "share is between 0 and 1"
                )]
                let index = (share * 7.0).round() as usize;
                BARS[index.min(7)]
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::status::tests::text;
    use fritz_core::smarthome::history::parse;

    fn socket() -> History {
        History {
            ain: "1".to_owned(),
            name: "Plug\u{1b}".to_owned(),
            series: parse(
                "<devicestats><power><stats grid=\"10\">2345,2300,-,0</stats></power>\
                 <energy><stats grid=\"86400\">100,95,-,110</stats></energy>\
                 <humidity><stats grid=\"900\">-</stats></humidity>\
                 <voltage><stats grid=\"60\">230000,230000</stats></voltage></devicestats>",
            )
            .unwrap(),
        }
    }

    #[test]
    fn prints_the_history_as_text() {
        let output = text(|out| history_table(&socket(), out));
        assert_eq!(
            output,
            "Plug\u{FFFD} (1)\n\
             SERIES    RANGE  INTERVAL  VALUES  LATEST    SUM       MIN      AVG       MAX       TREND\n\
             power     hours  10 s      3/4     23.4 W              0.0 W    15.5 W    23.4 W    ▁ ██\n\
             voltage   hours  1 min     2/2     230.0 V             230.0 V  230.0 V   230.0 V   ▁▁\n\
             energy    days   1 d       3/4     100.0 Wh  305.0 Wh  95.0 Wh  101.7 Wh  110.0 Wh  █ ▁▃\n\
             humidity  hours  15 min    0/1\n"
        );
        let empty = History {
            series: Vec::new(),
            ..socket()
        };
        let output = text(|out| history_table(&empty, out));
        assert_eq!(output, "Plug\u{FFFD} (1)\nNo history.\n");
    }

    #[test]
    fn names_intervals_and_ranges() {
        assert_eq!(interval(2_678_400), "31 d");
        assert_eq!(interval(7200), "2 h");
        assert_eq!(interval(90), "90 s");
        assert_eq!(range_name(Range::Months), "months");
        assert_eq!(range(Some("hours")).unwrap(), Some(Range::Hours));
        assert_eq!(range(None).unwrap(), None);
        assert!(range(Some("weeks")).is_err());
    }

    #[test]
    fn draws_long_series_in_few_bars() {
        let values = (0..360)
            .map(|v| v.to_string())
            .collect::<Vec<_>>()
            .join(",");
        let xml = format!(
            "<devicestats><power><stats grid=\"10\">{values}</stats></power></devicestats>"
        );
        let series = &parse(&xml).unwrap()[0];
        let line = sparkline(series);
        assert_eq!(line.chars().count(), TREND);
        assert!(line.starts_with('█') && line.ends_with('▁'), "{line}");
    }
}
