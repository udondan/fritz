//! The DSL line in detail: its values, its error counters and the result
//! of the last line diagnosis, through actions that only read. Starting a
//! diagnosis is a write and not done here.
//!
//! Getters that older firmware lacks are left out instead of failing the
//! whole read. [`Report::since`] turns two reads into the changes of the
//! error counters in between.

use serde::Serialize;

use crate::generic::Error;
use crate::interfaces::grown;
use crate::read::{Reader, Values};

/// The noise margin below which a line tends to lose its sync, in dB.
pub const LOW_NOISE_MARGIN: f64 = 6.0;

/// The state of a line diagnosis that found a cable fault.
pub const CABLE_FAULT: &str = "DONE_CABLE_NOK";

const INTERFACE: &str = "WANDSLInterfaceConfig1";
const LINK: &str = "WANDSLLinkConfig1";

/// The DSL line, or why there is none.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Report {
    /// Like `DSL`, `Ethernet` or `X_AVM-DE_Mobile`.
    pub access_type: Option<String>,
    /// Why the line is not shown, like a connection that is not DSL.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    #[serde(flatten)]
    pub details: Option<Details>,
}

/// The values, counters and diagnosis of a DSL line.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Details {
    pub line: Line,
    /// The error counters since the last sync, where the device reports
    /// them (`GetStatisticsShowtime`).
    pub errors_since_sync: Option<Errors>,
    /// The error counters in total.
    pub errors_total: Errors,
    /// The ATM layer, where the device reports it.
    pub link: Option<Link>,
    /// The line diagnosis, where the device reports it.
    pub diagnosis: Option<Diagnosis>,
    /// Hints that put the numbers into context.
    pub hints: Vec<String>,
}

/// The values of the line.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Line {
    /// Like `Up`, `Initializing` or `NoSignal`.
    pub status: Option<String>,
    pub enabled: Option<bool>,
    /// Whether the line is synchronized.
    pub sync: Option<bool>,
    /// Like `VDSL` or `ADSL`.
    pub standard: Option<String>,
    /// Like `35b` or `17a`.
    pub profile: Option<String>,
    /// Like `Fast` or `Interleaved`.
    pub data_path: Option<String>,
    /// The synchronized and the attainable rates, in kbit/s.
    pub downstream_rate: Option<u64>,
    pub upstream_rate: Option<u64>,
    pub downstream_max: Option<u64>,
    pub upstream_max: Option<u64>,
    /// In dB.
    pub downstream_noise_margin: Option<f64>,
    pub upstream_noise_margin: Option<f64>,
    pub downstream_attenuation: Option<f64>,
    pub upstream_attenuation: Option<f64>,
    /// The transmit power as the device reports it, `None` where it
    /// reports 0 (not supported). FRITZ!OS does not document its unit.
    pub downstream_power: Option<i64>,
    pub upstream_power: Option<i64>,
}

/// The error counters of a line. The `remote_*` counters are those of the
/// exchange (ATU-C), the others those of the device.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct Errors {
    /// Blocks with errors that could not be corrected.
    pub crc: Option<u64>,
    pub remote_crc: Option<u64>,
    /// Corrected errors (forward error correction).
    pub fec: Option<u64>,
    pub remote_fec: Option<u64>,
    /// Header errors of ATM cells.
    pub hec: Option<u64>,
    pub remote_hec: Option<u64>,
    /// Seconds with at least one error.
    pub errored_seconds: Option<u64>,
    /// Seconds with many errors.
    pub severely_errored_seconds: Option<u64>,
    pub loss_of_framing: Option<u64>,
    /// Resynchronizations of the line.
    pub resyncs: Option<u64>,
    pub init_errors: Option<u64>,
    pub init_timeouts: Option<u64>,
    pub cell_delineation: Option<u64>,
    pub receive_blocks: Option<u64>,
    pub transmit_blocks: Option<u64>,
}

impl Errors {
    /// The counters in the answers of statistics actions. The first answer
    /// that has a counter wins.
    fn from_values(answers: &[&Values]) -> Self {
        let find = |name: &str| answers.iter().find_map(|values| values.number(name));
        Self {
            crc: find("NewCRCErrors"),
            remote_crc: find("NewATUCCRCErrors"),
            fec: find("NewFECErrors"),
            remote_fec: find("NewATUCFECErrors"),
            hec: find("NewHECErrors"),
            remote_hec: find("NewATUCHECErrors"),
            errored_seconds: find("NewErroredSecs"),
            severely_errored_seconds: find("NewSeverelyErroredSecs"),
            loss_of_framing: find("NewLossOfFraming"),
            resyncs: find("NewLinkRetrain"),
            init_errors: find("NewInitErrors"),
            init_timeouts: find("NewInitTimeouts"),
            cell_delineation: find("NewCellDelin"),
            receive_blocks: find("NewReceiveBlocks"),
            transmit_blocks: find("NewTransmitBlocks"),
        }
    }

