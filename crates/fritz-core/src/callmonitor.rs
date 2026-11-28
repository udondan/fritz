//! The call monitor of the device: a TCP stream on port 1012 with a line
//! per event of a call, like `23.11.26 18:02:11;RING;0;030987;030123;SIP0;`.
//! It needs no login. Nothing is sent to it.
//!
//! The lines are untrusted and parsed strictly: a line that is not a known
//! event with valid fields is skipped.

use std::collections::{HashMap, VecDeque};
use std::io;
use std::time::Duration;

use serde::Serialize;
use tokio::io::{AsyncBufReadExt as _, AsyncReadExt as _, BufReader};
use tokio::net::TcpStream;

use crate::personal::{self, Phonebook};

/// The port of the call monitor.
pub const PORT: u16 = 1012;

/// How to turn the call monitor on and off.
pub const HINT: &str = "the call monitor may be off: dial #96*5* on a phone connected to the \
                        device to turn it on, #96*4* turns it off";

/// The longest line read; events are much shorter.
const MAX_LINE: u64 = 512;

/// The first wait before connecting again, doubled after every attempt up
/// to [`MAX_BACKOFF`].
const BACKOFF: Duration = Duration::from_secs(1);
const MAX_BACKOFF: Duration = Duration::from_secs(60);

/// The events remembered to drop repeated ones after connecting again.
const SEEN: usize = 64;

/// The call monitor cannot be reached.
#[derive(Debug, thiserror::Error)]
#[error("cannot connect to the call monitor at {address}: {error}; {HINT}")]
pub struct Error {
    pub address: String,
    /// Part of the message, not a source, so that it is shown once.
    pub error: io::Error,
}

/// An event of a call.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Event {
    /// Like `2026-11-23 18:02:11`, in the time zone of the device.
    pub time: String,
    /// `ring` (incoming), `call` (outgoing), `connect` or `disconnect`.
    pub kind: &'static str,
    /// The connection: the events of a call share it.
    pub id: u32,
    /// The number of the other party, none if it is withheld.
    pub number: Option<String>,
    /// The name of the other party, from the phonebooks.
    pub name: Option<String>,
    /// The own number the call came in on or went out from.
    pub own_number: Option<String>,
    /// The internal number of the phone, for `call` and `connect`.
    pub extension: Option<String>,
    /// The line, like `SIP0`, for `ring` and `call`.
    pub line: Option<String>,
    /// How long the call took, for `disconnect`.
    pub seconds: Option<u64>,
}

/// Parses a line of the call monitor, or `None` if it is not a valid
/// event.
pub fn parse(line: &str) -> Option<Event> {
    let line = line.trim_end_matches(['\r', '\n']);
    let fields: Vec<&str> = line.strip_suffix(';')?.split(';').collect();
    let (time, kind, id) = (fields.first()?, fields.get(1)?, fields.get(2)?);
    let event = Event {
        time: time_of(time)?,
        id: count(id, 5)?,
        ..Event::default()
    };
    let field = |index: usize| fields.get(index).copied();
    let event = match (*kind, fields.len()) {
        ("RING", 6) => Event {
            kind: "ring",
            number: number(field(3)?)?,
            own_number: number(field(4)?)?,
            line: Some(line_of(field(5)?)?),
            ..event
        },
        ("CALL", 7) => Event {
            kind: "call",
            extension: Some(extension(field(3)?)?),
            own_number: number(field(4)?)?,
            number: number(field(5)?)?,
            line: Some(line_of(field(6)?)?),
            ..event
        },
        ("CONNECT", 5) => Event {
            kind: "connect",
            extension: Some(extension(field(3)?)?),
            number: number(field(4)?)?,
            ..event
        },
        ("DISCONNECT", 4) => Event {
            kind: "disconnect",
            seconds: Some(count(field(3)?, 9)?),
            ..event
        },
        _ => return None,
    };
    Some(event)
}

/// A time like `23.11.26 18:02:11` as `2026-11-23 18:02:11`.
fn time_of(text: &str) -> Option<String> {
    let (day, time) = text.split_once(' ')?;
    let parts: Vec<&str> = time.split(':').collect();
    let valid = personal::is_date(day)
        && parts.len() == 3
        && parts.iter().all(|p| p.len() == 2 && is_digits(p, 2));
    valid.then(|| personal::date(text))
}

/// A number, `None` inside if empty: digits, `*` and `#`, optionally after
/// a `+`, at most 40.
#[allow(clippy::option_option)]
fn number(text: &str) -> Option<Option<String>> {
    let digits = text.strip_prefix('+').unwrap_or(text);
    let valid = text.len() <= 40
        && digits
            .bytes()
            .all(|b| b.is_ascii_digit() || b == b'*' || b == b'#');
    match text {
        "" => Some(None),
        _ if valid && !digits.is_empty() => Some(Some(text.to_owned())),
        _ => None,
    }
}

