//! `fritz calls`, `phonebook` and `log`: the personal lists of the device,
//! as text, JSON or YAML.

use std::io::Write;

use anyhow::Context as _;
use clap::Args;
use fritz_core::callmonitor::{self, Event, Monitor, Names};
use fritz_core::personal::{self, Call, CallFilter, LogEntry, LogFilter, Phonebook};
use fritz_core::read::Reader;

use crate::context::Context;
use crate::output::{self, Format, show};
use crate::status::{clean, reader, table};

/// Prints the latest calls that pass `filter`.
pub(crate) async fn calls(
    context: &Context,
    filter: &CallFilter,
    limit: u32,
    format: Format,
    out: &mut impl Write,
) -> anyhow::Result<()> {
    let calls = personal::find_calls(&mut reader(context).await?, filter, limit)
        .await
        .context("cannot read the calls")?;
    show(calls.as_slice(), format, out, calls_table)
}

fn calls_table(calls: &[Call], out: &mut impl Write) -> anyhow::Result<()> {
    if calls.is_empty() {
        writeln!(out, "No calls.")?;
        return Ok(());
    }
    let text = |value: &Option<String>| value.clone().unwrap_or_default();
    let rows = calls.iter().map(|c| {
        vec![
            text(&c.date),
            c.kind.to_owned(),
            text(&c.number),
            text(&c.name),
            text(&c.own_number),
            text(&c.device),
            c.minutes.map_or_else(String::new, |m| m.to_string()),
        ]
    });
    let headers = [
        "DATE",
        "TYPE",
        "NUMBER",
        "NAME",
        "OWN NUMBER",
        "DEVICE",
        "MINUTES",
    ];
    table(&headers, rows, out)
}

/// The options of `fritz calls --watch`.
#[derive(Debug, Clone, Copy, Args)]
pub(crate) struct WatchArgs {
    /// Print the calls as they happen, from the call monitor of the device,
    /// until interrupted. JSON prints one line per event. The call monitor
    /// must be on: dial #96*5* on a connected phone.
    #[arg(long, conflicts_with_all = ["kind", "days", "search"])]
    pub(crate) watch: bool,
    /// Stop after this many events.
    #[arg(long, requires = "watch", value_parser = clap::value_parser!(u64).range(1..))]
    count: Option<u64>,
    /// Do not look up the names in the phonebooks, which needs a login.
    #[arg(long, requires = "watch")]
    no_lookup: bool,
    /// The port of the call monitor.
    #[arg(long, requires = "watch", default_value_t = callmonitor::PORT)]
    monitor_port: u16,
}

/// Prints the events of the call monitor as they come, with the names from
/// the phonebooks, and notes on lost connections to `err`.
pub(crate) async fn watch_calls(
    context: &Context,
    args: WatchArgs,
    format: Format,
    out: &mut impl Write,
    err: &mut impl Write,
) -> anyhow::Result<()> {
    let target = context.target()?;
    let host = target.url.host_str().unwrap_or_default();
    let mut monitor = Monitor::connect(host, args.monitor_port).await?;
    let names = if args.no_lookup {
        Names::default()
    } else {
        names(context, err).await
    };
    let mut note = |text: String| {
        let _ = writeln!(err, "note: {text}");
    };
    for _ in 0..args.count.unwrap_or(u64::MAX) {
        let mut event = monitor.next(&mut note).await;
        names.name(&mut event);
        output::sample(&event, format, out, event_line)?;
    }
    Ok(())
}

/// The names of the phonebooks, none with a warning if they cannot be
/// read.
async fn names(context: &Context, err: &mut impl Write) -> Names {
    let phonebooks = async {
        personal::phonebooks(&mut reader(context).await?)
            .await
            .context("cannot read the phonebooks")
    };
    match phonebooks.await {
        Ok(phonebooks) => Names::new(&phonebooks),
        Err(error) => {
            let _ = writeln!(err, "warning: names are not shown: {error:#}");
            Names::default()
        }
    }
}

