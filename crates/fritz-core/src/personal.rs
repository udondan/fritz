//! The personal lists of a device: the calls, the phonebooks and the event
//! log. They name people, numbers and devices, so they are only read on
//! request.

use serde::Serialize;

use crate::generic::Error;
use crate::lists;
use crate::read::Reader;

mod categories;
mod filter;

pub use categories::{LOG_PAGE, parse_web_log, web_log};
pub use filter::{
    CALL_KINDS, CATEGORIES, CallFilter, FilterError, LogFilter, MAX_CALLS, Since, call_kind,
    category, filter_log, find_calls, now, timestamp,
};

/// The most phonebooks read, as a guard against a device that lists
/// endless ids.
pub const MAX_PHONEBOOKS: usize = 20;

/// A call.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Call {
    /// `incoming`, `missed`, `outgoing`, `rejected`, `active incoming`,
    /// `active outgoing` or `unknown`.
    pub kind: &'static str,
    /// Like `2026-11-20 18:05`, in the time zone of the device.
    pub date: Option<String>,
    /// How long it took, in minutes.
    pub minutes: Option<u32>,
    /// The number of the other party.
    pub number: Option<String>,
    /// The name of the other party, from the phonebook.
    pub name: Option<String>,
    /// The own number the call came in on or went out from.
    pub own_number: Option<String>,
    /// The phone or answering machine that took it.
    pub device: Option<String>,
}

/// A phonebook.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Phonebook {
    pub id: u32,
    pub name: Option<String>,
    pub contacts: Vec<Contact>,
}

/// A contact of a phonebook.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Contact {
    pub name: Option<String>,
    pub numbers: Vec<ContactNumber>,
}

/// A number of a contact.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ContactNumber {
    pub number: String,
    /// Like `home`, `mobile` or `work`.
    pub kind: Option<String>,
}

impl Contact {
    /// Whether the name contains `search`, ignoring case, or a number
    /// contains its digits, ignoring spaces and dashes.
    pub fn matches(&self, search: &str) -> bool {
        text_matches(
            self.name.as_deref(),
            self.numbers.iter().map(|n| &n.number),
            search,
        )
    }
}

/// Whether `name` contains `search`, ignoring case, or one of `numbers`
/// contains its digits, ignoring spaces and dashes.
fn text_matches<'a>(
    name: Option<&str>,
    mut numbers: impl Iterator<Item = &'a String>,
    search: &str,
) -> bool {
    let text = search.to_lowercase();
    if name.is_some_and(|n| n.to_lowercase().contains(&text)) {
        return true;
    }
    let wanted = digits(search);
    !wanted.is_empty() && numbers.any(|n| digits(n).contains(&wanted))
}

/// The phonebooks with only the contacts that match `search`, see
/// [`Contact::matches`], and without the phonebooks left empty.
pub fn search(phonebooks: Vec<Phonebook>, search: &str) -> Vec<Phonebook> {
    phonebooks
        .into_iter()
        .filter_map(|mut phonebook| {
            phonebook.contacts.retain(|c| c.matches(search));
            (!phonebook.contacts.is_empty()).then_some(phonebook)
        })
        .collect()
}

/// An entry of the event log.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct LogEntry {
    /// Like `2026-11-20 18:05:42`, in the time zone of the device.
    pub time: Option<String>,
    pub message: String,
    /// One of [`CATEGORIES`], or as the device names it. Only the event
    /// log of the web interface carries categories, see [`web_log`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
}

/// Reads the latest calls, at most `max`, the latest first. Empty if the
/// device has no call list.
///
/// # Errors
///
/// The errors of reading other than missing parts, see
/// [`Reader::try_read`], and [`Error::InvalidDocument`] if the list is not
/// valid.
pub async fn calls(reader: &mut Reader, max: u32) -> Result<Vec<Call>, Error> {
    let Some(xml) = reader
        .document_with(
            "X_AVM-DE_OnTel1",
            ("GetCallList", &[]),
            "NewCallListURL",
            &format!("max={max}"),
        )
        .await?
    else {
        return Ok(Vec::new());
    };
    let mut calls = parse_calls(&xml)?;
    calls.truncate(usize::try_from(max).unwrap_or(usize::MAX));
    Ok(calls)
}

