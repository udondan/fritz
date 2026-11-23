//! `fritz powerline`: the powerline adapters the device knows.

use std::io::Write;

use anyhow::Context as _;
use fritz_core::powerline::{self, Adapter, Powerline};

use super::{reader, table, yes_no};
use crate::context::Context;
use crate::output::{Format, show};

/// Prints the powerline adapters.
pub(crate) async fn powerline(
    context: &Context,
    format: Format,
    out: &mut impl Write,
) -> anyhow::Result<()> {
    let mut reader = reader(context).await?;
    let read = powerline::powerline(&mut reader).await;
    let powerline = read.context("cannot read the powerline adapters")?;
    show(&powerline, format, out, adapters)
}

/// One line per adapter, or why there is none.
fn adapters(powerline: &Powerline, out: &mut impl Write) -> anyhow::Result<()> {
    if !powerline.supported {
        writeln!(out, "The device does not support powerline adapters.")?;
        return Ok(());
    }
    if powerline.adapters.is_empty() {
        writeln!(out, "No powerline adapters.")?;
        return Ok(());
    }
    let text = |value: &Option<String>| value.clone().unwrap_or_else(|| "-".to_owned());
    let rows = powerline.adapters.iter().map(|a| {
        vec![
            text(&a.name),
            text(&a.model),
            text(&a.mac),
            yes_no(a.active),
            update(a),
        ]
    });
    table(&["NAME", "MODEL", "MAC", "ACTIVE", "UPDATE"], rows, out)
}

/// Whether an update is offered, and how the last one went.
fn update(adapter: &Adapter) -> String {
    let offered = match adapter.update_available {
        Some(true) => "available",
        Some(false) => "none",
        None => "unknown",
    };
    match &adapter.update_result {
        Some(result) => format!("{offered} (last {result})"),
        None => offered.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(powerline: &Powerline) -> String {
        let mut out = Vec::new();
        adapters(powerline, &mut out).unwrap();
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn prints_the_adapters() {
        let powerline = Powerline {
            supported: true,
            adapters: vec![
                Adapter {
                    name: Some("Office".to_owned()),
                    model: Some("FRITZ!Powerline 1260".to_owned()),
                    mac: Some("02:00:00:00:00:01".to_owned()),
                    active: true,
                    update_available: Some(true),
                    update_result: Some("failed".to_owned()),
                },
                Adapter::default(),
            ],
        };
        assert_eq!(
            text(&powerline),
            "NAME    MODEL                 MAC                ACTIVE  UPDATE\n\
             Office  FRITZ!Powerline 1260  02:00:00:00:00:01  yes     available (last failed)\n\
             -       -                     -                  no      unknown\n"
        );
    }

    #[test]
    fn says_why_there_are_none() {
        assert_eq!(
            text(&Powerline::default()),
            "The device does not support powerline adapters.\n"
        );
        let empty = Powerline {
            supported: true,
            adapters: Vec::new(),
        };
        assert_eq!(text(&empty), "No powerline adapters.\n");
    }
}
