//! Filters of the calls and the event log, by kind, time, text and
//! category. They run locally on the lists the device returns.
//!
//! Times of the device carry no time zone. They are compared with the
//! current time of the device, from `Time:GetInfo`, so both are in the
//! same time zone.

use std::str::FromStr;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::generic::Error;
use crate::read::Reader;

use super::{Call, LogEntry, text_matches};

/// The kinds of calls to filter by: `active` stands for `active incoming`
/// and `active outgoing`, `all` for every kind.
pub const CALL_KINDS: &[&str] = &[
    "incoming", "missed", "outgoing", "rejected", "active", "all",
];

/// The categories of the event log, as the web interface shows them.
pub const CATEGORIES: &[&str] = &["system", "internet", "telephony", "wlan", "usb"];

/// The most calls read for a filter, more than a device keeps.
pub const MAX_CALLS: u32 = 1000;

/// A filter that is not valid.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum FilterError {
    #[error("unknown kind of call {0:?}, expected one of: {kinds}", kinds = CALL_KINDS.join(", "))]
    Kind(String),
    #[error("unknown category {0:?}, expected one of: {categories}", categories = CATEGORIES.join(", "))]
    Category(String),
    #[error("{0:?} is neither a duration like 30m, 24h, 7d or 2w nor a date like 2026-11-20")]
    Since(String),
}

/// The kind of call `name` stands for, one of [`CALL_KINDS`].
///
/// # Errors
///
/// [`FilterError::Kind`] if it is none of them.
pub fn call_kind(name: &str) -> Result<&'static str, FilterError> {
    let name = name.trim().to_lowercase();
    CALL_KINDS
        .iter()
        .find(|k| **k == name)
        .copied()
        .ok_or(FilterError::Kind(name))
}

/// The category `name` stands for, one of [`CATEGORIES`].
///
/// # Errors
///
/// [`FilterError::Category`] if it is none of them.
pub fn category(name: &str) -> Result<&'static str, FilterError> {
    let name = name.trim().to_lowercase();
    CATEGORIES
        .iter()
        .find(|c| **c == name)
        .copied()
        .ok_or(FilterError::Category(name))
}

/// Which calls to keep. An empty filter keeps all.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CallFilter {
    /// Kinds of [`CALL_KINDS`]; empty for all.
    pub kinds: Vec<&'static str>,
    /// Only calls of the last this many days.
    pub days: Option<u32>,
    /// Part of the name, or digits of the number.
    pub search: Option<String>,
}

impl CallFilter {
    /// Whether the filter keeps every call.
    pub fn is_empty(&self) -> bool {
        self.kinds.is_empty() && self.days.is_none() && self.search.is_none()
    }

    /// Whether `call` passes, with `now` the current time of the device
    /// in seconds, see [`now`].
    pub fn matches(&self, call: &Call, now: i64) -> bool {
        let kind = self.kinds.is_empty()
            || self.kinds.iter().any(|wanted| match *wanted {
                "all" => true,
                "active" => call.kind.starts_with("active "),
                wanted => call.kind == wanted,
            });
        let recent = self
            .days
            .is_none_or(|days| after(call.date.as_deref(), now - i64::from(days) * DAY));
        let found = self
            .search
            .as_deref()
            .is_none_or(|search| text_matches(call.name.as_deref(), call.number.iter(), search));
        kind && recent && found
    }
}

/// From when on entries of the event log are kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Since {
    /// The last this many seconds.
    Ago(i64),
    /// From this time on, in seconds like [`timestamp`].
    At(i64),
}

impl FromStr for Since {
    type Err = FilterError;

    /// A duration like `30m`, `24h`, `7d` or `2w`, or a date like
    /// `2026-11-20`, with a time like `2026-11-20 18:00` or without.
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let text = text.trim();
        let invalid = || FilterError::Since(text.to_owned());
        if let Some(at) = timestamp(text) {
            return Ok(Self::At(at));
        }
        let unit = text.chars().last().ok_or_else(invalid)?;
        let seconds = match unit {
            'm' => 60,
            'h' => 60 * 60,
            'd' => DAY,
            'w' => 7 * DAY,
            _ => return Err(invalid()),
        };
        let count: i64 = text[..text.len() - 1].parse().map_err(|_| invalid())?;
        if count < 0 {
            return Err(invalid());
        }
        count
            .checked_mul(seconds)
            .map(Self::Ago)
            .ok_or_else(invalid)
    }
}

/// Which entries of the event log to keep. An empty filter keeps all.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LogFilter {
    /// Part of the message, ignoring case.
    pub search: Option<String>,
    pub since: Option<Since>,
    /// One of [`CATEGORIES`]. Only the event log of the web interface
    /// carries categories.
    pub category: Option<&'static str>,
}

impl LogFilter {
    /// Whether the filter needs the current time of the device.
    pub fn needs_now(&self) -> bool {
        matches!(self.since, Some(Since::Ago(_)))
    }