/// Parses a call list: `<root><Call><Type>1</Type>…</Call></root>`.
///
/// # Errors
///
/// [`Error::InvalidDocument`] if it is not such a list.
pub fn parse_calls(xml: &str) -> Result<Vec<Call>, Error> {
    Ok(lists::items(xml, "call list", "root", "Call")?
        .into_iter()
        .map(|item| {
            let (kind, incoming) = match item.text("Type").as_deref() {
                Some("1") => ("incoming", true),
                Some("2") => ("missed", true),
                Some("3") => ("outgoing", false),
                Some("9") => ("active incoming", true),
                Some("10") => ("rejected", true),
                Some("11") => ("active outgoing", false),
                _ => ("unknown", true),
            };
            let (number, own_number) = if incoming {
                (item.text("Caller"), item.text("CalledNumber"))
            } else {
                (item.text("Called"), item.text("CallerNumber"))
            };
            Call {
                kind,
                date: item.text("Date").map(|d| date(&d)),
                minutes: item.text("Duration").and_then(|d| minutes(&d)),
                number,
                name: item.text("Name"),
                own_number,
                device: item.text("Device"),
            }
        })
        .collect())
}

/// Reads the phonebooks with their contacts. Empty if the device has none.
///
/// # Errors
///
/// The errors of reading other than missing parts, see
/// [`Reader::try_read`], and [`Error::InvalidDocument`] if a phonebook is
/// not valid.
pub async fn phonebooks(reader: &mut Reader) -> Result<Vec<Phonebook>, Error> {
    let Some(list) = reader
        .try_read("X_AVM-DE_OnTel1", "GetPhonebookList", &[])
        .await?
        .and_then(|values| values.text("NewPhonebookList"))
    else {
        return Ok(Vec::new());
    };
    let ids = list
        .split(',')
        .filter_map(|id| id.trim().parse::<u32>().ok())
        .take(MAX_PHONEBOOKS);
    let mut phonebooks = Vec::new();
    for id in ids {
        let text = id.to_string();
        let arguments = [("NewPhonebookID", text.as_str())];
        let Some(xml) = reader
            .document_with(
                "X_AVM-DE_OnTel1",
                ("GetPhonebook", &arguments),
                "NewPhonebookURL",
                "",
            )
            .await?
        else {
            continue;
        };
        phonebooks.push(parse_phonebook(id, &xml)?);
    }
    Ok(phonebooks)
}

/// Parses a phonebook: `<phonebooks><phonebook name="…"><contact>
/// <person><realName>…</realName></person><telephony><number type="home">…
/// </number></telephony></contact></phonebook></phonebooks>`.
///
/// # Errors
///
/// [`Error::InvalidDocument`] if it is not such a phonebook.
pub fn parse_phonebook(id: u32, xml: &str) -> Result<Phonebook, Error> {
    let invalid = |reason: String| Error::InvalidDocument {
        document: "phonebook",
        reason,
    };
    let parsed = roxmltree::Document::parse(xml).map_err(|e| invalid(e.to_string()))?;
    let root = parsed.root_element();
    if root.tag_name().name() != "phonebooks" {
        return Err(invalid(format!(
            "the root is {}, not phonebooks",
            root.tag_name().name()
        )));
    }
    let text = |node: roxmltree::Node| {
        node.text()
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map(str::to_owned)
    };
    let contacts = root
        .descendants()
        .filter(|n| n.has_tag_name("contact"))
        .map(|contact| Contact {
            name: contact
                .descendants()
                .find(|n| n.has_tag_name("realName"))
                .and_then(text),
            numbers: contact
                .descendants()
                .filter(|n| n.has_tag_name("number"))
                .filter_map(|n| {
                    Some(ContactNumber {
                        number: text(n)?,
                        kind: n
                            .attribute("type")
                            .filter(|t| !t.is_empty())
                            .map(str::to_owned),
                    })
                })
                .collect(),
        })
        .collect();
    let name = root
        .children()
        .find(|n| n.has_tag_name("phonebook"))
        .and_then(|n| n.attribute("name"))
        .filter(|n| !n.is_empty())
        .map(str::to_owned);
    Ok(Phonebook { id, name, contacts })
}

