//! The DSL line of the simulated 7690: its values, error counters and line
//! diagnosis, connections that are not DSL, older firmware, and how the
//! counters change between reads.

use fritz_core::dsl::{self, Errors, Link};
use fritz_core::read::Reader;
use fritz_sim::{Answer, Fixture, Injection, Sim};

const FIXTURE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/fritzbox-7690/8.25");
const INTERFACE: &str = "WANDSLInterfaceConfig1";

async fn reader(sim: &Sim) -> Reader {
    Reader::new(sim.client().build().unwrap()).await.unwrap()
}

async fn report(fixture: Fixture, injections: &[Injection]) -> dsl::Report {
    let sim = Sim::start(fixture).await.unwrap();
    for injection in injections {
        sim.inject(injection.clone());
    }
    dsl::report(&mut reader(&sim).await).await.unwrap()
}

fn fixture() -> Fixture {
    Fixture::load(FIXTURE).unwrap()
}

#[tokio::test]
async fn reads_the_line() {
    let report = report(fixture(), &[]).await;
    assert_eq!(report.access_type.as_deref(), Some("DSL"));
    assert_eq!(report.note, None);
    let details = report.details.unwrap();
    let line = &details.line;
    assert_eq!(line.status.as_deref(), Some("Up"));
    assert_eq!(line.enabled, Some(true));
    assert_eq!(line.sync, Some(true));
    assert_eq!(line.standard.as_deref(), Some("VDSL"));
    assert_eq!(line.profile.as_deref(), Some("35b"));
    assert_eq!(line.data_path.as_deref(), Some("Fast"));
    assert_eq!(line.downstream_rate, Some(274_793));
    assert_eq!(line.upstream_rate, Some(45_853));
    assert_eq!(line.downstream_max, Some(276_310));
    assert_eq!(line.upstream_max, Some(45_853));
    assert_eq!(line.downstream_noise_margin, Some(7.0));
    assert_eq!(line.upstream_noise_margin, Some(8.0));
    assert_eq!(line.downstream_attenuation, Some(14.0));
    assert_eq!(line.upstream_attenuation, Some(10.0));
    assert_eq!(line.downstream_power, Some(514));
    assert_eq!(line.upstream_power, Some(500));
    assert_eq!(
        details.errors_total,
        Errors {
            crc: Some(203),
            remote_crc: Some(0),
            fec: Some(36_486_673),
            remote_fec: Some(77_148),
            hec: Some(0),
            remote_hec: Some(0),
            errored_seconds: Some(71),
            severely_errored_seconds: Some(0),
            loss_of_framing: Some(0),
            resyncs: Some(1),
            init_errors: Some(0),
            init_timeouts: Some(0),
            cell_delineation: Some(0),
            receive_blocks: Some(343_061_557),
            transmit_blocks: Some(98_199_997),
        }
    );
    // The 7690 has no GetStatisticsShowtime.
    assert_eq!(details.errors_since_sync, None);
    assert_eq!(
        details.link,
        Some(Link {
            link_type: Some("PPPoE".to_owned()),
            link_status: Some("Up".to_owned()),
            atm_crc_errors: Some(0),
            aal5_crc_errors: Some(0),
        })
    );
    let diagnosis = details.diagnosis.unwrap();
    assert_eq!(diagnosis.state.as_deref(), Some("NONE"));
    assert_eq!(diagnosis.last_run_seconds, None);
    assert_eq!(diagnosis.signal_lost_seconds, None);
    assert_eq!(diagnosis.cable_fault_distance, None);
    assert_eq!(diagnosis.dsl_active, Some(true));
    assert_eq!(details.hints, Vec::<String>::new());
}

fn answer(pairs: &[(&str, &str)]) -> Answer {
    Answer::Ok(
        pairs
            .iter()
            .map(|(n, v)| ((*n).to_owned(), (*v).to_owned()))
            .collect(),
    )
}

#[tokio::test]
async fn says_when_the_connection_is_not_dsl() {
    let cable = fixture().with_answer(
        "WANCommonInterfaceConfig1",
        "GetCommonLinkProperties",
        None,
        answer(&[("NewWANAccessType", "Ethernet")]),
    );
    let report = report(cable, &[]).await;
    assert_eq!(report.access_type.as_deref(), Some("Ethernet"));
    assert_eq!(
        report.note.as_deref(),
        Some("the connection is not DSL but Ethernet")
    );
    assert_eq!(report.details, None);

    let repeater = Fixture::bundled("fritz-repeater-2400/8.25").unwrap();
    let report = self::report(repeater, &[]).await;
    assert_eq!(report.access_type, None);
    assert_eq!(
        report.note.as_deref(),
        Some("the device has no DSL interface")
    );
}

