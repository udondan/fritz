use crate::host::Attachment;

use super::*;

const IP: Ipv4Addr = Ipv4Addr::new(192, 168, 178, 23);

fn nas() -> Host {
    Host {
        name: Some("nas".to_owned()),
        ip: Some(IP.to_string()),
        mac: Some("AA:BB:CC:00:00:01".to_owned()),
        active: true,
        interface: Some("Ethernet".to_owned()),
        address_source: Some("DHCP".to_owned()),
        speed: Some(1000),
        ..Host::default()
    }
}

fn detail(host: Host, attached_to: Option<Attachment>) -> Detail {
    Detail {
        host,
        lease_remaining: None,
        attached_to,
    }
}

fn wireless(node: &str) -> Attachment {
    Attachment {
        node: Some(node.to_owned()),
        kind: Some("WLAN".to_owned()),
        band: Some("5 GHz".to_owned()),
        ..Attachment::default()
    }
}

fn line(check: &Check) -> (Status, Option<&str>, Option<&str>) {
    (check.status, check.detail.as_deref(), check.hint.as_deref())
}

#[tokio::test]
async fn passes_a_healthy_host() {
    let network = Fixed {
        names: vec![("nas".to_owned(), vec![IpAddr::V4(IP)])],
        ports: vec![(SocketAddr::new(IpAddr::V4(IP), 445), Port::Open)],
    };
    let options = Options {
        ports: vec![445, 445],
        private_only: true,
    };
    let checks = checks(&detail(nas(), None), None, &network, &options).await;
    let lines: Vec<_> = checks
        .iter()
        .map(|c| (c.name.as_str(), c.status, c.detail.as_deref()))
        .collect();
    assert_eq!(
        lines,
        vec![
            (KNOWN, Status::Pass, Some("nas (DHCP)")),
            (ACTIVE, Status::Pass, None),
            (ADDRESS, Status::Pass, Some("192.168.178.23, DHCP")),
            (LINK, Status::Pass, Some("LAN, 1000 Mbit/s")),
            (RESOLVES, Status::Pass, Some("192.168.178.23")),
            ("TCP 445", Status::Pass, Some("open")),
        ]
    );
}

#[tokio::test]
async fn names_the_failures() {
    let host = Host {
        active: false,
        name: Some("bad name".to_owned()),
        address_source: None,
        ..nas()
    };
    let options = Options {
        ports: vec![22],
        private_only: false,
    };
    let checks = checks(&detail(host, None), None, &Fixed::default(), &options).await;
    assert_eq!(checks[0].detail.as_deref(), Some("bad name"));
    assert_eq!(checks[1].status, Status::Fail);
    assert!(checks[1].hint.as_deref().unwrap().contains("asleep"));
    assert_eq!(
        checks[2].detail.as_deref(),
        Some("192.168.178.23, last known")
    );
    assert_eq!(
        line(&checks[3]),
        (Status::Skip, Some("not connected"), None)
    );
    assert_eq!(
        line(&checks[4]),
        (Status::Skip, Some("the name is not a DNS name"), None)
    );
    assert_eq!(
        line(&checks[5]),
        (Status::Skip, Some("not connected"), None)
    );
}

#[test]
fn checks_the_address() {
    let none = Host { ip: None, ..nas() };
    let check = address(&none, None);
    assert_eq!(check.status, Status::Fail);
    assert!(check.hint.unwrap().contains("no IPv4 address"));
    let other = Host {
        ip: Some("fd00::23".to_owned()),
        ..nas()
    };
    assert_eq!(
        line(&address(&other, None)),
        (Status::Unknown, Some("fd00::23"), None)
    );
    let host = Host {
        name: None,
        ..nas()
    };
    assert_eq!(known(&host).detail.as_deref(), Some("(no name) (DHCP)"));
}

#[test]
fn hints_at_a_weak_signal() {
    let host = Host {
        interface: Some("802.11".to_owned()),
        speed: None,
        ..nas()
    };
    let attached = detail(host.clone(), Some(wireless("Repeater")));
    assert!(is_wireless(&attached));
    let weak = link(&attached, Some(WEAK_SIGNAL - 1));
    assert_eq!(
        line(&weak),
        (
            Status::Fail,
            Some("WLAN 5 GHz, signal 29%, via Repeater"),
            Some(
                "the host is attached to Repeater with a weak signal of 29%; move it closer or \
                 add a repeater"
            )
        )
    );
    let strong = link(&attached, Some(80));
    assert_eq!(strong.status, Status::Pass);
    let direct = link(&detail(host.clone(), None), Some(10));
    assert!(direct.hint.unwrap().contains("attached to the device"));
    let unknown = Host {
        interface: None,
        ..host
    };
    assert_eq!(link(&detail(unknown, None), None).status, Status::Unknown);
    let other = Host {
        interface: Some("HomePlug".to_owned()),
        ..nas()
    };
    assert_eq!(link_kind(&detail(other, None)).as_deref(), Some("HomePlug"));
}