/// An event as a line, like `2026-11-23 18:02:11  ring        line 0  from
/// 030987 (name)  to 030123  SIP0`.
fn event_line(event: &Event, out: &mut impl Write) -> anyhow::Result<()> {
    let mut parts = vec![
        event.time.clone(),
        format!("{:<10}", event.kind),
        format!("line {}", event.id),
    ];
    let other = match (&event.number, &event.name) {
        (Some(number), Some(name)) => format!("{number} ({})", clean(name)),
        (number, _) => number.clone().unwrap_or_else(|| "unknown".to_owned()),
    };
    let own = event
        .own_number
        .clone()
        .unwrap_or_else(|| "unknown".to_owned());
    let extension = |e: &Option<String>| format!("extension {}", e.as_deref().unwrap_or_default());
    match event.kind {
        "ring" => parts.extend([format!("from {other}"), format!("to {own}")]),
        "call" => parts.extend([
            format!("from {own}"),
            format!("to {other}"),
            extension(&event.extension),
        ]),
        "connect" => parts.extend([format!("with {other}"), extension(&event.extension)]),
        _ => parts.push(duration(event.seconds.unwrap_or_default())),
    }
    parts.extend(event.line.clone());
    writeln!(out, "{}", parts.join("  "))?;
    Ok(())
}

/// Seconds like `1h 2m 5s`, `2m 25s` or `5s`.
fn duration(seconds: u64) -> String {
    let (hours, minutes, seconds) = (seconds / 3600, seconds % 3600 / 60, seconds % 60);
    match (hours, minutes) {
        (0, 0) => format!("{seconds}s"),
        (0, _) => format!("{minutes}m {seconds}s"),
        _ => format!("{hours}h {minutes}m {seconds}s"),
    }
}

/// Prints the contacts of the phonebooks, those that match `search` if
/// given.
pub(crate) async fn phonebook(
    context: &Context,
    search: Option<&str>,
    format: Format,
    out: &mut impl Write,
) -> anyhow::Result<()> {
    let mut reader = reader(context).await?;
    let phonebooks = personal::phonebooks(&mut reader)
        .await
        .context("cannot read the phonebooks")?;
    let phonebooks = match search {
        Some(text) => personal::search(phonebooks, text),
        None => phonebooks,
    };
    show(phonebooks.as_slice(), format, out, contacts_table)
}

/// A row per number, and one for a contact without a number.
fn contacts_table(phonebooks: &[Phonebook], out: &mut impl Write) -> anyhow::Result<()> {
    let mut rows = Vec::new();
    for phonebook in phonebooks {
        let book = phonebook
            .name
            .clone()
            .unwrap_or_else(|| phonebook.id.to_string());
        for contact in &phonebook.contacts {
            let name = contact.name.clone().unwrap_or_default();
            if contact.numbers.is_empty() {
                rows.push(vec![
                    book.clone(),
                    name.clone(),
                    String::new(),
                    String::new(),
                ]);
            }
            for number in &contact.numbers {
                let kind = number.kind.clone().unwrap_or_default();
                rows.push(vec![
                    book.clone(),
                    name.clone(),
                    number.number.clone(),
                    kind,
                ]);
            }
        }
    }
    if rows.is_empty() {
        writeln!(out, "No contacts.")?;
        return Ok(());
    }
    table(
        &["PHONEBOOK", "NAME", "NUMBER", "TYPE"],
        rows.into_iter(),
        out,
    )
}

/// Prints the latest entries of the event log that pass `filter`.
pub(crate) async fn log(
    context: &Context,
    filter: &LogFilter,
    limit: usize,
    format: Format,
    out: &mut impl Write,
) -> anyhow::Result<()> {
    let log = find_log(context, &mut reader(context).await?, filter, limit).await?;
    show(log.as_slice(), format, out, log_lines)
}

/// Reads the latest entries of the event log that pass `filter`, at most
/// `limit`. A category needs the event log of the web interface, the
/// only one with categories; without it, the filter fails rather than
/// returning all entries.
pub(crate) async fn find_log(
    context: &Context,
    reader: &mut Reader,
    filter: &LogFilter,
    limit: usize,
) -> anyhow::Result<Vec<LogEntry>> {
    let no_categories = "cannot filter by category: the device shows no event log with categories";
    let log = if let Some(category) = filter.category {
        let session = context.session().await.context(no_categories)?;
        let log = personal::web_log(&session, category).await;
        // The device ends the session itself after a while.
        let _ = session.logout().await;
        log.context(no_categories)?
    } else {
        personal::log(reader, usize::MAX)
            .await
            .context("cannot read the event log")?
    };
    let now = if filter.needs_now() {
        personal::now(reader)
            .await
            .context("cannot read the time of the device")?
    } else {
        0
    };
    Ok(personal::filter_log(log, filter, now, limit))
}

