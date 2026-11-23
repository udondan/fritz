//! The history of a Smart Home device: power, voltage, energy, temperature
//! and humidity over the last hours, days or months, as the device keeps
//! them.
//!
//! The statistics come from the command `getbasicdevicestats` of the Smart
//! Home HTTP interface, `/webservices/homeautoswitch.lua`, which only reads.
//! It is posted with the session of the web interface, so the session ID
//! stays out of the URL. Each series has a grid, the seconds between two
//! values, and the values with the newest first; `-` marks a gap. Newer
//! firmware also gives the time of the newest value, from which
//! [`Series::time`] rebuilds the time of each value.

use std::str::FromStr;

use serde::Serialize;

use super::Device;
use crate::session::{Error, Session};
use crate::system::{self, Range as Spread};

/// The Smart Home HTTP interface.
pub const HOMEAUTO: &str = "/webservices/homeautoswitch.lua";

/// The command that reads the statistics of a device.
const STATS: &str = "getbasicdevicestats";

const DAY: u64 = 86_400;

/// How far a series reaches back: hours for values seconds or minutes
/// apart, days for values a day apart, and months for values a month apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Range {
    Hours,
    Days,
    Months,
}

impl Range {
    /// The range of a series with values `interval` seconds apart.
    fn of(interval: u64) -> Self {
        match interval {
            i if i < DAY => Self::Hours,
            i if i < 28 * DAY => Self::Days,
            _ => Self::Months,
        }
    }
}

impl FromStr for Range {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        match text {
            "hours" => Ok(Self::Hours),
            "days" => Ok(Self::Days),
            "months" => Ok(Self::Months),
            other => Err(format!(
                "unknown range {other:?}, expected hours, days or months"
            )),
        }
    }
}

/// One series of values.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Series {
    /// `power`, `voltage`, `energy`, `temperature` or `humidity`.
    pub kind: &'static str,
    /// `W`, `V`, `Wh`, `°C` or `%`.
    pub unit: &'static str,
    pub range: Range,
    /// The seconds between two values.
    pub interval: u64,
    /// The time of the newest value, in seconds since 1970, if the device
    /// gives it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub newest: Option<u64>,
    pub summary: Summary,
    /// The values, the newest first. `null` marks a gap.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub values: Vec<Option<f64>>,
}

impl Series {
    /// The time of the value at `index`, in seconds since 1970: the time of
    /// the newest value less `index` intervals. `None` without the time of
    /// the newest value. For months, the device counts 31 days each, so the
    /// times are near the start of the month, not on it.
    pub fn time(&self, index: usize) -> Option<u64> {
        let back = u64::try_from(index).ok()?.checked_mul(self.interval)?;
        self.newest?.checked_sub(back)
    }
}

/// The values of a series in short.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Summary {
    /// The values without the gaps.
    pub count: usize,
    pub gaps: usize,
    /// The sum, only for energy, where it is the energy of the whole
    /// series.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sum: Option<f64>,
    #[serde(flatten)]
    pub spread: Option<Spread>,
}

/// The history of a device.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct History {
    pub ain: String,
    pub name: String,
    /// Empty if the device keeps no history.
    pub series: Vec<Series>,
}

/// The device named `name`, or with the AIN `name`, ignoring spaces. A name
/// that matches exactly wins over one that only matches ignoring case.
pub fn find<'a>(devices: &'a [Device], name: &str) -> Option<&'a Device> {
    let compact = |ain: &str| ain.chars().filter(|c| *c != ' ').collect::<String>();
    devices
        .iter()
        .find(|d| d.name == name || compact(&d.ain) == compact(name))
        .or_else(|| devices.iter().find(|d| d.name.eq_ignore_ascii_case(name)))
}