    /// How much each counter grew since `earlier`, see [`grown`].
    #[must_use]
    pub fn since(&self, earlier: &Self) -> Self {
        let grown = |now: Option<u64>, then: Option<u64>| grown(then?, now?);
        Self {
            crc: grown(self.crc, earlier.crc),
            remote_crc: grown(self.remote_crc, earlier.remote_crc),
            fec: grown(self.fec, earlier.fec),
            remote_fec: grown(self.remote_fec, earlier.remote_fec),
            hec: grown(self.hec, earlier.hec),
            remote_hec: grown(self.remote_hec, earlier.remote_hec),
            errored_seconds: grown(self.errored_seconds, earlier.errored_seconds),
            severely_errored_seconds: grown(
                self.severely_errored_seconds,
                earlier.severely_errored_seconds,
            ),
            loss_of_framing: grown(self.loss_of_framing, earlier.loss_of_framing),
            resyncs: grown(self.resyncs, earlier.resyncs),
            init_errors: grown(self.init_errors, earlier.init_errors),
            init_timeouts: grown(self.init_timeouts, earlier.init_timeouts),
            cell_delineation: grown(self.cell_delineation, earlier.cell_delineation),
            receive_blocks: grown(self.receive_blocks, earlier.receive_blocks),
            transmit_blocks: grown(self.transmit_blocks, earlier.transmit_blocks),
        }
    }
}

/// The ATM layer of the line (`WANDSLLinkConfig`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Link {
    /// Like `PPPoE`.
    pub link_type: Option<String>,
    /// Like `Up`.
    pub link_status: Option<String>,
    pub atm_crc_errors: Option<u64>,
    pub aal5_crc_errors: Option<u64>,
}

/// The line diagnosis of the device and the last loss of signal. The
/// device measures the cable on its own 5 minutes after the line lost its
/// sync.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Diagnosis {
    /// `NONE`, `RUNNING`, `NO_CALIB` (could not start), `DONE`,
    /// `DONE_CABLE_OK` or `DONE_CABLE_NOK` (a cable fault).
    pub state: Option<String>,
    /// The seconds between the last measurement and the next sync, `None`
    /// if there was none (0).
    pub last_run_seconds: Option<u64>,
    /// The seconds since the line lost its sync, while it has none; `None`
    /// while it is in sync (0).
    pub signal_lost_seconds: Option<u64>,
    /// The estimated distance to a cable fault in meters, where the
    /// measurement found one and could tell (not -1).
    pub cable_fault_distance: Option<u64>,
    pub dsl_active: Option<bool>,
    pub dsl_sync: Option<bool>,
}

/// What changed on the line between two reads.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Sample {
    /// The seconds between the reads.
    pub seconds: f64,
    /// Like `Up`.
    pub status: Option<String>,
    /// How much the total counters grew, see [`grown`]. `None` if one of
    /// the reads had no DSL line.
    pub changes: Option<Errors>,
    pub hints: Vec<String>,
}

impl Report {
    /// What changed since `earlier`, read `seconds` before.
    #[must_use]
    pub fn since(&self, earlier: &Self, seconds: f64) -> Sample {
        let (Some(now), Some(then)) = (&self.details, &earlier.details) else {
            return Sample {
                seconds,
                hints: self.note.iter().cloned().collect(),
                ..Sample::default()
            };
        };
        let changes = now.errors_total.since(&then.errors_total);
        let mut hints = Vec::new();
        if let Some(crc) = changes.crc.filter(|c| *c > 0) {
            hints.push(format!(
                "CRC errors rose by {crc} in {seconds:.0} s: data was lost on the line"
            ));
        }
        if changes.resyncs.is_some_and(|r| r > 0) {
            hints.push("the line resynchronized".to_owned());
        }
        hints.extend(noise_hints(&now.line));
        Sample {
            seconds,
            status: now.line.status.clone(),
            changes: Some(changes),
            hints,
        }
    }
}

