//! One test per verdict, and the parts that judge the evidence.

use super::*;

fn ip(text: &str) -> IpAddr {
    text.parse().unwrap()
}

fn answered(name: &str, server: Option<&str>, addresses: &[&str]) -> Answer {
    Answer {
        server: server.map(str::to_owned),
        name: name.to_owned(),
        status: Status::Answered,
        addresses: addresses.iter().map(|a| ip(a)).collect(),
        note: None,
    }
}

fn missing(name: &str, server: Option<&str>, status: Status, note: &str) -> Answer {
    Answer {
        server: server.map(str::to_owned),
        name: name.to_owned(),
        status,
        addresses: Vec::new(),
        note: Some(note.to_owned()),
    }
}

fn config(nameservers: &[&str], search: &[&str]) -> ClientConfig {
    ClientConfig {
        source: "test".to_owned(),
        resolvers: vec![Resolver {
            nameservers: nameservers.iter().map(|s| (*s).to_owned()).collect(),
            search: search.iter().map(|s| (*s).to_owned()).collect(),
            ..Resolver::default()
        }],
    }
}

/// A report for `nas.fritz.box`, where the router and this machine both
/// answer 192.168.178.20, and this machine asks the router.
fn report() -> Report {
    Report {
        name: "nas.fritz.box".to_owned(),
        router: RouterView {
            domain: Some("fritz.box".to_owned()),
            dns_servers: vec!["192.168.178.1".to_owned()],
            address: Some("192.168.178.1".to_owned()),
            host: Some(HostEntry {
                name: "nas".to_owned(),
                ip: Some("192.168.178.20".to_owned()),
                active: true,
            }),
        },
        router_answer: answered("nas.fritz.box", Some("192.168.178.1"), &["192.168.178.20"]),
        client_answer: answered("nas.fritz.box", None, &["192.168.178.20"]),
        client_config: Some(config(&["192.168.178.1"], &["fritz.box"])),
        expected: None,
        verdict: Verdict::new(Cause::Ok, ""),
    }
}

fn judge(report: &Report) -> (Cause, String) {
    let verdict = verdict(report);
    (verdict.cause, verdict.message)
}

#[test]
fn both_answer_the_same() {
    let (cause, message) = judge(&report());
    assert_eq!(cause, Cause::Ok);
    assert_eq!(
        message,
        "the router and this machine both answer 192.168.178.20"
    );
    let mut report = report();
    report.expected = Some(Expected {
        address: ip("192.168.178.20"),
        router: Some(true),
        client: Some(true),
    });
    assert_eq!(verdict(&report).cause, Cause::Ok);
}

#[test]
fn names_mdns() {
    let mut report = report();
    report.name = "nas.local".to_owned();
    let (cause, message) = judge(&report);
    assert_eq!(cause, Cause::Mdns);
    assert!(
        message.ends_with("the router knows the host as nas.fritz.box"),
        "{message}"
    );
    report.router.domain = None;
    let (_, message) = judge(&report);
    assert!(message.ends_with("so they were not checked"), "{message}");
}

#[test]
fn names_missing_forwarding() {
    let mut report = report();
    report.client_answer = missing("nas.fritz.box", None, Status::NoSuchName, "no such name");
    report.client_config = Some(config(&["192.168.178.5"], &["fritz.box"]));
    let (cause, message) = judge(&report);
    assert_eq!(cause, Cause::NotForwarded);
    assert_eq!(
        message,
        "the router answers, but this machine asks 192.168.178.5, which does not forward \
         fritz.box to the router; add conditional forwarding of fritz.box to 192.168.178.1 there"
    );
    // A local resolver, whose upstream servers are unknown.
    report.client_config = Some(config(&["127.0.0.53"], &["fritz.box"]));
    let (_, message) = judge(&report);
    assert!(
        message.contains("asks a resolver on this machine (127.0.0.53), whose upstream"),
        "{message}"
    );
}

#[test]
fn names_a_missing_search_domain() {
    let mut report = report();
    report.name = "nas".to_owned();
    report.router_answer.name = "nas.fritz.box".to_owned();
    report.client_answer = missing("nas", None, Status::Failed, "not known");
    report.client_config = Some(config(&["192.168.178.1"], &["example.org"]));
    let (cause, message) = judge(&report);
    assert_eq!(cause, Cause::SearchDomainMissing);
    assert_eq!(
        message,
        "the router answers nas.fritz.box, but this machine does not add the search domain \
         fritz.box to nas; use the full name, or add fritz.box to the search domains of this \
         machine"
    );
}