/// Reads the history of the device named `name`, or with the AIN `name`,
/// only the series of `range` if given. `None` if no device has the name.
///
/// # Errors
///
/// The errors of the requests, and [`Error::Invalid`] for answers that are
/// not an overview or statistics.
pub async fn history(
    session: &Session,
    name: &str,
    range: Option<Range>,
) -> Result<Option<History>, Error> {
    let devices = super::devices(session).await?;
    let Some(device) = find(&devices, name) else {
        return Ok(None);
    };
    let form = [("switchcmd", STATS), ("ain", device.ain.as_str())];
    let mut series = parse(&session.post(HOMEAUTO, &form).await?)?;
    series.retain(|s| range.is_none_or(|r| s.range == r));
    Ok(Some(History {
        ain: device.ain.clone(),
        name: device.name.clone(),
        series,
    }))
}

/// The kinds of series, their units, and the factor from the values of the
/// device to the unit.
const KINDS: [(&str, &str, f64); 5] = [
    ("power", "W", 0.01),
    ("voltage", "V", 0.001),
    ("energy", "Wh", 1.0),
    ("temperature", "°C", 0.1),
    ("humidity", "%", 1.0),
];

/// The series of an answer of `getbasicdevicestats`, in the order of
/// [`KINDS`], and the shortest interval first. Series of unknown kinds and
/// without a grid are left out.
///
/// # Errors
///
/// [`Error::Invalid`] if `xml` is not such an answer.
pub fn parse(xml: &str) -> Result<Vec<Series>, Error> {
    let invalid = |reason: String| Error::Invalid {
        what: "the Smart Home HTTP interface",
        reason,
    };
    let document = roxmltree::Document::parse(xml).map_err(|e| invalid(e.to_string()))?;
    let root = document.root_element();
    if !root.has_tag_name("devicestats") {
        return Err(invalid(format!("unexpected <{}>", root.tag_name().name())));
    }
    let mut series = Vec::new();
    for (kind, unit, factor) in KINDS {
        let stats = root
            .children()
            .filter(|n| n.has_tag_name(kind))
            .flat_map(|n| n.children().filter(|s| s.has_tag_name("stats")));
        let mut found: Vec<Series> = stats
            .filter_map(|stats| {
                let number = |name| stats.attribute(name)?.trim().parse::<u64>().ok();
                let interval = number("grid").filter(|g| *g > 0)?;
                let values: Vec<Option<f64>> = stats
                    .text()
                    .unwrap_or_default()
                    .split(',')
                    .map(str::trim)
                    .filter(|v| !v.is_empty())
                    .map(|v| v.parse::<i64>().ok().map(|n| scale(n, factor)))
                    .collect();
                Some(Series {
                    kind,
                    unit,
                    range: Range::of(interval),
                    interval,
                    newest: number("datatime"),
                    summary: summary(kind, &values),
                    values,
                })
            })
            .collect();
        found.sort_by_key(|s| s.interval);
        series.extend(found);
    }
    Ok(series)
}

/// A value of the device in the unit, rounded to three decimals against
/// the noise of the factor.
#[expect(clippy::cast_precision_loss, reason = "values are far below 2^52")]
fn scale(value: i64, factor: f64) -> f64 {
    (value as f64 * factor * 1000.0).round() / 1000.0
}