    /// Whether `entry` passes, with `now` the current time of the device
    /// in seconds, see [`now`].
    pub fn matches(&self, entry: &LogEntry, now: i64) -> bool {
        let search = self.search.as_deref().map(str::to_lowercase);
        let found = search
            .as_deref()
            .is_none_or(|s| entry.message.to_lowercase().contains(s));
        let recent = self.since.is_none_or(|since| {
            let from = match since {
                Since::Ago(seconds) => now - seconds,
                Since::At(at) => at,
            };
            after(entry.time.as_deref(), from)
        });
        let category = self
            .category
            .is_none_or(|c| entry.category.as_deref() == Some(c));
        found && recent && category
    }
}

/// The entries that pass `filter`, at most `limit`.
pub fn filter_log(log: Vec<LogEntry>, filter: &LogFilter, now: i64, limit: usize) -> Vec<LogEntry> {
    log.into_iter()
        .filter(|e| filter.matches(e, now))
        .take(limit)
        .collect()
}

/// Reads the latest calls that pass `filter`, at most `limit`, the latest
/// first. Reads only `limit` calls without a filter.
///
/// # Errors
///
/// The errors of [`calls`](super::calls) and [`now`].
pub async fn find_calls(
    reader: &mut Reader,
    filter: &CallFilter,
    limit: u32,
) -> Result<Vec<Call>, Error> {
    if filter.is_empty() {
        return super::calls(reader, limit).await;
    }
    let calls = super::calls(reader, MAX_CALLS).await?;
    let now = if filter.days.is_some() {
        now(reader).await?
    } else {
        0
    };
    Ok(calls
        .into_iter()
        .filter(|c| filter.matches(c, now))
        .take(usize::try_from(limit).unwrap_or(usize::MAX))
        .collect())
}

/// The current time of the device in seconds, like [`timestamp`]. The
/// clock of this computer in UTC if the device does not tell it.
///
/// # Errors
///
/// The errors of reading other than missing parts, see
/// [`Reader::try_read`].
pub async fn now(reader: &mut Reader) -> Result<i64, Error> {
    let device = reader
        .try_read("Time1", "GetInfo", &[])
        .await?
        .and_then(|values| values.text("NewCurrentLocalTime"))
        .and_then(|time| timestamp(&time));
    Ok(device.unwrap_or_else(|| {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
    }))
}

const DAY: i64 = 24 * 60 * 60;

/// Whether `time` is at or after `from`. A missing or unknown time is not.
fn after(time: Option<&str>, from: i64) -> bool {
    time.and_then(timestamp).is_some_and(|t| t >= from)
}

/// The seconds since 1970-01-01 of a time like `2026-11-20`,
/// `2026-11-20 18:05`, `2026-11-20 18:05:42` or
/// `2026-11-20T18:05:42+01:00`, without regard to its time zone.
pub fn timestamp(text: &str) -> Option<i64> {
    let text = text.trim();
    let (day, time) = match text.split_once([' ', 'T']) {
        Some((day, time)) => (day, Some(time)),
        None => (text, None),
    };
    let mut parts = day.split('-').map(|p| p.parse::<i64>().ok());
    let (year, month, date) = (parts.next()??, parts.next()??, parts.next()??);
    if parts.next().is_some() || !(1..=12).contains(&month) || !(1..=31).contains(&date) {
        return None;
    }
    let seconds = match time {
        None => 0,
        Some(time) => {
            // Leaves out a time zone like `+01:00` or `Z`.
            let time = time.get(..8.min(time.len()))?;
            let time = time.trim_end_matches(['+', '-', 'Z']);
            let mut parts = time.split(':').map(|p| p.parse::<i64>().ok());
            let (hours, minutes) = (parts.next()??, parts.next()??);
            let seconds = parts.next().unwrap_or(Some(0))?;
            if hours > 23 || minutes > 59 || seconds > 60 {
                return None;
            }
            hours * 3600 + minutes * 60 + seconds
        }
    };
    Some(days(year, month, date) * DAY + seconds)
}