fn log_lines(log: &[LogEntry], out: &mut impl Write) -> anyhow::Result<()> {
    if log.is_empty() {
        writeln!(out, "No entries.")?;
        return Ok(());
    }
    let categories = log.iter().any(|e| e.category.is_some());
    let rows = log.iter().map(|e| {
        let mut row = vec![e.time.clone().unwrap_or_default()];
        if categories {
            row.push(e.category.clone().unwrap_or_default());
        }
        row.push(e.message.clone());
        row
    });
    let headers: &[&str] = if categories {
        &["TIME", "CATEGORY", "MESSAGE"]
    } else {
        &["TIME", "MESSAGE"]
    };
    table(headers, rows, out)
}

#[cfg(test)]
mod tests {
    use fritz_core::personal::{Contact, ContactNumber};

    use super::*;
    use crate::status::tests::text;

    #[test]
    fn prints_calls() {
        let calls = vec![Call {
            kind: "missed",
            date: Some("2026-11-20 18:05".to_owned()),
            minutes: Some(0),
            number: Some("phone-2".to_owned()),
            own_number: Some("phone-1".to_owned()),
            ..Call::default()
        }];
        assert_eq!(
            text(|out| calls_table(&calls, out)),
            "DATE              TYPE    NUMBER   NAME  OWN NUMBER  DEVICE  MINUTES\n\
             2026-11-20 18:05  missed  phone-2        phone-1             0\n"
        );
        assert_eq!(text(|out| calls_table(&[], out)), "No calls.\n");
    }

    #[test]
    fn prints_events() {
        let event = |kind, number: Option<&str>, name: Option<&str>| Event {
            time: "2026-11-23 18:02:11".to_owned(),
            kind,
            id: 1,
            number: number.map(str::to_owned),
            name: name.map(str::to_owned),
            own_number: Some("phone-1".to_owned()),
            extension: Some("10".to_owned()),
            line: (kind == "ring").then(|| "SIP0".to_owned()),
            seconds: Some(3725),
        };
        let lines = text(|out| {
            event_line(&event("ring", None, None), out)?;
            event_line(&event("call", Some("phone-2"), Some("name\x1b-2")), out)?;
            event_line(&event("connect", Some("phone-2"), None), out)?;
            event_line(&event("disconnect", None, None), out)
        });
        assert_eq!(
            lines,
            "2026-11-23 18:02:11  ring        line 1  from unknown  to phone-1  SIP0\n\
             2026-11-23 18:02:11  call        line 1  from phone-1  to phone-2 (name\u{FFFD}-2)  extension 10\n\
             2026-11-23 18:02:11  connect     line 1  with phone-2  extension 10\n\
             2026-11-23 18:02:11  disconnect  line 1  1h 2m 5s\n"
        );
        assert_eq!(duration(145), "2m 25s");
        assert_eq!(duration(5), "5s");
    }

    #[test]
    fn prints_contacts() {
        let phonebooks = vec![Phonebook {
            id: 0,
            name: None,
            contacts: vec![
                Contact {
                    name: Some("name-2".to_owned()),
                    numbers: vec![ContactNumber {
                        number: "phone-2".to_owned(),
                        kind: Some("home".to_owned()),
                    }],
                },
                Contact::default(),
            ],
        }];
        assert_eq!(
            text(|out| contacts_table(&phonebooks, out)),
            "PHONEBOOK  NAME    NUMBER   TYPE\n\
             0          name-2  phone-2  home\n\
             0\n"
        );
        assert_eq!(text(|out| contacts_table(&[], out)), "No contacts.\n");
    }

    #[test]
    fn prints_the_log() {
        let log = vec![
            LogEntry {
                time: Some("2026-11-20 18:05:42".to_owned()),
                message: "Internet connected.".to_owned(),
                category: None,
            },
            LogEntry {
                time: None,
                message: "odd".to_owned(),
                category: None,
            },
        ];
        assert_eq!(
            text(|out| log_lines(&log, out)),
            "TIME                 MESSAGE\n\
             2026-11-20 18:05:42  Internet connected.\n\
             \x20                    odd\n"
        );
        assert_eq!(text(|out| log_lines(&[], out)), "No entries.\n");
        let log = [LogEntry {
            category: Some("wlan".to_owned()),
            ..log[0].clone()
        }];
        assert_eq!(
            text(|out| log_lines(&log, out)),
            "TIME                 CATEGORY  MESSAGE\n\
             2026-11-20 18:05:42  wlan      Internet connected.\n"
        );
    }
}
