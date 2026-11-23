//! The load of the device: CPU load, CPU temperature and memory, read from
//! the page `ecoStat` of the web interface, which shows the energy monitor
//! and the system load, with a [`Session`].
//!
//! Experimental: the page is not documented, and its fields may change
//! with FRITZ!OS. On FRITZ!OS 8.25, it holds three charts, oldest value
//! first: `cpuutil` and `cputemp` with a value every 6 minutes, and
//! `ramusage` with a value every hour, as the shares of memory that is
//! used, used as cache and free. Labels name the hour of the day where a
//! new hour begins.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::session::{Error, Session};
use crate::status::Right;

/// Where the pages of the web interface are.
pub const DATA: &str = "/data.lua";

/// The page with the charts.
pub const PAGE: &str = "ecoStat";

/// The right to see the settings of the device, which the page needs.
const RIGHT: &str = "BoxAdmin";

/// The load of the device now, and over the time the device keeps.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct System {
    /// The latest CPU load in percent.
    pub cpu_load_percent: Option<f64>,
    /// The latest CPU temperature in °C.
    pub cpu_temperature_celsius: Option<f64>,
    /// The latest memory use.
    pub memory: Memory,
    /// The values the device keeps.
    pub history: History,
}

/// The shares of the memory in percent.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Memory {
    pub used_percent: Option<f64>,
    pub cache_percent: Option<f64>,
    pub free_percent: Option<f64>,
}

/// The lowest, average and highest values the device keeps.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct History {
    /// How many hours the values cover.
    pub hours: Option<usize>,
    pub cpu_load_percent: Option<Range>,
    pub cpu_temperature_celsius: Option<Range>,
    pub memory_used_percent: Option<Range>,
}

/// The lowest, average and highest of several values.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Range {
    pub min: f64,
    /// Rounded to one decimal.
    pub avg: f64,
    pub max: f64,
}

/// The load of the device, or why it is missing.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Section {
    /// The load, or `null` if the device does not show it.
    pub system: Option<System>,
    /// Why the load is missing.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_note: Option<String>,
}

impl Section {
    /// A section without values, because of `note`.
    pub fn missing(note: String) -> Self {
        Self {
            system: None,
            system_note: Some(note),
        }
    }
}

/// Reads the load. Never fails: if the page cannot be read, the section
/// tells why, with a hint if the user lacks the right to see the settings.
pub async fn section(session: &Session) -> Section {
    match system(session).await {
        Ok(system) => Section {
            system: Some(system),
            system_note: None,
        },
        Err(error) => Section::missing(note(&error, session.rights())),
    }
}

/// Why the load could not be read, with a hint if the user lacks the
/// right to see the settings.
fn note(error: &Error, rights: &[Right]) -> String {
    let hint = if rights.iter().any(|r| r.area == RIGHT) {
        ""
    } else {
        "; the user may need the right to see the settings of the device"
    };
    format!("the device shows no system load: {error}{hint}")
}

/// Reads the load.
///
/// # Errors
///
/// The errors of the request, and [`Error::Invalid`] if the answer holds
/// none of the charts.
pub async fn system(session: &Session) -> Result<System, Error> {
    let form = [
        ("xhr", "1"),
        ("lang", "en"),
        ("page", PAGE),
        ("xhrId", "all"),
        ("useajax", "1"),
        ("no_sidrenew", ""),
    ];
    parse(&session.post(DATA, &form).await?)
}

/// The load in the page `ecoStat`.
///
/// # Errors
///
/// [`Error::Invalid`] if `json` is not the page or holds none of the
/// charts.
pub fn parse(json: &str) -> Result<System, Error> {
    let invalid = |reason: String| Error::Invalid {
        what: "the system load page",
        reason,
    };
    let page: Page = serde_json::from_str(json).map_err(|e| invalid(e.to_string()))?;
    let Data {
        cpuutil,
        cputemp,
        ramusage,
    } = page.data;
    if [&cpuutil, &cputemp, &ramusage].iter().all(|c| c.is_none()) {
        return Err(invalid("no CPU or memory values".to_owned()));
    }
    let series = |chart: &Option<Chart>, index: usize| {
        chart
            .as_ref()
            .and_then(|c| c.series.get(index))
            .map_or_else(Vec::new, |values| {
                values.iter().filter_map(number).collect::<Vec<f64>>()
            })
    };
    let (load, temperature) = (series(&cpuutil, 0), series(&cputemp, 0));
    let (used, cache, free) = (
        series(&ramusage, 0),
        series(&ramusage, 1),
        series(&ramusage, 2),
    );
    let hours = [&cpuutil, &cputemp, &ramusage]
        .iter()
        .filter_map(|c| c.as_ref().map(Chart::hours))
        .max()
        .filter(|h| *h > 0);
    Ok(System {
        cpu_load_percent: load.last().copied(),
        cpu_temperature_celsius: temperature.last().copied(),
        memory: Memory {
            used_percent: used.last().copied(),
            cache_percent: cache.last().copied(),
            free_percent: free.last().copied(),
        },
        history: History {
            hours,
            cpu_load_percent: range(&load),
            cpu_temperature_celsius: range(&temperature),
            memory_used_percent: range(&used),
        },
    })
}

