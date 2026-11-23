//! The event log of the web interface, read from its page `log` with a
//! [`Session`]. Unlike `DeviceInfo:GetDeviceLog`, it carries the category
//! of every entry, like `wlan`.
//!
//! Experimental: the page is not documented, and its fields may change
//! with FRITZ!OS. Entries are objects like `{"date": "20.11.26", "time":
//! "18:05:42", "msg": "…", "group": "wlan"}`, or, on older versions,
//! lists like `["20.11.26", "18:05:42", "…", 123, "wlan"]`.

use serde_json::Value;

use crate::session::{Error, Session};

use super::{LogEntry, date};

/// The page with the event log.
pub const LOG_PAGE: &str = "log";

/// The categories by the names and numbers of the web interface.
const GROUPS: &[(&str, &str, u64)] = &[
    ("sys", "system", 1),
    ("net", "internet", 2),
    ("fon", "telephony", 3),
    ("wlan", "wlan", 4),
    ("usb", "usb", 5),
];

/// Reads the entries of `category`, one of
/// [`CATEGORIES`](super::CATEGORIES), the latest first.
///
/// The page shows the category of its field `filter`, or else the one last
/// chosen, and the device remembers that field for the web interface. So
/// this leaves the device as it was: it first reads the page without the
/// field, which names the chosen category in `data.filter`. If that is
/// `category`, this answer serves. Otherwise it reads `category` and then
/// chooses the previous category again, also if that read failed. A page
/// without `data.filter` gives nothing to restore, so the web interface then
/// keeps showing `category`.
///
/// # Errors
///
/// The errors of the requests, and those of [`parse_web_log`]. If the read
/// of `category` fails, its error, after the previous category was chosen
/// again.
pub async fn web_log(session: &Session, category: &str) -> Result<Vec<LogEntry>, Error> {
    let group = GROUPS
        .iter()
        .find(|(_, c, _)| *c == category)
        .map_or("all", |(group, ..)| group);
    let current = session.page(LOG_PAGE, &[]).await?;
    let chosen = chosen_filter(&current);
    if chosen.as_deref() == Some(group) {
        return parse_web_log(&current);
    }
    let read = session.page(LOG_PAGE, &[("filter", group)]).await;
    let restored = match &chosen {
        Some(previous) => session.page(LOG_PAGE, &[("filter", previous)]).await,
        None => Ok(String::new()),
    };
    let json = read?;
    restored?;
    parse_web_log(&json)
}

/// The filter the page `log` names in `data.filter`, like `net`.
fn chosen_filter(json: &str) -> Option<String> {
    let page: Value = serde_json::from_str(json).ok()?;
    page.pointer("/data/filter")?.as_str().map(str::to_owned)
}

/// The entries of the page `log`.
///
/// # Errors
///
/// [`Error::Invalid`] if `json` is not the page, or if its entries carry
/// no categories.
pub fn parse_web_log(json: &str) -> Result<Vec<LogEntry>, Error> {
    let invalid = |reason: &str| Error::Invalid {
        what: "the event log page",
        reason: reason.to_owned(),
    };
    let page: Value = serde_json::from_str(json).map_err(|e| invalid(&e.to_string()))?;
    let items = page
        .pointer("/data/log")
        .and_then(Value::as_array)
        .ok_or_else(|| invalid("no entries"))?;
    let entries: Vec<LogEntry> = items.iter().filter_map(entry).collect();
    if !entries.is_empty() && entries.iter().all(|e| e.category.is_none()) {
        return Err(invalid("the entries carry no categories"));
    }
    Ok(entries)
}

/// An entry, as an object or as a list.
fn entry(item: &Value) -> Option<LogEntry> {
    let field = |name: &str, index: usize| match item {
        Value::Object(fields) => fields.get(name),
        Value::Array(values) => values.get(index),
        _ => None,
    };
    let text = |name: &str, index: usize| {
        field(name, index)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|t| !t.is_empty())
    };
    let message = text("msg", 2)?.to_owned();
    let time = match (text("date", 0), text("time", 1)) {
        (Some(day), Some(time)) => Some(date(&format!("{day} {time}"))),
        (Some(day), None) => Some(date(day)),
        _ => None,
    };
    let category = field("group", 4).and_then(group);
    Some(LogEntry {
        time,
        message,
        category,
    })
}

/// The category of a group like `wlan` or `4`.
fn group(value: &Value) -> Option<String> {
    let known = match value {
        Value::String(name) => GROUPS.iter().find(|(n, ..)| n == name),
        Value::Number(number) => GROUPS.iter().find(|(.., n)| number.as_u64() == Some(*n)),
        _ => return None,
    };
    match (known, value) {
        (Some((_, category, _)), _) => Some((*category).to_owned()),
        (None, Value::String(name)) if !name.is_empty() => Some(name.clone()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_entries_with_categories() {
        let json = r#"{"pid": "log", "data": {"log": [
            {"date": "20.11.26", "time": "18:05:42", "msg": "WLAN on.", "group": "wlan", "id": 1},
            {"date": "20.11.26", "msg": "Internet up.", "group": "net"},
            {"msg": "Odd.", "group": "aha"},
            ["19.11.26", "07:00:01", "Phone ready.", 2, 3],
            ["19.11.26", "07:00:00", "Booted.", 3, "sys"],
            ["19.11.26", "07:00:00", "Plain.", 4],
            {"date": "19.11.26", "group": "usb"},
            {"msg": "Nothing.", "group": ""},
            {"msg": "Flag.", "group": true},
            "odd"
        ]}}"#;
        let log = parse_web_log(json).unwrap();
        let summary: Vec<_> = log
            .iter()
            .map(|e| (e.time.as_deref(), e.message.as_str(), e.category.as_deref()))
            .collect();
        assert_eq!(
            summary,
            [
                (Some("2026-11-20 18:05:42"), "WLAN on.", Some("wlan")),
                (Some("2026-11-20"), "Internet up.", Some("internet")),
                (None, "Odd.", Some("aha")),
                (
                    Some("2026-11-19 07:00:01"),
                    "Phone ready.",
                    Some("telephony")
                ),
                (Some("2026-11-19 07:00:00"), "Booted.", Some("system")),
                (Some("2026-11-19 07:00:00"), "Plain.", None),
                (None, "Nothing.", None),
                (None, "Flag.", None),
            ]
        );
        assert_eq!(parse_web_log(r#"{"data": {"log": []}}"#).unwrap(), []);
    }

    #[test]
    fn rejects_pages_without_categories() {
        for (json, reason) in [
            ("<html>", "expected value"),
            (r#"{"data": {}}"#, "no entries"),
            (
                r#"{"data": {"log": [{"msg": "x"}]}}"#,
                "the entries carry no categories",
            ),
        ] {
            let error = parse_web_log(json).unwrap_err().to_string();
            assert!(
                error.starts_with("invalid answer from the event log page: ")
                    && error.contains(reason),
                "{json}: {error}"
            );
        }
    }
}
