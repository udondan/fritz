//! `fritz internet --dsl`: the DSL line in detail, its error counters and
//! the line diagnosis, once or with `--watch` as the changes of the
//! counters per interval.

use std::io::Write;

use anyhow::Context as _;
use fritz_core::dsl::{self, Details, Errors, Report, Sample};
use fritz_core::read::Reader;

use crate::context::Context;
use crate::output::{Format, show};
use crate::status::{self, both, decibels, pairs, rates, table, yes_no};
use crate::watch;

/// Prints the DSL line, or with `--watch` the changes of its error counters
/// in every interval. On a connection that is not DSL, prints why and
/// stops.
pub(crate) async fn dsl(
    context: &Context,
    options: watch::Options,
    format: Format,
    out: &mut impl Write,
) -> anyhow::Result<()> {
    let mut reader = status::reader(context).await?;
    let report = read(&mut reader).await?;
    if !options.watch || report.details.is_none() {
        return show(&report, format, out, report_text);
    }
    // The first read of the watch is the one above.
    let mut first = Some(report);
    let read = async || match first.take() {
        Some(report) => Ok(report),
        None => read(&mut reader).await,
    };
    watch::run(options, read, format, out).await
}

impl watch::Reading for Report {
    type Sample = Sample;

    fn since(&self, before: &Self, seconds: f64) -> Sample {
        Report::since(self, before, seconds)
    }

    fn text(sample: &Sample, out: &mut impl Write) -> anyhow::Result<()> {
        sample_text(sample, out)
    }
}

async fn read(reader: &mut Reader) -> anyhow::Result<Report> {
    dsl::report(reader)
        .await
        .context("cannot read the DSL line")
}

/// A counter of [`Errors`].
type Counter = fn(&Errors) -> Option<u64>;

/// The counters of [`Errors`] shown as text, with their names.
const COUNTERS: [(&str, Counter); 13] = [
    ("CRC", |e| e.crc),
    ("CRC (exchange)", |e| e.remote_crc),
    ("FEC", |e| e.fec),
    ("FEC (exchange)", |e| e.remote_fec),
    ("HEC", |e| e.hec),
    ("HEC (exchange)", |e| e.remote_hec),
    ("Errored seconds", |e| e.errored_seconds),
    ("Severely errored seconds", |e| e.severely_errored_seconds),
    ("Loss of framing", |e| e.loss_of_framing),
    ("Resyncs", |e| e.resyncs),
    ("Init errors", |e| e.init_errors),
    ("Init timeouts", |e| e.init_timeouts),
    ("Cell delineation", |e| e.cell_delineation),
];

fn report_text(report: &Report, out: &mut impl Write) -> anyhow::Result<()> {
    let Some(details) = &report.details else {
        let note = report.note.as_deref().unwrap_or("no DSL line");
        return pairs(&[("DSL", Some(format!("not available: {note}")))], out);
    };
    pairs(&line_pairs(details), out)?;
    writeln!(out)?;
    error_table(details, out)?;
    if !details.hints.is_empty() {
        writeln!(out)?;
        let hints: Vec<_> = details
            .hints
            .iter()
            .map(|h| ("Hint", Some(h.clone())))
            .collect();
        pairs(&hints, out)?;
    }
    Ok(())
}