#[tokio::test]
async fn leaves_out_what_older_firmware_lacks() {
    let missing = [
        Injection::fault(INTERFACE, "X_AVM-DE_GetDSLDiagnoseInfo", 401),
        Injection::fault(INTERFACE, "X_AVM-DE_GetDSLInfo", 401),
        Injection::fault(INTERFACE, "GetStatisticsTotal", 401),
        Injection::fault("WANDSLLinkConfig1", "GetDSLLinkInfo", 401),
        Injection::fault("WANDSLLinkConfig1", "GetStatistics", 401),
    ];
    let details = report(fixture(), &missing).await.details.unwrap();
    assert_eq!(details.line.status.as_deref(), Some("Up"));
    assert_eq!(details.line.downstream_noise_margin, Some(7.0));
    assert_eq!(details.line.standard, None);
    assert_eq!(details.line.sync, None);
    assert_eq!(details.errors_total, Errors::default());
    assert_eq!(details.link, None);
    assert_eq!(details.diagnosis, None);

    // Without GetStatisticsTotal, CRC and FEC from X_AVM-DE_GetDSLInfo.
    let details = report(fixture(), &missing[2..3]).await.details.unwrap();
    assert_eq!(details.errors_total.crc, Some(203));
    assert_eq!(details.errors_total.fec, Some(36_486_673));
    assert_eq!(details.errors_total.hec, None);

    // Without GetInfo, the line from X_AVM-DE_GetDSLInfo.
    let only_extra = [Injection::fault(INTERFACE, "GetInfo", 401)];
    let details = report(fixture(), &only_extra).await.details.unwrap();
    assert_eq!(details.line.status.as_deref(), Some("Up"));
    assert_eq!(details.line.enabled, None);
    assert_eq!(details.line.downstream_rate, Some(274_793));
}

#[tokio::test]
async fn reports_a_cable_fault() {
    let fault = fixture().with_answer(
        INTERFACE,
        "X_AVM-DE_GetDSLDiagnoseInfo",
        None,
        answer(&[
            ("NewX_AVM-DE_CableNokDistance", "85"),
            ("NewX_AVM-DE_DSLActive", "1"),
            ("NewX_AVM-DE_DSLDiagnoseState", "DONE_CABLE_NOK"),
            ("NewX_AVM-DE_DSLLastDiagnoseTime", "12"),
            ("NewX_AVM-DE_DSLSignalLossTime", "420"),
            ("NewX_AVM-DE_DSLSync", "0"),
        ]),
    );
    let details = report(fault, &[]).await.details.unwrap();
    assert_eq!(details.line.sync, Some(false));
    let diagnosis = details.diagnosis.as_ref().unwrap();
    assert_eq!(diagnosis.cable_fault_distance, Some(85));
    assert_eq!(diagnosis.signal_lost_seconds, Some(420));
    assert_eq!(diagnosis.last_run_seconds, Some(12));
    assert_eq!(
        details.hints,
        [
            "the line is not synchronized",
            "the line lost its signal 420 s ago",
            "the line diagnosis found a cable fault about 85 m away",
        ]
    );
}

#[tokio::test]
async fn computes_changes_of_the_counters() {
    let sim = Sim::start(fixture()).await.unwrap();
    sim.inject(Injection::counter(
        INTERFACE,
        "Stats.Total.CRCErrors",
        2,
        None,
    ));
    sim.inject(Injection::counter(
        INTERFACE,
        "Stats.Total.LinkRetrain",
        1,
        None,
    ));
    let mut reader = reader(&sim).await;
    let first = dsl::report(&mut reader).await.unwrap();
    let second = dsl::report(&mut reader).await.unwrap();
    let sample = second.since(&first, 5.0);
    let changes = sample.changes.unwrap();
    // X_AVM-DE_GetDSLInfo and GetStatisticsTotal both read the CRC errors.
    assert_eq!(changes.crc, Some(4));
    assert_eq!(changes.resyncs, Some(1));
    assert_eq!(changes.fec, Some(0));
    assert_eq!(sample.status.as_deref(), Some("Up"));
    assert_eq!(
        sample.hints,
        [
            "CRC errors rose by 4 in 5 s: data was lost on the line",
            "the line resynchronized",
        ]
    );
}