/// The days since 1970-01-01 of a date of the Gregorian calendar.
fn days(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let day_of_year = (153 * ((month + 9) % 12) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(kind: &'static str, date: &str) -> Call {
        Call {
            kind,
            date: Some(date.to_owned()),
            number: Some("+49 30 1234-5".to_owned()),
            name: Some("Name-2".to_owned()),
            ..Call::default()
        }
    }

    #[test]
    fn reads_times() {
        assert_eq!(timestamp("1970-01-01"), Some(0));
        assert_eq!(timestamp("1970-01-02 00:01"), Some(DAY + 60));
        assert_eq!(timestamp("2000-03-01"), Some(951_868_800));
        assert_eq!(
            timestamp("2026-11-20T12:25:44+01:00"),
            timestamp("2026-11-20 12:25:44")
        );
        assert_eq!(
            timestamp("2026-11-20T12:25Z"),
            timestamp("2026-11-20 12:25")
        );
        for odd in [
            "",
            "soon",
            "2026-13-01",
            "2026-11-32",
            "2026-11-20-1",
            "2026-11",
            "2026-11-20 25:00",
            "2026-11-20 18:61",
            "2026-11-20 18:00:99",
            "2026-11-20 18",
            "2026-11-20 x",
        ] {
            assert_eq!(timestamp(odd), None, "{odd}");
        }
    }

    #[test]
    fn reads_since() {
        assert_eq!("30m".parse(), Ok(Since::Ago(1800)));
        assert_eq!("24h".parse(), Ok(Since::Ago(DAY)));
        assert_eq!(" 7d".parse(), Ok(Since::Ago(7 * DAY)));
        assert_eq!("2w".parse(), Ok(Since::Ago(14 * DAY)));
        assert_eq!(
            "2026-11-20".parse(),
            Ok(Since::At(timestamp("2026-11-20").unwrap()))
        );
        for odd in ["", "7", "d", "7y", "-1d", "xd", "9223372036854775807w"] {
            assert_eq!(
                odd.parse::<Since>(),
                Err(FilterError::Since(odd.to_owned()))
            );
        }
        assert!(
            FilterError::Since("7y".to_owned())
                .to_string()
                .contains("24h")
        );
    }

    #[test]
    fn checks_kinds_and_categories() {
        assert_eq!(call_kind(" Missed"), Ok("missed"));
        assert_eq!(category("WLAN"), Ok("wlan"));
        let error = call_kind("lost").unwrap_err();
        assert_eq!(
            error.to_string(),
            "unknown kind of call \"lost\", expected one of: \
             incoming, missed, outgoing, rejected, active, all"
        );
        let error = category("dsl").unwrap_err();
        assert!(error.to_string().contains("system, internet"), "{error}");
    }

    #[test]
    fn filters_calls() {
        let now = timestamp("2026-11-20 12:00").unwrap();
        let calls = [
            call("missed", "2026-11-20 09:00"),
            call("active incoming", "2026-11-19 09:00"),
            call("outgoing", "2026-11-01 09:00"),
            Call::default(),
        ];
        let kept = |filter: &CallFilter| -> Vec<usize> {
            (0..calls.len())
                .filter(|i| filter.matches(&calls[*i], now))
                .collect()
        };
        assert!(CallFilter::default().is_empty());
        assert_eq!(kept(&CallFilter::default()), [0, 1, 2, 3]);
        let kinds = |kinds: Vec<&'static str>| CallFilter {
            kinds,
            ..CallFilter::default()
        };
        assert_eq!(kept(&kinds(vec!["active"])), [1]);
        assert_eq!(kept(&kinds(vec!["missed", "outgoing"])), [0, 2]);
        assert_eq!(kept(&kinds(vec!["all"])), [0, 1, 2, 3]);
        let days = CallFilter {
            days: Some(2),
            ..CallFilter::default()
        };
        assert!(!days.is_empty());
        assert_eq!(kept(&days), [0, 1]);
        let search = |text: &str| CallFilter {
            search: Some(text.to_owned()),
            ..CallFilter::default()
        };
        assert_eq!(kept(&search("name-2")), [0, 1, 2]);
        assert_eq!(kept(&search("3012")), [0, 1, 2]);
        assert_eq!(kept(&search("other")), Vec::<usize>::new());
    }

    #[test]
    fn filters_the_log() {
        let now = timestamp("2026-11-20 12:00").unwrap();
        let entry = |time: Option<&str>, message: &str, category: Option<&str>| LogEntry {
            time: time.map(str::to_owned),
            message: message.to_owned(),
            category: category.map(str::to_owned),
        };
        let log = vec![
            entry(Some("2026-11-20 11:00:00"), "WLAN on", Some("wlan")),
            entry(Some("2026-11-18 11:00:00"), "DSL ready", Some("internet")),
            entry(None, "no time", None),
        ];
        let kept = |filter: &LogFilter, limit: usize| -> Vec<String> {
            filter_log(log.clone(), filter, now, limit)
                .into_iter()
                .map(|e| e.message)
                .collect()
        };
        let all = LogFilter::default();
        assert!(!all.needs_now());
        assert_eq!(kept(&all, 2), ["WLAN on", "DSL ready"]);
        let search = LogFilter {
            search: Some("dsl".to_owned()),
            ..LogFilter::default()
        };
        assert_eq!(kept(&search, 10), ["DSL ready"]);
        let since = |since: &str| LogFilter {
            since: Some(since.parse().unwrap()),
            ..LogFilter::default()
        };
        assert!(since("24h").needs_now());
        assert!(!since("2026-11-18").needs_now());
        assert_eq!(kept(&since("24h"), 10), ["WLAN on"]);
        assert_eq!(kept(&since("2026-11-18"), 10), ["WLAN on", "DSL ready"]);
        let category = LogFilter {
            category: Some("internet"),
            ..LogFilter::default()
        };
        assert_eq!(kept(&category, 10), ["DSL ready"]);
    }
}