/// An internal number of up to 4 digits.
fn extension(text: &str) -> Option<String> {
    is_digits(text, 4).then(|| text.to_owned())
}

/// A line like `SIP0`: up to 16 letters and digits.
fn line_of(text: &str) -> Option<String> {
    let valid =
        !text.is_empty() && text.len() <= 16 && text.bytes().all(|b| b.is_ascii_alphanumeric());
    valid.then(|| text.to_owned())
}

/// A number of 1 to `max` digits.
fn count<T: std::str::FromStr>(text: &str, max: usize) -> Option<T> {
    is_digits(text, max).then(|| text.parse().ok()).flatten()
}

/// Whether `text` has 1 to `max` digits and nothing else.
fn is_digits(text: &str, max: usize) -> bool {
    (1..=max).contains(&text.len()) && text.bytes().all(|b| b.is_ascii_digit())
}

/// The names of the numbers in the phonebooks, found by their digits.
#[derive(Debug, Clone, Default)]
pub struct Names(HashMap<String, String>);

impl Names {
    /// The names of all numbers of `phonebooks`; the first contact wins.
    pub fn new(phonebooks: &[Phonebook]) -> Self {
        let mut names = HashMap::new();
        for contact in phonebooks.iter().flat_map(|p| &p.contacts) {
            let Some(name) = &contact.name else { continue };
            for number in &contact.numbers {
                let digits = personal::digits(&number.number);
                if !digits.is_empty() {
                    names.entry(digits).or_insert_with(|| name.clone());
                }
            }
        }
        Self(names)
    }

    /// Sets the name of the event from its number, if it has one.
    pub fn name(&self, event: &mut Event) {
        let digits = event.number.as_deref().map(personal::digits);
        event.name = digits.and_then(|d| self.0.get(&d)).cloned();
    }
}

/// A connection to the call monitor that connects again when it is lost.
#[derive(Debug)]
pub struct Monitor {
    address: (String, u16),
    stream: Option<BufReader<TcpStream>>,
    backoff: Duration,
    seen: VecDeque<(String, &'static str, u32)>,
}

impl Monitor {
    /// Connects to the call monitor on `host`, like `fritz.box` or
    /// `[fd00::1]`.
    ///
    /// # Errors
    ///
    /// [`Error`] if the connection fails, with a hint on how to turn the
    /// call monitor on.
    pub async fn connect(host: &str, port: u16) -> Result<Self, Error> {
        let host = host.trim_start_matches('[').trim_end_matches(']');
        let address = (host.to_owned(), port);
        let stream = open(&address).await.map_err(|error| Error {
            address: format!("{}:{port}", address.0),
            error,
        })?;
        Ok(Self {
            address,
            stream: Some(stream),
            backoff: BACKOFF,
            seen: VecDeque::new(),
        })
    }

    /// The next event. Connects again with growing waits when the
    /// connection is lost, and tells `note` about it. Skips invalid lines
    /// and events seen before.
    pub async fn next(&mut self, note: &mut impl FnMut(String)) -> Event {
        loop {
            if let Some(stream) = &mut self.stream {
                match read_line(stream).await {
                    Ok(Some(line)) => match self.accept(&line) {
                        Some(event) => return event,
                        None => continue,
                    },
                    Ok(None) => note("the call monitor closed the connection".to_owned()),
                    Err(error) => note(format!("lost the call monitor: {error}")),
                }
                self.stream = None;
            }
            note(format!("connecting again in {}s", self.backoff.as_secs()));
            tokio::time::sleep(self.backoff).await;
            self.backoff = (self.backoff * 2).min(MAX_BACKOFF);
            match open(&self.address).await {
                Ok(stream) => self.stream = Some(stream),
                Err(error) => note(format!("cannot connect to the call monitor: {error}")),
            }
        }
    }