/// Reads the latest entries of the event log, at most `max`, the latest
/// first. Empty if the device has no log.
///
/// # Errors
///
/// The errors of reading other than missing parts, see
/// [`Reader::try_read`].
pub async fn log(reader: &mut Reader, max: usize) -> Result<Vec<LogEntry>, Error> {
    let log = reader
        .try_read("DeviceInfo1", "GetDeviceLog", &[])
        .await?
        .and_then(|values| values.text("NewDeviceLog"))
        .unwrap_or_default();
    Ok(parse_log(&log).into_iter().take(max).collect())
}

/// Parses the event log: a line per entry, like `20.11.26 18:05:42 Text`.
/// A line without a time is kept as a message.
pub fn parse_log(log: &str) -> Vec<LogEntry> {
    log.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|line| {
            let mut parts = line.splitn(3, ' ');
            let (day, time) = (parts.next().unwrap_or_default(), parts.next());
            match (time, parts.next(), is_date(day)) {
                (Some(time), Some(message), true) if time.contains(':') => LogEntry {
                    time: Some(date(&format!("{day} {time}"))),
                    message: message.trim().to_owned(),
                    category: None,
                },
                _ => LogEntry {
                    time: None,
                    message: line.to_owned(),
                    category: None,
                },
            }
        })
        .collect()
}

/// Whether `text` is a date like `20.11.26`.
fn is_date(text: &str) -> bool {
    let parts: Vec<&str> = text.split('.').collect();
    parts.len() == 3
        && parts
            .iter()
            .all(|p| p.len() == 2 && p.bytes().all(|b| b.is_ascii_digit()))
}

/// A date of the device, like `20.11.26 18:05`, as `2026-11-20 18:05`.
/// Other texts stay as they are.
pub(crate) fn date(text: &str) -> String {
    let (day, time) = text.split_once(' ').unwrap_or((text, ""));
    if !is_date(day) {
        return text.to_owned();
    }
    let parts: Vec<&str> = day.split('.').collect();
    let date = format!("20{}-{}-{}", parts[2], parts[1], parts[0]);
    if time.is_empty() {
        date
    } else {
        format!("{date} {time}")
    }
}

/// A duration like `1:05` in minutes.
fn minutes(text: &str) -> Option<u32> {
    let (hours, minutes) = text.split_once(':')?;
    let hours: u32 = hours.trim().parse().ok()?;
    let minutes: u32 = minutes.trim().parse().ok()?;
    Some(hours * 60 + minutes)
}

