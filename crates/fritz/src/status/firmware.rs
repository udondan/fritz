//! `fritz firmware`: the installed firmware, the update the device offers,
//! automatic updates and the last update.

use std::io::Write;

use anyhow::Context as _;
use fritz_core::firmware::{self, Firmware, State};

use super::{pairs, reader};
use crate::context::Context;
use crate::output::{Format, show};

/// What is shown for a value the device does not report.
const UNKNOWN: &str = "unknown";

/// Prints the firmware state.
pub(crate) async fn firmware(
    context: &Context,
    format: Format,
    out: &mut impl Write,
) -> anyhow::Result<()> {
    let mut reader = reader(context).await?;
    let read = firmware::firmware(&mut reader).await;
    let firmware = read.context("cannot read the firmware")?;
    show(&firmware, format, out, |firmware, out| {
        pairs(&firmware_pairs(firmware), out)
    })
}

/// The firmware as lines, with `unknown` for what the device does not
/// report.
fn firmware_pairs(firmware: &Firmware) -> Vec<(&'static str, Option<String>)> {
    let or_unknown = |value: Option<String>| Some(value.unwrap_or_else(|| UNKNOWN.to_owned()));
    let installed = firmware.installed.as_ref().map(|version| {
        let build = firmware.build_type.as_ref();
        build.map_or_else(
            || version.clone(),
            |build| format!("{version} ({})", build.to_lowercase()),
        )
    });
    let state = match firmware.state {
        State::Available => "update available",
        State::NoUpdate => "no update offered",
        State::Unknown => UNKNOWN,
        State::Unsupported => "unsupported, the device has no update service",
    };
    if firmware.state == State::Unsupported {
        return vec![
            ("Installed", or_unknown(installed)),
            ("State", Some(state.to_owned())),
        ];
    }
    let offered = match (&firmware.offered, firmware.state) {
        (Some(version), _) => version.clone(),
        (None, State::NoUpdate) => "none".to_owned(),
        (None, _) => UNKNOWN.to_owned(),
    };
    // Without an update, there is no page about it.
    let info = match firmware.state {
        State::NoUpdate => firmware.info_url.clone(),
        _ => or_unknown(firmware.info_url.clone()),
    };
    vec![
        ("Installed", or_unknown(installed)),
        ("Offered", Some(offered)),
        ("Info", info),
        (
            "Auto update",
            or_unknown(firmware.auto_update.as_deref().map(auto_update)),
        ),
        ("Last update", Some(last_update(firmware))),
        ("State", Some(state.to_owned())),
    ]
}

/// What the device does with new versions by itself.
fn auto_update(mode: &str) -> String {
    match mode {
        "off" => "off",
        "check" => "notify only",
        "important" => "install important updates",
        "all" => "install all updates",
        other => other,
    }
    .to_owned()
}

/// The last update, like `successful, 2026-08-02 03:28, from 285.08.22`.
fn last_update(firmware: &Firmware) -> String {
    let result = firmware.last_update_result.as_deref().map(|r| match r {
        "succeeded" => "successful".to_owned(),
        other => other.to_owned(),
    });
    let time = firmware.last_update_time.as_deref().map(time);
    let from = firmware
        .previous_version
        .as_ref()
        .map(|v| format!("from {v}"));
    let parts: Vec<String> = [result, time, from].into_iter().flatten().collect();
    if parts.is_empty() {
        UNKNOWN.to_owned()
    } else {
        parts.join(", ")
    }
}

/// A time like `2026-08-02T03:28:11+02:00` as `2026-08-02 03:28`, in the
/// time zone of the device; other forms as they are.
fn time(time: &str) -> String {
    match (time.get(..10), time.get(10..11), time.get(11..16)) {
        (Some(date), Some("T"), Some(clock)) => format!("{date} {clock}"),
        _ => time.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(firmware: &Firmware) -> String {
        let mut out = Vec::new();
        pairs(&firmware_pairs(firmware), &mut out).unwrap();
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn prints_an_offered_update() {
        let firmware = Firmware {
            state: State::Available,
            installed: Some("8.25".to_owned()),
            build_type: Some("Release".to_owned()),
            offered: Some("8.30".to_owned()),
            info_url: Some("https://example.com/info.txt".to_owned()),
            auto_update: Some("check".to_owned()),
            last_update_time: Some("2026-10-02T03:12:44+02:00".to_owned()),
            last_update_result: Some("succeeded".to_owned()),
            previous_version: Some("8.22".to_owned()),
            ..Firmware::default()
        };
        assert_eq!(
            text(&firmware),
            "Installed    8.25 (release)\n\
             Offered      8.30\n\
             Info         https://example.com/info.txt\n\
             Auto update  notify only\n\
             Last update  successful, 2026-10-02 03:12, from 8.22\n\
             State        update available\n"
        );
    }

    #[test]
    fn prints_unknown_for_missing_values() {
        assert_eq!(
            text(&Firmware::default()),
            "Installed    unknown\n\
             Offered      unknown\n\
             Info         unknown\n\
             Auto update  unknown\n\
             Last update  unknown\n\
             State        unknown\n"
        );
        let firmware = Firmware {
            state: State::Unsupported,
            installed: Some("8.25".to_owned()),
            ..Firmware::default()
        };
        assert_eq!(
            text(&firmware),
            "Installed  8.25\n\
             State      unsupported, the device has no update service\n"
        );
    }

    #[test]
    fn names_the_modes_and_times() {
        for (mode, expected) in [
            ("off", "off"),
            ("check", "notify only"),
            ("important", "install important updates"),
            ("all", "install all updates"),
            ("other", "other"),
        ] {
            assert_eq!(auto_update(mode), expected);
        }
        assert_eq!(time("2026-10-02T03:12:44+02:00"), "2026-10-02 03:12");
        assert_eq!(time("yesterday"), "yesterday");
        let failed = Firmware {
            last_update_result: Some("failed".to_owned()),
            ..Firmware::default()
        };
        assert_eq!(last_update(&failed), "failed");
    }
}