    /// The event of a line, unless it is invalid or was seen before.
    fn accept(&mut self, line: &str) -> Option<Event> {
        let Some(event) = parse(line) else {
            tracing::debug!(bytes = line.len(), "skipped a line that is not an event");
            return None;
        };
        let key = (event.time.clone(), event.kind, event.id);
        if self.seen.contains(&key) {
            tracing::debug!("skipped an event seen before");
            return None;
        }
        if self.seen.len() == SEEN {
            self.seen.pop_front();
        }
        self.seen.push_back(key);
        self.backoff = BACKOFF;
        Some(event)
    }
}

async fn open(address: &(String, u16)) -> io::Result<BufReader<TcpStream>> {
    let stream = TcpStream::connect((address.0.as_str(), address.1)).await?;
    Ok(BufReader::new(stream))
}

/// The next line, `None` at the end of the stream. A line that is too long
/// comes in parts that do not parse.
async fn read_line(stream: &mut BufReader<TcpStream>) -> io::Result<Option<String>> {
    let mut line = Vec::new();
    let read = (&mut *stream)
        .take(MAX_LINE)
        .read_until(b'\n', &mut line)
        .await?;
    Ok((read > 0).then(|| String::from_utf8_lossy(&line).into_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::personal::{Contact, ContactNumber};

    #[test]
    fn parses_events() {
        let ring = parse("23.11.26 18:02:11;RING;0;030987;030123;SIP0;\r\n").unwrap();
        assert_eq!(
            ring,
            Event {
                time: "2026-11-23 18:02:11".to_owned(),
                kind: "ring",
                id: 0,
                number: Some("030987".to_owned()),
                own_number: Some("030123".to_owned()),
                line: Some("SIP0".to_owned()),
                ..Event::default()
            }
        );
        let call = parse("23.11.26 18:10:03;CALL;1;10;030123;+49157*#;SIP1;").unwrap();
        assert_eq!(
            (call.kind, call.extension.as_deref(), call.number.as_deref()),
            ("call", Some("10"), Some("+49157*#"))
        );
        let connect = parse("23.11.26 18:02:15;CONNECT;0;12;;").unwrap();
        assert_eq!((connect.kind, connect.number), ("connect", None));
        let disconnect = parse("23.11.26 18:04:40;DISCONNECT;0;145;").unwrap();
        assert_eq!(
            (disconnect.kind, disconnect.seconds),
            ("disconnect", Some(145))
        );
    }

    #[test]
    fn skips_invalid_lines() {
        for line in [
            "",
            "hello",
            "23.11.26 18:02:11;RING;0;030987;030123;SIP0;extra;",
            "23.11.26 18:02:11;HANGUP;0;",
            "23.11.2026 18:02:11;DISCONNECT;0;1;",
            "23.11.26 18:02;DISCONNECT;0;1;",
            "23.11.26 18:02:1x;DISCONNECT;0;1;",
            "23.11.26;DISCONNECT;0;1;",
            "23.11.26 18:02:11;DISCONNECT;x;1;",
            "23.11.26 18:02:11;DISCONNECT;0;-1;",
            "23.11.26 18:02:11;RING;0;03\x1b[2J;030123;SIP0;",
            "23.11.26 18:02:11;RING;0;+;030123;SIP0;",
            "23.11.26 18:02:11;RING;0;030987;030123;;",
            "23.11.26 18:02:11;RING;0;030987;030123;SIP 0;",
            "23.11.26 18:02:11;CALL;0;abc;030123;030987;SIP0;",
            "23.11.26 18:02:11;CALL;0;10;030123;030987;SIP0",
        ] {
            assert_eq!(parse(line), None, "{line:?}");
        }
        let long = format!("23.11.26 18:02:11;RING;0;{};1;SIP0;", "1".repeat(41));
        assert_eq!(parse(&long), None);
    }

    #[test]
    fn forgets_the_oldest_event_seen() {
        let mut monitor = Monitor {
            address: ("localhost".to_owned(), 1012),
            stream: None,
            backoff: BACKOFF,
            seen: VecDeque::new(),
        };
        let line = |id: usize| format!("23.11.26 18:02:11;DISCONNECT;{id};1;");
        for id in 0..=SEEN {
            assert!(monitor.accept(&line(id)).is_some());
        }
        assert_eq!(monitor.seen.len(), SEEN);
        assert!(monitor.accept(&line(SEEN)).is_none());
        assert!(monitor.accept(&line(0)).is_some());
    }

    #[test]
    fn finds_names() {
        let contact = |name: Option<&str>, number: &str| Contact {
            name: name.map(str::to_owned),
            numbers: vec![ContactNumber {
                number: number.to_owned(),
                kind: None,
            }],
        };
        let names = Names::new(&[Phonebook {
            contacts: vec![
                contact(Some("name-1"), "030 98-7"),
                contact(Some("name-2"), "030987"),
                contact(None, "0301"),
                contact(Some("name-3"), "-"),
            ],
            ..Phonebook::default()
        }]);
        let mut event = Event {
            number: Some("030987".to_owned()),
            ..Event::default()
        };
        names.name(&mut event);
        assert_eq!(event.name.as_deref(), Some("name-1"));
        event.number = Some("0301".to_owned());
        names.name(&mut event);
        assert_eq!(event.name, None);
    }
}