#[test]
fn says_when_the_client_fails_for_unknown_reasons() {
    let mut report = report();
    report.client_answer = missing("nas.fritz.box", None, Status::Failed, "not known");
    // This machine asks the router itself.
    let (cause, message) = judge(&report);
    assert_eq!(cause, Cause::ClientFails);
    assert!(message.contains("(not known)"), "{message}");
    // Resolvers that cannot be told apart from the router, or none known.
    report.client_config = Some(config(&["fd00::1"], &[]));
    assert_eq!(verdict(&report).cause, Cause::ClientFails);
    report.client_config = None;
    assert_eq!(verdict(&report).cause, Cause::ClientFails);
    report.client_answer.note = None;
    assert!(verdict(&report).message.contains("(no address)"));
}

#[test]
fn names_answers_from_elsewhere() {
    let mut report = report();
    report.router_answer = missing(
        "nas.fritz.box",
        Some("192.168.178.1"),
        Status::NoSuchName,
        "no such name",
    );
    let (cause, message) = judge(&report);
    assert_eq!(cause, Cause::ClientOnly);
    assert!(
        message.starts_with("this machine resolves the name (192.168.178.20), but the router does not (no such name)"),
        "{message}"
    );
    report.router_answer.note = None;
    assert!(verdict(&report).message.contains("(no address)"));
}

#[test]
fn names_stale_records() {
    let mut report = report();
    report.client_answer = answered("nas.fritz.box", None, &["192.168.178.21", "fd00::20"]);
    let (cause, message) = judge(&report);
    assert_eq!(cause, Cause::StaleRecord);
    assert!(
        message.starts_with(
            "the router answers 192.168.178.20 and this machine 192.168.178.21, fd00::20"
        ),
        "{message}"
    );
}

#[test]
fn names_unexpected_addresses() {
    let mut report = report();
    report.expected = Some(Expected {
        address: ip("192.168.178.99"),
        router: Some(false),
        client: Some(false),
    });
    let (cause, message) = judge(&report);
    assert_eq!(cause, Cause::UnexpectedAddress);
    assert!(
        message.starts_with("both answer 192.168.178.20, not 192.168.178.99"),
        "{message}"
    );
}

#[test]
fn names_hosts_without_a_record() {
    let mut report = report();
    report.router_answer = missing(
        "nas.fritz.box",
        Some("192.168.178.1"),
        Status::NoSuchName,
        "no such name",
    );
    report.client_answer = missing("nas.fritz.box", None, Status::Failed, "not known");
    let (cause, message) = judge(&report);
    assert_eq!(cause, Cause::RouterNoRecord);
    assert_eq!(
        message,
        "the router knows the host nas (192.168.178.20, active), but its DNS server does not \
         answer for nas.fritz.box"
    );
    report.router.host = Some(HostEntry {
        name: "nas".to_owned(),
        ip: None,
        active: false,
    });
    assert!(verdict(&report).message.contains("(no address, inactive)"));
}

#[test]
fn names_unknown_names() {
    let mut report = report();
    report.router.host = None;
    report.router_answer = missing(
        "nas.fritz.box",
        Some("192.168.178.1"),
        Status::NoAddress,
        "no address",
    );
    report.client_answer = missing("nas.fritz.box", None, Status::Failed, "not known");
    assert_eq!(verdict(&report).cause, Cause::UnknownName);
}

#[test]
fn names_an_unreachable_router() {
    let mut report = report();
    report.router_answer = missing(
        "nas.fritz.box",
        Some("192.168.178.1"),
        Status::Failed,
        "no answer in time",
    );
    report.client_answer = missing("nas.fritz.box", None, Status::Failed, "not known");
    let (cause, message) = judge(&report);
    assert_eq!(cause, Cause::RouterUnreachable);
    assert!(
        message.starts_with(
            "the DNS server of the router (192.168.178.1) did not answer (no answer in time)"
        ),
        "{message}"
    );
    report.router_answer.server = None;
    report.router_answer.note = None;
    assert!(
        verdict(&report)
            .message
            .contains("(unknown) did not answer (no answer)")
    );
}

#[test]
fn names_an_unknown_router() {
    let mut report = report();
    report.router_answer = Answer::skipped("nas.fritz.box", None, "unknown");
    assert_eq!(verdict(&report).cause, Cause::RouterUnknown);
}

#[test]
fn checks_names() {
    assert_eq!(check_name("nas.fritz.box.").unwrap(), "nas.fritz.box");
    assert_eq!(check_name("my_host-1").unwrap(), "my_host-1");
    for bad in [
        "",
        ".",
        "a..b",
        "a b",
        "192.168.178.1",
        "::1",
        "nas;rm",
        "ä.example",
    ] {
        assert!(check_name(bad).is_err(), "{bad}");
    }
    assert!(check_name(&"a".repeat(64)).is_err());
    assert!(check_name(&["a"; 130].join(".")).is_err());
    assert_eq!(
        check_name("a b").unwrap_err().to_string(),
        "\"a b\" is not a host name: letters, digits, `-` and `_` in labels of up to 63 \
         characters, separated by dots"
    );
}