fn line_pairs(details: &Details) -> Vec<(&'static str, Option<String>)> {
    let line = &details.line;
    let standard = [
        line.standard.clone(),
        line.profile.as_ref().map(|p| format!("profile {p}")),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(", ");
    let atm = details.link.as_ref().map(|l| {
        [l.link_type.clone(), l.link_status.clone()]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(", ")
    });
    let diagnosis = details.diagnosis.as_ref();
    let state = diagnosis.map(|d| {
        let state = d.state.as_deref().map_or("unknown", state);
        match d.last_run_seconds {
            Some(seconds) => format!("{state}, {} before the sync", duration(seconds)),
            None => state.to_owned(),
        }
    });
    vec![
        ("Status", line.status.clone()),
        ("Enabled", line.enabled.map(yes_no)),
        ("Synchronized", line.sync.map(yes_no)),
        ("Standard", Some(standard).filter(|s| !s.is_empty())),
        ("Data path", line.data_path.clone()),
        ("Rate", rates(line.downstream_rate, line.upstream_rate)),
        ("Maximum", rates(line.downstream_max, line.upstream_max)),
        (
            "Noise margin",
            decibels(line.downstream_noise_margin, line.upstream_noise_margin),
        ),
        (
            "Attenuation",
            decibels(line.downstream_attenuation, line.upstream_attenuation),
        ),
        (
            "Power",
            both(
                line.downstream_power.map(|p| p.to_string()),
                line.upstream_power.map(|p| p.to_string()),
            ),
        ),
        ("Link", atm.filter(|l| !l.is_empty())),
        (
            "Signal lost",
            diagnosis.map(|d| {
                d.signal_lost_seconds
                    .map_or_else(|| "no".to_owned(), |s| format!("{} ago", duration(s)))
            }),
        ),
        ("Diagnosis", state),
        (
            "Cable fault",
            diagnosis
                .and_then(|d| d.cable_fault_distance)
                .map(|m| format!("about {m} m away")),
        ),
    ]
}

fn error_table(details: &Details, out: &mut impl Write) -> anyhow::Result<()> {
    let number = |value: Option<u64>| value.map_or_else(String::new, |v| v.to_string());
    let since_sync = details.errors_since_sync.as_ref();
    let mut rows: Vec<Vec<String>> = COUNTERS
        .iter()
        .filter_map(|(name, counter)| {
            let total = counter(&details.errors_total);
            let since = since_sync.and_then(counter);
            (total.is_some() || since.is_some()).then(|| {
                let mut row = vec![(*name).to_owned()];
                if since_sync.is_some() {
                    row.push(number(since));
                }
                row.push(number(total));
                row
            })
        })
        .collect();
    let link = details.link.as_ref();
    for (name, value) in [
        ("ATM CRC", link.and_then(|l| l.atm_crc_errors)),
        ("AAL5 CRC", link.and_then(|l| l.aal5_crc_errors)),
    ] {
        if let Some(value) = value {
            let mut row = vec![name.to_owned()];
            if since_sync.is_some() {
                row.push(String::new());
            }
            row.push(value.to_string());
            rows.push(row);
        }
    }
    let headers: &[&str] = if since_sync.is_some() {
        &["ERRORS", "SINCE SYNC", "TOTAL"]
    } else {
        &["ERRORS", "TOTAL"]
    };
    table(headers, rows.into_iter(), out)
}

fn sample_text(sample: &Sample, out: &mut impl Write) -> anyhow::Result<()> {
    let changes = sample.changes.as_ref().map(|changes| {
        let changes: Vec<String> = COUNTERS
            .iter()
            .filter_map(|(name, counter)| {
                counter(changes)
                    .filter(|v| *v > 0)
                    .map(|v| format!("{name} +{v}"))
            })
            .collect();
        if changes.is_empty() {
            "none".to_owned()
        } else {
            changes.join(", ")
        }
    });
    let mut lines = vec![
        ("Status", sample.status.clone()),
        (
            "Changes",
            changes.map(|c| format!("{c} in {:.0} s", sample.seconds)),
        ),
    ];
    lines.extend(sample.hints.iter().map(|h| ("Hint", Some(h.clone()))));
    pairs(&lines, out)
}

/// The state of a line diagnosis in words.
fn state(state: &str) -> &str {
    match state {
        "NONE" => "none",
        "RUNNING" => "running",
        "NO_CALIB" => "could not measure (no calibration)",
        "DONE" => "done",
        "DONE_CABLE_OK" => "done, no cable fault",
        dsl::CABLE_FAULT => "done, cable fault",
        other => other,
    }
}

/// Seconds as hours, minutes and seconds, like `1h 2m 3s`.
fn duration(seconds: u64) -> String {
    let (hours, minutes, seconds) = (seconds / 3600, seconds % 3600 / 60, seconds % 60);
    match (hours, minutes) {
        (0, 0) => format!("{seconds}s"),
        (0, _) => format!("{minutes}m {seconds}s"),
        _ => format!("{hours}h {minutes}m {seconds}s"),
    }
}

#[cfg(test)]
mod tests {
    use fritz_core::dsl::{Diagnosis, Line, Link};

    use super::*;
    use crate::status::tests::text;

    #[test]
    fn prints_durations_and_states() {
        assert_eq!(duration(42), "42s");
        assert_eq!(duration(320), "5m 20s");
        assert_eq!(duration(7384), "2h 3m 4s");
        for (raw, words) in [
            ("NONE", "none"),
            ("RUNNING", "running"),
            ("NO_CALIB", "could not measure (no calibration)"),
            ("DONE", "done"),
            ("DONE_CABLE_OK", "done, no cable fault"),
            ("DONE_CABLE_NOK", "done, cable fault"),
            ("NEW", "NEW"),
        ] {
            assert_eq!(state(raw), words);
        }
    }

    fn details() -> Details {
        Details {
            line: Line {
                status: Some("Up".to_owned()),
                enabled: Some(true),
                sync: Some(true),
                standard: Some("VDSL".to_owned()),
                profile: Some("35b".to_owned()),
                downstream_rate: Some(274_793),
                upstream_rate: Some(45_853),
                downstream_noise_margin: Some(5.5),
                upstream_noise_margin: Some(8.0),
                downstream_power: Some(514),
                ..Line::default()
            },
            errors_since_sync: Some(Errors {
                crc: Some(3),
                ..Errors::default()
            }),
            errors_total: Errors {
                crc: Some(203),
                resyncs: Some(1),
                ..Errors::default()
            },
            link: Some(Link {
                link_type: Some("PPPoE".to_owned()),
                link_status: Some("Up".to_owned()),
                atm_crc_errors: Some(0),
                aal5_crc_errors: None,
            }),
            diagnosis: Some(Diagnosis {
                state: Some("DONE_CABLE_NOK".to_owned()),
                last_run_seconds: Some(320),
                signal_lost_seconds: Some(42),
                cable_fault_distance: Some(120),
                ..Diagnosis::default()
            }),
            hints: vec!["a hint".to_owned()],
        }
    }

    #[test]
    fn prints_the_line_as_text() {
        let report = Report {
            access_type: Some("DSL".to_owned()),
            note: None,
            details: Some(details()),
        };
        let output = text(|out| report_text(&report, out));
        assert_eq!(
            output,
            "Status        Up\n\
             Enabled       yes\n\
             Synchronized  yes\n\
             Standard      VDSL, profile 35b\n\
             Rate          ↓ 274.8 Mbit/s  ↑ 45.9 Mbit/s\n\
             Noise margin  ↓ 5.5 dB  ↑ 8.0 dB\n\
             Power         ↓ 514\n\
             Link          PPPoE, Up\n\
             Signal lost   42s ago\n\
             Diagnosis     done, cable fault, 5m 20s before the sync\n\
             Cable fault   about 120 m away\n\
             \n\
             ERRORS   SINCE SYNC  TOTAL\n\
             CRC      3           203\n\
             Resyncs              1\n\
             ATM CRC              0\n\
             \n\
             Hint  a hint\n"
        );
    }

    #[test]
    fn prints_a_line_without_diagnosis_as_text() {
        let report = Report {
            details: Some(Details {
                line: Line {
                    status: Some("Up".to_owned()),
                    ..Line::default()
                },
                errors_total: Errors {
                    crc: Some(2),
                    ..Errors::default()
                },
                link: Some(Link {
                    aal5_crc_errors: Some(1),
                    ..Link::default()
                }),
                diagnosis: Some(Diagnosis::default()),
                ..Details::default()
            }),
            ..Report::default()
        };
        let output = text(|out| report_text(&report, out));
        assert_eq!(
            output,
            "Status        Up\n\
             Signal lost   no\n\
             Diagnosis     unknown\n\
             \n\
             ERRORS    TOTAL\n\
             CRC       2\n\
             AAL5 CRC  1\n"
        );
    }

    #[test]
    fn prints_why_there_is_no_line() {
        let report = Report {
            access_type: Some("Ethernet".to_owned()),
            note: Some("the connection is not DSL but Ethernet".to_owned()),
            details: None,
        };
        let output = text(|out| report_text(&report, out));
        assert_eq!(
            output,
            "DSL  not available: the connection is not DSL but Ethernet\n"
        );
        let output = text(|out| report_text(&Report::default(), out));
        assert_eq!(output, "DSL  not available: no DSL line\n");
    }

    #[test]
    fn prints_samples_as_text() {
        let sample = Sample {
            seconds: 5.0,
            status: Some("Up".to_owned()),
            changes: Some(Errors {
                crc: Some(3),
                fec: Some(0),
                ..Errors::default()
            }),
            hints: vec!["CRC errors rose".to_owned()],
        };
        let output = text(|out| sample_text(&sample, out));
        assert_eq!(
            output,
            "Status   Up\n\
             Changes  CRC +3 in 5 s\n\
             Hint     CRC errors rose\n"
        );
        let steady = Sample {
            seconds: 5.0,
            changes: Some(Errors {
                fec: Some(0),
                ..Errors::default()
            }),
            ..Sample::default()
        };
        let output = text(|out| sample_text(&steady, out));
        assert_eq!(output, "Changes  none in 5 s\n");
    }
}
