//! `fritz calls`, `phonebook` and `log`: the personal lists of the device,
//! as text, JSON or YAML.

use std::io::Write;

use anyhow::Context as _;
use fritz_core::personal::{self, Call, LogEntry, Phonebook};

use crate::context::Context;
use crate::output::{Format, show};
use crate::status::{reader, table};

/// Prints the latest calls.
pub(crate) async fn calls(
    context: &Context,
    limit: u32,
    format: Format,
    out: &mut impl Write,
) -> anyhow::Result<()> {
    let calls = personal::calls(&mut reader(context).await?, limit)
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

/// Prints the latest entries of the event log.
pub(crate) async fn log(
    context: &Context,
    limit: usize,
    format: Format,
    out: &mut impl Write,
) -> anyhow::Result<()> {
    let log = personal::log(&mut reader(context).await?, limit)
        .await
        .context("cannot read the event log")?;
    show(log.as_slice(), format, out, log_lines)
}

fn log_lines(log: &[LogEntry], out: &mut impl Write) -> anyhow::Result<()> {
    if log.is_empty() {
        writeln!(out, "No entries.")?;
        return Ok(());
    }
    let rows = log
        .iter()
        .map(|e| vec![e.time.clone().unwrap_or_default(), e.message.clone()]);
    table(&["TIME", "MESSAGE"], rows, out)
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
            },
            LogEntry {
                time: None,
                message: "odd".to_owned(),
            },
        ];
        assert_eq!(
            text(|out| log_lines(&log, out)),
            "TIME                 MESSAGE\n\
             2026-11-20 18:05:42  Internet connected.\n\
             \x20                    odd\n"
        );
        assert_eq!(text(|out| log_lines(&[], out)), "No entries.\n");
    }
}