#[test]
fn combines_answers() {
    let both = Answer::from_results(
        "nas",
        None,
        vec![
            Ok(vec![ip("192.0.2.1"), ip("192.0.2.1")]),
            Err(Failure::NoRecord),
        ],
    );
    assert_eq!((both.status, both.addresses.len()), (Status::Answered, 1));
    let failed = Answer::from_results(
        "nas",
        None,
        vec![Err(Failure::NoSuchName), Err(Failure::Timeout)],
    );
    assert_eq!(failed.status, Status::Failed);
    assert_eq!(failed.note.as_deref(), Some("no answer in time"));
    let unknown = Answer::from_results(
        "nas",
        None,
        vec![Err(Failure::NoRecord), Err(Failure::NoSuchName)],
    );
    assert_eq!(unknown.status, Status::NoSuchName);
    let empty = Answer::from_results("nas", None, vec![Err(Failure::NoRecord)]);
    assert_eq!(empty.status, Status::NoAddress);
}

#[test]
fn compares_families() {
    let v4 = [ip("192.0.2.1")];
    let v6 = [ip("fd00::1")];
    let both = [ip("192.0.2.1"), ip("fd00::1")];
    assert!(!differ(&v4, &both));
    assert!(!differ(&v4, &v6));
    assert!(differ(&v4, &[ip("192.0.2.2")]));
    assert!(differ(&both, &[ip("fd00::2")]));
}

#[test]
fn finds_hosts_by_name() {
    assert_eq!(host_label("nas", None), Some("nas"));
    assert_eq!(host_label("nas.local", None), Some("nas"));
    assert_eq!(host_label("nas.Fritz.Box", Some("fritz.box")), Some("nas"));
    assert_eq!(host_label("nas.example.org", Some("fritz.box")), None);
    let host = |name: &str, ip: &str, active| status::Host {
        name: Some(name.to_owned()),
        ip: Some(ip.to_owned()),
        active,
        ..status::Host::default()
    };
    let hosts = [
        host("other", "192.0.2.1", true),
        host("NAS", "192.0.2.2", false),
        host("nas", "192.0.2.3", true),
    ];
    let router = RouterView::default();
    let found = host_entry(&hosts, "nas", &router).unwrap();
    assert_eq!(found.ip.as_deref(), Some("192.0.2.3"));
    assert_eq!(host_entry(&hosts, "nas.example", &router), None);
    assert_eq!(host_entry(&hosts, "missing", &router), None);
}

#[test]
fn shows_servers() {
    assert_eq!(show_server("192.0.2.1:53".parse().unwrap()), "192.0.2.1");
    assert_eq!(
        show_server("127.0.0.1:5353".parse().unwrap()),
        "127.0.0.1:5353"
    );
}

/// Probes that never answer.
struct Stuck;

impl Probes for Stuck {
    async fn query(&self, _: SocketAddr, _: &str, _: Record) -> Result<Vec<IpAddr>, Failure> {
        std::future::pending().await
    }

    async fn resolve(&self, _: &str) -> Result<Vec<IpAddr>, Failure> {
        std::future::pending().await
    }

    fn client_config(&self) -> impl Future<Output = Option<ClientConfig>> + Send {
        std::future::ready(None)
    }
}

#[tokio::test(start_paused = true)]
async fn gives_up_after_the_total_timeout() {
    let server = "192.0.2.1:53".parse().ok();
    let start = tokio::time::Instant::now();
    let (router, client, config) = ask(&Stuck, "nas", Some("fritz.box"), server).await;
    assert_eq!(start.elapsed(), TOTAL_TIMEOUT);
    assert_eq!(router.name, "nas.fritz.box");
    assert_eq!(router.server.as_deref(), Some("192.0.2.1"));
    assert_eq!(
        (router.status, client.status),
        (Status::Failed, Status::Failed)
    );
    assert_eq!(client.note.as_deref(), Some("no answer in time"));
    assert_eq!(config, None);
}

#[tokio::test]
async fn resolves_like_this_machine() {
    // localhost is answered by this machine itself, without the network.
    let addresses = System.resolve("localhost").await.unwrap();
    assert!(addresses.iter().all(IpAddr::is_loopback), "{addresses:?}");
    assert_eq!(resolved(None), Err(Failure::Timeout));
    assert_eq!(
        resolved(Some(Err(std::io::Error::other("not known")))),
        Err(Failure::Network("not known".to_owned()))
    );
    let _ = System.client_config().await;
}

#[test]
fn parses_servers() {
    assert_eq!(
        parse_server("192.0.2.1"),
        Ok("192.0.2.1:53".parse().unwrap())
    );
    assert_eq!(
        parse_server("127.0.0.1:5353"),
        Ok("127.0.0.1:5353".parse().unwrap())
    );
    assert_eq!(parse_server("fd00::1"), Ok("[fd00::1]:53".parse().unwrap()));
    assert_eq!(
        parse_server("router"),
        Err("\"router\" is not an address like 192.168.178.1 or 192.168.178.1:53".to_owned())
    );
}