/// A number, also as text like `"68"`.
fn number(value: &Value) -> Option<f64> {
    match value {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
    .filter(|n: &f64| n.is_finite())
}

#[expect(clippy::cast_precision_loss, reason = "a few hundred values")]
fn range(values: &[f64]) -> Option<Range> {
    let first = *values.first()?;
    let (min, max) = values
        .iter()
        .fold((first, first), |(min, max), v| (min.min(*v), max.max(*v)));
    let avg = values.iter().sum::<f64>() / values.len() as f64;
    Some(Range {
        min,
        avg: (avg * 10.0).round() / 10.0,
        max,
    })
}

#[derive(Debug, Deserialize)]
struct Page {
    data: Data,
}

#[derive(Debug, Deserialize)]
struct Data {
    cpuutil: Option<Chart>,
    cputemp: Option<Chart>,
    ramusage: Option<Chart>,
}

#[derive(Debug, Deserialize)]
struct Chart {
    #[serde(default)]
    labels: Vec<Value>,
    #[serde(default)]
    series: Vec<Vec<Value>>,
}

impl Chart {
    /// The number of hours the chart covers: its labels, which name the
    /// hours, while the others are `false`.
    fn hours(&self) -> usize {
        self.labels.iter().filter(|l| number(l).is_some()).count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_latest_values_and_the_history() {
        let system = parse(
            r#"{"data": {
                "cpuutil": {"labels": [false, 20, false, 21], "series": [["5", "9", "", "7"], ["0", "0", "0", "0"]]},
                "cputemp": {"labels": [false, 20, false, 21], "series": [["60", "64", "62", "66"]]},
                "ramusage": {"labels": [20, 21], "series": [[48.9, 49], [11.1, 11], [40, 40]]}
            }, "sid": "0123456789abcdef"}"#,
        )
        .unwrap();
        assert_eq!(
            system,
            System {
                cpu_load_percent: Some(7.0),
                cpu_temperature_celsius: Some(66.0),
                memory: Memory {
                    used_percent: Some(49.0),
                    cache_percent: Some(11.0),
                    free_percent: Some(40.0),
                },
                history: History {
                    hours: Some(2),
                    cpu_load_percent: Some(Range {
                        min: 5.0,
                        avg: 7.0,
                        max: 9.0
                    }),
                    cpu_temperature_celsius: Some(Range {
                        min: 60.0,
                        avg: 63.0,
                        max: 66.0
                    }),
                    memory_used_percent: Some(Range {
                        min: 48.9,
                        avg: 49.0,
                        max: 49.0
                    }),
                },
            }
        );
    }

    #[test]
    fn leaves_out_what_the_device_does_not_report() {
        let system = parse(r#"{"data": {"cputemp": {"series": []}}}"#).unwrap();
        assert_eq!(system, System::default());
    }

    #[test]
    fn rejects_pages_without_charts() {
        for json in ["<html>", r#"{"data": {}}"#, r#"{"pid": "ecoStat"}"#] {
            let error = parse(json).unwrap_err().to_string();
            assert!(
                error.starts_with("invalid answer from the system load page: "),
                "{json}: {error}"
            );
        }
    }

    #[test]
    fn hints_at_the_right() {
        let error = Error::Invalid {
            what: "the system load page",
            reason: "no CPU or memory values".to_owned(),
        };
        assert_eq!(
            note(&error, &[]),
            "the device shows no system load: invalid answer from the system load page: \
             no CPU or memory values; the user may need the right to see the settings of the device"
        );
        let admin = Right {
            area: RIGHT.to_owned(),
            access: "read".to_owned(),
        };
        assert!(!note(&error, &[admin]).contains("right"));
    }
}
