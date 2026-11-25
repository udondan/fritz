//! Diagnosing hosts of the simulated 7690, with a fixed network instead of
//! the resolver and TCP of this machine.

use std::net::{IpAddr, SocketAddr};

use fritz_core::diagnose::{self, Fixed, Options, Port, Report, Status};
use fritz_core::read::Reader;
use fritz_sim::{Fixture, Sim};

const FIXTURE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../fritz-sim/fixtures/fritzbox-7690/8.25");

/// Diagnoses `query` and checks that only getters were called.
async fn diagnose(query: &str, network: &Fixed, options: &Options) -> Report {
    let sim = Sim::start(Fixture::load(FIXTURE).unwrap()).await.unwrap();
    let mut reader = Reader::new(sim.client().build().unwrap()).await.unwrap();
    let report = diagnose::host(&mut reader, network, query, options)
        .await
        .unwrap();
    for request in sim.requests() {
        let action = request.action.unwrap_or_default();
        let name = action.trim_start_matches("X_AVM-DE_");
        assert!(name.is_empty() || name.starts_with("Get"), "{action}");
    }
    report
}

fn summary(report: &Report) -> Vec<(&str, Status, Option<&str>)> {
    report
        .checks
        .iter()
        .map(|c| (c.name.as_str(), c.status, c.detail.as_deref()))
        .collect()
}

fn address(text: &str) -> IpAddr {
    text.parse().unwrap()
}

#[tokio::test]
async fn diagnoses_a_host_by_ip_address() {
    let ip = address("192.0.2.30");
    let network = Fixed {
        names: vec![("device-47".to_owned(), vec![ip])],
        ports: vec![
            (SocketAddr::new(ip, 22), Port::Open),
            (SocketAddr::new(ip, 80), Port::Closed),
        ],
    };
    let options = Options {
        ports: vec![443, 80, 22],
        private_only: false,
    };
    let report = diagnose("192.0.2.30", &network, &options).await;
    assert_eq!(report.target, "192.0.2.30");
    assert_eq!(
        report.host.as_ref().unwrap().host.name.as_deref(),
        Some("device-47")
    );
    assert_eq!(
        summary(&report),
        vec![
            (
                "known to the device",
                Status::Pass,
                Some("device-47 (DHCP)")
            ),
            ("active", Status::Pass, None),
            ("address", Status::Pass, Some("192.0.2.30, DHCP")),
            (
                "link",
                Status::Pass,
                Some("LAN, 1000 Mbit/s, via device-51")
            ),
            ("name resolves", Status::Pass, Some("192.0.2.30")),
            ("TCP 22", Status::Pass, Some("open")),
            ("TCP 80", Status::Fail, Some("closed")),
            ("TCP 443", Status::Fail, Some("filtered")),
        ]
    );
    assert_eq!(report.failures(), 2);
}

#[tokio::test]
async fn shows_the_signal_of_a_wireless_host() {
    let options = Options::default();
    let report = diagnose("aa:bb:cc:00:00:0f", &Fixed::default(), &options).await;
    let link = &report.checks[3];
    assert_eq!(link.status, Status::Pass);
    assert!(
        link.detail.as_deref().unwrap().contains("signal 85%"),
        "{link:?}"
    );
    let resolves = &report.checks[4];
    assert_eq!(resolves.status, Status::Fail);
    assert!(
        resolves
            .hint
            .as_deref()
            .unwrap()
            .contains("does not resolve")
    );
    assert_eq!(report.checks[5].detail.as_deref(), Some("no port given"));
}

#[tokio::test]
async fn finds_a_stale_name() {
    let network = Fixed {
        names: vec![("device-2".to_owned(), vec![address("192.0.2.99")])],
        ..Fixed::default()
    };
    let report = diagnose("AA:BB:CC:00:00:03", &network, &Options::default()).await;
    let link = &report.checks[3];
    assert_eq!(
        link.detail.as_deref(),
        Some("WLAN 5 GHz, 866 Mbit/s, via device-50")
    );
    let resolves = &report.checks[4];
    assert_eq!(
        resolves.detail.as_deref(),
        Some("192.0.2.99 (device: 192.0.2.2)")
    );
    assert_eq!(
        resolves.hint.as_deref(),
        Some("DNS returns 192.0.2.99, the device knows 192.0.2.2")
    );
}

#[tokio::test]
async fn skips_what_an_inactive_host_cannot_answer() {
    let options = Options {
        ports: vec![22],
        private_only: false,
    };
    let report = diagnose("device-1", &Fixed::default(), &options).await;
    let statuses: Vec<_> = report.checks.iter().map(|c| c.status).collect();
    assert_eq!(statuses[1], Status::Fail);
    assert_eq!(statuses[3], Status::Skip);
    assert_eq!(report.checks[5].detail.as_deref(), Some("not connected"));
}

#[tokio::test]
async fn probes_only_private_addresses_when_asked() {
    let options = Options {
        ports: vec![22],
        private_only: true,
    };
    let report = diagnose("device-47", &Fixed::default(), &options).await;
    let ports = report.checks.last().unwrap();
    assert_eq!(ports.status, Status::Skip);
    assert_eq!(
        ports.detail.as_deref(),
        Some("192.0.2.30 is not a private address")
    );
}

#[tokio::test]
async fn names_hosts_it_cannot_pick() {
    let report = diagnose("device-14", &Fixed::default(), &Options::default()).await;
    assert!(report.host.is_none());
    let [check] = report.checks.as_slice() else {
        panic!("{report:?}");
    };
    assert_eq!(check.status, Status::Fail);
    assert_eq!(check.detail.as_deref(), Some("2 hosts match"));
    assert!(check.hint.as_deref().unwrap().contains("device-14 ("));

    let report = diagnose("nobody", &Fixed::default(), &Options::default()).await;
    assert_eq!(report.failures(), 1);
    assert!(
        report.checks[0]
            .hint
            .as_deref()
            .unwrap()
            .contains("\"nobody\"")
    );
    let unknown = diagnose("192.0.2.200", &Fixed::default(), &Options::default()).await;
    assert_eq!(unknown.checks[0].status, Status::Fail);
}