/// Reads the DSL line. On a connection that is not DSL, or a device
/// without a DSL interface, the report has a note and no details.
///
/// # Errors
///
/// The errors of reading other than missing parts, see
/// [`Reader::try_read`].
pub async fn report(reader: &mut Reader) -> Result<Report, Error> {
    let access_type = reader
        .try_read("WANCommonInterfaceConfig1", "GetCommonLinkProperties", &[])
        .await?
        .and_then(|c| c.text("NewWANAccessType"));
    if let Some(other) = access_type.as_deref().filter(|a| *a != "DSL") {
        return Ok(Report {
            note: Some(format!("the connection is not DSL but {other}")),
            access_type,
            details: None,
        });
    }
    let info = reader.try_read(INTERFACE, "GetInfo", &[]).await?;
    let extra = reader
        .try_read(INTERFACE, "X_AVM-DE_GetDSLInfo", &[])
        .await?;
    if info.is_none() && extra.is_none() {
        return Ok(Report {
            note: Some("the device has no DSL interface".to_owned()),
            access_type,
            details: None,
        });
    }
    let total = reader
        .try_read(INTERFACE, "GetStatisticsTotal", &[])
        .await?;
    let showtime = reader
        .try_read(INTERFACE, "GetStatisticsShowtime", &[])
        .await?;
    let diagnosis = reader
        .try_read(INTERFACE, "X_AVM-DE_GetDSLDiagnoseInfo", &[])
        .await?;
    let link_info = reader.try_read(LINK, "GetDSLLinkInfo", &[]).await?;
    let link_statistics = reader.try_read(LINK, "GetStatistics", &[]).await?;
    let (info, extra) = (info.unwrap_or_default(), extra.unwrap_or_default());
    let diagnosis = diagnosis.as_ref().map(diagnosis_of);
    let values = line(&info, &extra, diagnosis.as_ref());
    // The total counters, and CRC and FEC from X_AVM-DE_GetDSLInfo where the
    // device lacks GetStatisticsTotal.
    let errors_total = Errors::from_values(&total.iter().chain([&extra]).collect::<Vec<_>>());
    let link = (link_info.is_some() || link_statistics.is_some()).then(|| {
        let (info, statistics) = (
            link_info.unwrap_or_default(),
            link_statistics.unwrap_or_default(),
        );
        Link {
            link_type: info.text("NewLinkType"),
            link_status: info.text("NewLinkStatus"),
            atm_crc_errors: statistics.number("NewATMCRCErrors"),
            aal5_crc_errors: statistics.number("NewAAL5CRCErrors"),
        }
    });
    let mut details = Details {
        line: values,
        errors_since_sync: showtime.as_ref().map(|s| Errors::from_values(&[s])),
        errors_total,
        link,
        diagnosis,
        hints: Vec::new(),
    };
    details.hints = hints(&details);
    Ok(Report {
        access_type,
        note: None,
        details: Some(details),
    })
}

/// The line from the answers of `GetInfo` and `X_AVM-DE_GetDSLInfo`, whose
/// noise margins and attenuations are in tenths of a dB. `GetInfo` wins.
fn line(info: &Values, extra: &Values, diagnosis: Option<&Diagnosis>) -> Line {
    let text = |name| info.text(name).or_else(|| extra.text(name));
    let rate = |name| info.number::<u64>(name).or_else(|| extra.number(name));
    let signed = |name| info.number::<i64>(name).or_else(|| extra.number(name));
    let decibels = |name| signed(name).map(crate::status::tenths);
    // Devices that do not support it report a power of 0.
    let power = |name| signed(name).filter(|p| *p != 0);
    Line {
        status: text("NewStatus").or_else(|| extra.text("NewLinkStatus")),
        enabled: info.flag("NewEnable"),
        sync: diagnosis.and_then(|d| d.dsl_sync),
        standard: extra.text("NewModulationType"),
        profile: extra.text("NewCurrentProfile"),
        data_path: info.text("NewDataPath"),
        downstream_rate: rate("NewDownstreamCurrRate"),
        upstream_rate: rate("NewUpstreamCurrRate"),
        downstream_max: rate("NewDownstreamMaxRate"),
        upstream_max: rate("NewUpstreamMaxRate"),
        downstream_noise_margin: decibels("NewDownstreamNoiseMargin"),
        upstream_noise_margin: decibels("NewUpstreamNoiseMargin"),
        downstream_attenuation: decibels("NewDownstreamAttenuation"),
        upstream_attenuation: decibels("NewUpstreamAttenuation"),
        downstream_power: power("NewDownstreamPower"),
        upstream_power: power("NewUpstreamPower"),
    }
}