#[test]
fn compares_the_resolved_address() {
    let stale = Ipv4Addr::new(192, 168, 178, 40);
    let check = resolution(Some("nas"), Some(IP), Some(Ok(vec![IpAddr::V4(stale)])));
    assert_eq!(
        line(&check),
        (
            Status::Fail,
            Some("192.168.178.40 (device: 192.168.178.23)"),
            Some("DNS returns 192.168.178.40, the device knows 192.168.178.23")
        )
    );
    let missing = resolution(
        Some("nas"),
        Some(IP),
        Some(Err(io::ErrorKind::NotFound.into())),
    );
    assert_eq!(missing.status, Status::Fail);
    assert!(missing.hint.unwrap().starts_with("nas does not resolve"));
    let unknown = resolution(Some("nas"), None, Some(Ok(vec![IpAddr::V4(IP)])));
    assert_eq!(
        line(&unknown),
        (Status::Unknown, Some("192.168.178.23"), None)
    );
    assert_eq!(
        line(&resolution(None, Some(IP), None)),
        (Status::Skip, Some("the host has no name"), None)
    );
}

#[test]
fn knows_dns_names() {
    for name in ["nas", "nas.fritz.box", "nas.", "my-nas"] {
        assert!(is_dns_name(name), "{name}");
    }
    for name in ["", "bad name", "a..b", "nas\u{1b}", &"a".repeat(64)] {
        assert!(!is_dns_name(name), "{name}");
    }
}

#[tokio::test]
async fn probes_only_what_makes_sense() {
    let network = Fixed {
        ports: vec![(SocketAddr::new(IpAddr::V4(IP), 80), Port::Closed)],
        ..Fixed::default()
    };
    let options = |private_only| Options {
        ports: vec![80, 81],
        private_only,
    };
    let skipped = ports(&network, true, Some(IP), &Options::default()).await;
    assert_eq!(skipped[0].detail.as_deref(), Some("no port given"));
    let skipped = ports(&network, true, None, &options(false)).await;
    assert_eq!(skipped[0].detail.as_deref(), Some("no IPv4 address"));
    let public = Ipv4Addr::new(192, 0, 2, 1);
    let skipped = ports(&network, true, Some(public), &options(true)).await;
    assert_eq!(
        line(&skipped[0]),
        (
            Status::Skip,
            Some("192.0.2.1 is not a private address"),
            None
        )
    );
    let probed = ports(&network, true, Some(IP), &options(true)).await;
    assert_eq!(probed[0].name, "TCP 80");
    assert_eq!(probed[0].detail.as_deref(), Some("closed"));
    assert!(
        probed[0]
            .hint
            .as_deref()
            .unwrap()
            .contains("refuses port 80")
    );
    assert_eq!(probed[1].detail.as_deref(), Some("filtered"));
    assert!(probed[1].hint.as_deref().unwrap().contains("within 1 s"));
}

#[test]
fn lists_the_candidates() {
    let hosts: Vec<Host> = (0..12)
        .map(|i| Host {
            name: Some("printer".to_owned()),
            ip: (i > 0).then(|| format!("192.0.2.{i}")),
            mac: Some(format!("AA:BB:CC:00:00:{i:02}")),
            ..Host::default()
        })
        .collect();
    let check = ambiguous(&hosts);
    assert_eq!(check.detail.as_deref(), Some("12 hosts match"));
    let hint = check.hint.unwrap();
    assert!(
        hint.starts_with(
            "give one by its MAC or IP address: printer (AA:BB:CC:00:00:00), printer (192.0.2.1)"
        ),
        "{hint}"
    );
    assert!(hint.ends_with("printer (192.0.2.9) and 2 more"), "{hint}");
    assert_eq!(
        ambiguous(&hosts[..2])
            .hint
            .unwrap()
            .matches("printer")
            .count(),
        2
    );
}

#[test]
fn counts_failures() {
    let report = Report {
        target: "nas".to_owned(),
        host: None,
        checks: vec![
            Check::new(ACTIVE, Status::Pass, None),
            Check::fail(LINK, None, "hint".to_owned()),
        ],
    };
    assert_eq!(report.failures(), 1);
    let json = serde_json::to_value(&report).unwrap();
    assert_eq!(json["checks"][1]["status"], "fail");
    assert!(json.get("host").is_none());
}