fn digits(text: &str) -> String {
    text.chars().filter(char::is_ascii_digit).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_calls() {
        let xml = "<root><timestamp>1</timestamp>\
            <Call><Id>2</Id><Type>3</Type><Called>phone-2</Called><Caller></Caller>\
            <CallerNumber>phone-1</CallerNumber><Name>name-2</Name><Device>name-3</Device>\
            <Date>20.11.26 18:05</Date><Duration>1:05</Duration></Call>\
            <Call><Id>1</Id><Type>2</Type><Caller>phone-3</Caller>\
            <CalledNumber>phone-1</CalledNumber><Date>soon</Date><Duration>x</Duration></Call>\
            <Call><Type>7</Type></Call></root>";
        let calls = parse_calls(xml).unwrap();
        assert_eq!(
            calls[0],
            Call {
                kind: "outgoing",
                date: Some("2026-11-20 18:05".to_owned()),
                minutes: Some(65),
                number: Some("phone-2".to_owned()),
                name: Some("name-2".to_owned()),
                own_number: Some("phone-1".to_owned()),
                device: Some("name-3".to_owned()),
            }
        );
        assert_eq!(calls[1].kind, "missed");
        assert_eq!(calls[1].number.as_deref(), Some("phone-3"));
        assert_eq!(calls[1].own_number.as_deref(), Some("phone-1"));
        assert_eq!(calls[1].date.as_deref(), Some("soon"));
        assert_eq!(calls[1].minutes, None);
        assert_eq!(calls[2].kind, "unknown");
        let kinds: Vec<_> = ["1", "9", "10", "11"]
            .iter()
            .map(|t| {
                parse_calls(&format!("<root><Call><Type>{t}</Type></Call></root>")).unwrap()[0].kind
            })
            .collect();
        assert_eq!(
            kinds,
            ["incoming", "active incoming", "rejected", "active outgoing"]
        );
        assert!(parse_calls("<list/>").is_err());
    }

    #[test]
    fn parses_a_phonebook() {
        let xml = r#"<?xml version="1.0"?><phonebooks><phonebook name="name-1">
            <contact><person><realName>Name-2</realName></person><telephony>
            <number type="home" prio="1">+49 30 1234-5</number><number type="mobile"> </number>
            </telephony></contact><contact><telephony/></contact></phonebook></phonebooks>"#;
        let phonebook = parse_phonebook(1, xml).unwrap();
        assert_eq!(
            (phonebook.id, phonebook.name.as_deref()),
            (1, Some("name-1"))
        );
        let contacts = &phonebook.contacts;
        assert_eq!(
            contacts,
            &vec![
                Contact {
                    name: Some("Name-2".to_owned()),
                    numbers: vec![ContactNumber {
                        number: "+49 30 1234-5".to_owned(),
                        kind: Some("home".to_owned()),
                    }],
                },
                Contact::default(),
            ]
        );
        assert!(contacts[0].matches("name-2"));
        assert!(contacts[0].matches("3012"));
        assert!(!contacts[0].matches("999"));
        assert!(!contacts[0].matches("other"));
        assert!(!contacts[1].matches("x"));
        let found = search(vec![phonebook.clone()], "3012");
        assert_eq!(found[0].contacts.len(), 1);
        assert_eq!(search(vec![phonebook.clone()], "nobody"), []);
        let unnamed = parse_phonebook(0, "<phonebooks><phonebook/></phonebooks>").unwrap();
        assert_eq!(unnamed, Phonebook::default());
        assert!(parse_phonebook(0, "<root/>").is_err());
        assert!(parse_phonebook(0, "not xml").is_err());
    }

    #[test]
    fn parses_the_log() {
        let log = "20.11.26 18:05:42 Internet connected.\n\n  \
            19.11.26 07:00:01 DSL ready.\nno time here\n1.2.3 4:5 odd";
        assert_eq!(
            parse_log(log),
            vec![
                LogEntry {
                    time: Some("2026-11-20 18:05:42".to_owned()),
                    message: "Internet connected.".to_owned(),
                    category: None,
                },
                LogEntry {
                    time: Some("2026-11-19 07:00:01".to_owned()),
                    message: "DSL ready.".to_owned(),
                    category: None,
                },
                LogEntry {
                    time: None,
                    message: "no time here".to_owned(),
                    category: None,
                },
                LogEntry {
                    time: None,
                    message: "1.2.3 4:5 odd".to_owned(),
                    category: None,
                },
            ]
        );
        assert_eq!(date("20.11.26"), "2026-11-20");
        assert_eq!(minutes("0:01"), Some(1));
    }
}