fn summary(kind: &str, values: &[Option<f64>]) -> Summary {
    let present: Vec<f64> = values.iter().flatten().copied().collect();
    Summary {
        count: present.len(),
        gaps: values.len() - present.len(),
        sum: (kind == "energy" && !present.is_empty()).then(|| present.iter().sum()),
        spread: system::range(&present),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SOCKET: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../fritz-sim/fixtures/fritzbox-7690/8.25/homeauto/116300000001.xml"
    ));

    #[test]
    fn reads_the_series_of_a_socket() {
        let series = parse(SOCKET).unwrap();
        let kinds: Vec<_> = series.iter().map(|s| (s.kind, s.range)).collect();
        assert_eq!(
            kinds,
            [
                ("power", Range::Hours),
                ("voltage", Range::Hours),
                ("energy", Range::Days),
                ("energy", Range::Months),
                ("temperature", Range::Hours),
            ]
        );
        let power = &series[0];
        assert_eq!(power.unit, "W");
        assert_eq!(power.interval, 10);
        assert_eq!(power.values, [Some(23.45), Some(23.0), None, Some(0.0)]);
        assert_eq!(power.summary.count, 3);
        assert_eq!(power.summary.gaps, 1);
        assert_eq!(power.summary.sum, None);
        let spread = power.summary.spread.unwrap();
        assert_eq!((spread.min, spread.avg, spread.max), (0.0, 15.5, 23.45));
        assert_eq!(series[1].values[0], Some(228.055));
        assert_eq!(series[2].summary.sum, Some(305.0));
        assert_eq!(series[4].values[0], Some(21.5));
    }

    #[test]
    fn rebuilds_the_times() {
        let series = parse(SOCKET).unwrap();
        let power = &series[0];
        assert_eq!(power.newest, Some(1_767_225_600));
        assert_eq!(power.time(0), Some(1_767_225_600));
        assert_eq!(power.time(3), Some(1_767_225_570));
        assert_eq!(series[3].time(1), Some(1_767_225_600 - 2_678_400));
        // Before 1970, or without the time of the newest value: unknown.
        assert_eq!(power.time(usize::MAX), None);
        let without = Series {
            newest: None,
            ..power.clone()
        };
        assert_eq!(without.time(0), None);
    }

    #[test]
    fn prints_summaries_and_values_as_json() {
        let series = parse(SOCKET).unwrap();
        let mut energy = series[2].clone();
        assert_eq!(
            serde_json::to_value(&energy).unwrap()["values"],
            serde_json::json!([100.0, 95.0, null, 110.0])
        );
        energy.values.clear();
        assert_eq!(
            serde_json::to_value(&energy).unwrap(),
            serde_json::json!({
                "kind": "energy",
                "unit": "Wh",
                "range": "days",
                "interval": 86400,
                "newest": 1_767_225_600,
                "summary": {"count": 3, "gaps": 1, "sum": 305.0, "min": 95.0, "avg": 101.7, "max": 110.0},
            })
        );
    }

    #[test]
    fn tolerates_odd_series() {
        let series = parse(
            "<devicestats><humidity><stats grid=\"900\">-, x,,</stats>\
             <stats grid=\"0\">1</stats><stats>1</stats></humidity>\
             <noise><stats grid=\"10\">1</stats></noise>\
             <temperature><stats grid=\"86400\"/></temperature></devicestats>",
        )
        .unwrap();
        assert_eq!(series.len(), 2);
        assert_eq!(series[0].kind, "temperature");
        assert_eq!(series[0].values, []);
        assert_eq!(series[1].values, [None, None]);
        assert_eq!(series[1].summary.spread, None);
        assert_eq!(series[1].summary.gaps, 2);
        assert_eq!(parse("<devicestats/>").unwrap(), []);
    }

    #[test]
    fn rejects_other_answers() {
        for (xml, reason) in [("<other/>", "unexpected <other>"), ("inval", "")] {
            let error = parse(xml).unwrap_err().to_string();
            assert!(
                error.starts_with("invalid answer from the Smart Home HTTP interface: ")
                    && error.contains(reason),
                "{error}"
            );
        }
    }

    #[test]
    fn knows_the_ranges() {
        assert_eq!(Range::of(10), Range::Hours);
        assert_eq!(Range::of(900), Range::Hours);
        assert_eq!(Range::of(DAY), Range::Days);
        assert_eq!(Range::of(31 * DAY), Range::Months);
        assert_eq!("days".parse(), Ok(Range::Days));
        assert_eq!("hours".parse(), Ok(Range::Hours));
        assert_eq!("months".parse(), Ok(Range::Months));
        assert!("weeks".parse::<Range>().unwrap_err().contains("weeks"));
    }

    #[test]
    fn finds_devices_by_name_or_ain() {
        let device = |name: &str, ain: &str| Device {
            ain: ain.to_owned(),
            name: name.to_owned(),
            product: None,
            category: None,
            firmware: None,
            connected: true,
            battery: None,
            battery_low: None,
            units: Vec::new(),
        };
        let devices = [device("Lamp", "1 2"), device("lamp", "3")];
        assert_eq!(find(&devices, "lamp").unwrap().ain, "3");
        assert_eq!(find(&devices, "LAMP").unwrap().ain, "1 2");
        assert_eq!(find(&devices, "12").unwrap().name, "Lamp");
        assert!(find(&devices, "other").is_none());
    }
}