fn diagnosis_of(values: &Values) -> Diagnosis {
    let seconds = |name| values.number::<u64>(name).filter(|t| *t > 0);
    let state = values
        .text("NewX_AVM-DE_DSLDiagnoseState")
        // The spelling of the documentation.
        .or_else(|| values.text("NewX_AVM-DE_DSLDigagnoseState"));
    // The distance is 0 by default, and only valid with a cable fault.
    let fault = state.as_deref() == Some(CABLE_FAULT);
    Diagnosis {
        last_run_seconds: seconds("NewX_AVM-DE_DSLLastDiagnoseTime"),
        signal_lost_seconds: seconds("NewX_AVM-DE_DSLSignalLossTime"),
        cable_fault_distance: values
            .number::<u64>("NewX_AVM-DE_CableNokDistance")
            .filter(|_| fault),
        state,
        dsl_active: values.flag("NewX_AVM-DE_DSLActive"),
        dsl_sync: values.flag("NewX_AVM-DE_DSLSync"),
    }
}

/// Hints on low noise margins.
fn noise_hints(line: &Line) -> Vec<String> {
    [
        ("downstream", line.downstream_noise_margin),
        ("upstream", line.upstream_noise_margin),
    ]
    .into_iter()
    .filter_map(|(direction, margin)| {
        margin.filter(|m| *m < LOW_NOISE_MARGIN).map(|m| {
            format!(
                "the {direction} noise margin of {m:.1} dB is below {LOW_NOISE_MARGIN:.0} dB: \
                 the line may lose its sync"
            )
        })
    })
    .collect()
}

/// Hints on a single read.
fn hints(details: &Details) -> Vec<String> {
    let mut hints = Vec::new();
    if details.line.sync == Some(false) {
        hints.push("the line is not synchronized".to_owned());
    }
    hints.extend(noise_hints(&details.line));
    let errors = details.errors_since_sync.unwrap_or(details.errors_total);
    if let Some(seconds) = errors.severely_errored_seconds.filter(|s| *s > 0) {
        hints.push(format!(
            "{seconds} severely errored seconds: the line had phases of many errors"
        ));
    }
    if let Some(resyncs) = details.errors_total.resyncs.filter(|r| *r > 1) {
        hints.push(format!("the line resynchronized {resyncs} times"));
    }
    if let Some(diagnosis) = &details.diagnosis {
        if let Some(seconds) = diagnosis.signal_lost_seconds {
            hints.push(format!("the line lost its signal {seconds} s ago"));
        }
        match diagnosis.cable_fault_distance {
            Some(distance) => hints.push(format!(
                "the line diagnosis found a cable fault about {distance} m away"
            )),
            None if diagnosis.state.as_deref() == Some(CABLE_FAULT) => hints
                .push("the line diagnosis found a cable fault, but not how far away".to_owned()),
            None => {}
        }
    }
    hints
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::read::values;

    #[test]
    fn reads_the_line_from_both_answers() {
        let info = values(&[("NewStatus", "Up"), ("NewDownstreamNoiseMargin", "55")]);
        let extra = values(&[
            ("NewLinkStatus", "Down"),
            ("NewModulationType", "VDSL"),
            ("NewDownstreamNoiseMargin", "70"),
            ("NewUpstreamNoiseMargin", "80"),
            ("NewDownstreamPower", "-12"),
            ("NewUpstreamPower", "0"),
        ]);
        let line = line(&info, &extra, None);
        assert_eq!(line.status.as_deref(), Some("Up"));
        assert_eq!(line.standard.as_deref(), Some("VDSL"));
        assert_eq!(line.downstream_noise_margin, Some(5.5));
        assert_eq!(line.upstream_noise_margin, Some(8.0));
        assert_eq!(line.downstream_power, Some(-12));
        assert_eq!(line.upstream_power, None);
        assert_eq!(line.sync, None);
        let line = super::line(&Values::default(), &extra, None);
        assert_eq!(line.status.as_deref(), Some("Down"));
    }

    #[test]
    fn reads_the_diagnosis() {
        let diagnosis = diagnosis_of(&values(&[
            ("NewX_AVM-DE_DSLDiagnoseState", "DONE_CABLE_NOK"),
            ("NewX_AVM-DE_DSLLastDiagnoseTime", "320"),
            ("NewX_AVM-DE_DSLSignalLossTime", "0"),
            ("NewX_AVM-DE_CableNokDistance", "350"),
            ("NewX_AVM-DE_DSLSync", "0"),
        ]));
        assert_eq!(diagnosis.state.as_deref(), Some(CABLE_FAULT));
        assert_eq!(diagnosis.last_run_seconds, Some(320));
        assert_eq!(diagnosis.signal_lost_seconds, None);
        assert_eq!(diagnosis.cable_fault_distance, Some(350));
        assert_eq!(diagnosis.dsl_sync, Some(false));
        let unknown = diagnosis_of(&values(&[
            ("NewX_AVM-DE_DSLDiagnoseState", CABLE_FAULT),
            ("NewX_AVM-DE_CableNokDistance", "-1"),
        ]));
        assert_eq!(unknown.cable_fault_distance, None);
        // The default distance of 0 without a fault.
        let none = diagnosis_of(&values(&[
            ("NewX_AVM-DE_DSLDiagnoseState", "NONE"),
            ("NewX_AVM-DE_CableNokDistance", "0"),
            ("NewX_AVM-DE_DSLSignalLossTime", "42"),
        ]));
        assert_eq!(none.cable_fault_distance, None);
        assert_eq!(none.signal_lost_seconds, Some(42));
        let documented = diagnosis_of(&values(&[("NewX_AVM-DE_DSLDigagnoseState", "RUNNING")]));
        assert_eq!(documented.state.as_deref(), Some("RUNNING"));
    }

    #[test]
    fn gives_hints() {
        let details = Details {
            line: Line {
                sync: Some(false),
                downstream_noise_margin: Some(4.5),
                upstream_noise_margin: Some(9.0),
                ..Line::default()
            },
            errors_since_sync: Some(Errors {
                severely_errored_seconds: Some(3),
                ..Errors::default()
            }),
            errors_total: Errors {
                resyncs: Some(4),
                ..Errors::default()
            },
            diagnosis: Some(Diagnosis {
                signal_lost_seconds: Some(30),
                cable_fault_distance: Some(120),
                ..Diagnosis::default()
            }),
            ..Details::default()
        };
        assert_eq!(
            hints(&details),
            [
                "the line is not synchronized",
                "the downstream noise margin of 4.5 dB is below 6 dB: the line may lose its sync",
                "3 severely errored seconds: the line had phases of many errors",
                "the line resynchronized 4 times",
                "the line lost its signal 30 s ago",
                "the line diagnosis found a cable fault about 120 m away",
            ]
        );
        let unknown = Details {
            diagnosis: Some(Diagnosis {
                state: Some(CABLE_FAULT.to_owned()),
                ..Diagnosis::default()
            }),
            ..Details::default()
        };
        assert_eq!(
            hints(&unknown),
            ["the line diagnosis found a cable fault, but not how far away"]
        );
        assert_eq!(hints(&Details::default()), Vec::<String>::new());
    }

    #[test]
    fn computes_changes_between_reads() {
        let earlier = Report {
            details: Some(Details {
                errors_total: Errors {
                    crc: Some(10),
                    fec: Some(u64::from(u32::MAX)),
                    resyncs: Some(1),
                    ..Errors::default()
                },
                ..Details::default()
            }),
            ..Report::default()
        };
        let mut now = earlier.clone();
        let details = now.details.as_mut().unwrap();
        details.line.status = Some("Up".to_owned());
        details.errors_total.crc = Some(13);
        details.errors_total.fec = Some(4);
        details.errors_total.resyncs = Some(2);
        let sample = now.since(&earlier, 5.0);
        assert_eq!(sample.status.as_deref(), Some("Up"));
        let changes = sample.changes.unwrap();
        assert_eq!(changes.crc, Some(3));
        assert_eq!(changes.fec, Some(5));
        assert_eq!(changes.hec, None);
        assert_eq!(
            sample.hints,
            [
                "CRC errors rose by 3 in 5 s: data was lost on the line",
                "the line resynchronized",
            ]
        );
        let steady = earlier.since(&earlier, 5.0);
        assert_eq!(steady.hints, Vec::<String>::new());

        let gone = Report {
            note: Some("the connection is not DSL but Ethernet".to_owned()),
            ..Report::default()
        };
        let sample = gone.since(&earlier, 5.0);
        assert_eq!(sample.changes, None);
        assert_eq!(sample.hints, ["the connection is not DSL but Ethernet"]);
    }
}
