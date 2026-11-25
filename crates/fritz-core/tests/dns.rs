//! `fritz_core::dns` against the simulated device and a DNS server on the
//! loopback interface. The resolver of this machine and its configuration
//! are replaced, so nothing leaves the loopback interface.

use std::collections::BTreeMap;
use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

use fritz_core::dns::{
    self, Cause, ClientConfig, Failure, Probes, Record, Report, Request, Resolver, Status, System,
};
use fritz_core::read::Reader;
use fritz_sim::dns::DnsServer;
use fritz_sim::{Answer, Fixture, Sim};

const FIXTURE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../fritz-sim/fixtures/fritzbox-7690/8.25");

/// Real queries to the DNS server, a fixed answer of the resolver of this
/// machine and a fixed configuration.
struct Fake {
    resolved: Result<Vec<IpAddr>, Failure>,
    config: Option<ClientConfig>,
}

impl Probes for Fake {
    async fn query(
        &self,
        server: SocketAddr,
        name: &str,
        kind: Record,
    ) -> Result<Vec<IpAddr>, Failure> {
        System.query(server, name, kind).await
    }

    fn resolve(&self, _: &str) -> impl Future<Output = Result<Vec<IpAddr>, Failure>> + Send {
        std::future::ready(self.resolved.clone())
    }

    fn client_config(&self) -> impl Future<Output = Option<ClientConfig>> + Send {
        std::future::ready(self.config.clone())
    }
}

/// The fixture with the LAN settings of a router at 192.0.2.34 in the
/// domain `domain`.
fn fixture(domain: &str, routers: &str) -> Fixture {
    let out: BTreeMap<String, String> = [
        ("NewDomainName", domain),
        ("NewDNSServers", "192.0.2.34"),
        ("NewIPRouters", routers),
    ]
    .iter()
    .map(|(n, v)| ((*n).to_owned(), (*v).to_owned()))
    .collect();
    Fixture::load(FIXTURE).unwrap().with_answer(
        "LANHostConfigManagement1",
        "GetInfo",
        None,
        Answer::Ok(out),
    )
}

fn search(domains: &[&str]) -> ClientConfig {
    ClientConfig {
        source: "test".to_owned(),
        resolvers: vec![Resolver {
            nameservers: vec!["192.0.2.34".to_owned()],
            search: domains.iter().map(|d| (*d).to_owned()).collect(),
            ..Resolver::default()
        }],
    }
}

async fn diagnose(sim: &Sim, probes: &Fake, request: &Request) -> Report {
    let client = sim.client().build().unwrap();
    let mut reader = Reader::new(client).await.unwrap();
    dns::diagnose(&mut reader, probes, request).await.unwrap()
}

fn request(name: &str, dns: Option<&DnsServer>) -> Request {
    Request {
        name: name.to_owned(),
        expected: None,
        router_dns: dns.map(DnsServer::address),
    }
}

#[tokio::test]
async fn names_a_missing_search_domain() {
    let sim = Sim::start(fixture("fritz.box", "192.0.2.34"))
        .await
        .unwrap();
    let dns = DnsServer::start(&[("device-2.fritz.box", "192.0.2.2")])
        .await
        .unwrap();
    let probes = Fake {
        resolved: Err(Failure::Network("not known".to_owned())),
        config: Some(search(&["example.org"])),
    };
    let report = diagnose(&sim, &probes, &request("device-2", Some(&dns))).await;
    assert_eq!(report.router.domain.as_deref(), Some("fritz.box"));
    assert_eq!(report.router.dns_servers, ["192.0.2.34"]);
    assert_eq!(report.router.address.as_deref(), Some("192.0.2.34"));
    let host = report.router.host.as_ref().unwrap();
    assert_eq!((host.ip.as_deref(), host.active), (Some("192.0.2.2"), true));
    assert_eq!(report.router_answer.name, "device-2.fritz.box");
    assert_eq!(report.router_answer.status, Status::Answered);
    assert_eq!(
        report.router_answer.addresses,
        ["192.0.2.2".parse::<IpAddr>().unwrap()]
    );
    assert_eq!(report.client_answer.status, Status::Failed);
    assert_eq!(report.verdict.cause, Cause::SearchDomainMissing);
    // Only the name with the local domain, once for A and once for AAAA.
    assert_eq!(dns.queries(), ["device-2.fritz.box", "device-2.fritz.box"]);
    // Only getters.
    assert!(
        sim.requests()
            .iter()
            .filter_map(|r| r.action.as_deref())
            .all(|a| a.starts_with("Get") || a.starts_with("X_AVM-DE_Get")),
        "{:?}",
        sim.requests()
    );
}

#[tokio::test]
async fn checks_the_expected_address() {
    let sim = Sim::start(fixture("(none)", "192.0.2.34")).await.unwrap();
    let dns = DnsServer::start(&[("nas.example", "192.0.2.7"), ("nas.example", "2001:db8::7")])
        .await
        .unwrap();
    let probes = Fake {
        resolved: Ok(vec!["192.0.2.7".parse().unwrap()]),
        config: Some(search(&[])),
    };
    let mut request = request("nas.example", Some(&dns));
    request.expected = Some("192.0.2.8".parse().unwrap());
    let report = diagnose(&sim, &probes, &request).await;
    assert_eq!(report.router.domain, None);
    assert_eq!(report.router.host, None);
    assert_eq!(report.router_answer.addresses.len(), 2);
    let expected = report.expected.as_ref().unwrap();
    assert_eq!(
        (expected.router, expected.client),
        (Some(false), Some(false))
    );
    assert_eq!(report.verdict.cause, Cause::UnexpectedAddress);
}

#[tokio::test]
async fn skips_mdns_names() {
    let sim = Sim::start(fixture("fritz.box", "192.0.2.34"))
        .await
        .unwrap();
    let dns = DnsServer::start(&[]).await.unwrap();
    let probes = Fake {
        resolved: Ok(Vec::new()),
        config: None,
    };
    let report = diagnose(&sim, &probes, &request("device-2.local", Some(&dns))).await;
    assert_eq!(report.verdict.cause, Cause::Mdns);
    assert!(report.verdict.message.ends_with("as device-2.fritz.box"));
    assert_eq!(report.router_answer.status, Status::Skipped);
    assert_eq!(report.client_answer.status, Status::Skipped);
    assert_eq!(dns.queries(), Vec::<String>::new());
}

#[tokio::test]
async fn needs_the_address_of_the_router() {
    let sim = Sim::start(fixture("fritz.box", "")).await.unwrap();
    let probes = Fake {
        resolved: Ok(Vec::new()),
        config: None,
    };
    let report = diagnose(&sim, &probes, &request("nas.example", None)).await;
    assert_eq!(report.router.address, None);
    assert_eq!(report.verdict.cause, Cause::RouterUnknown);
}

#[tokio::test]
async fn answers_unknown_names_with_nxdomain() {
    let dns = DnsServer::start(&[("nas.example", "192.0.2.7")])
        .await
        .unwrap();
    let ask = |name, kind| dns::query(dns.address(), name, kind, Duration::from_secs(2));
    assert_eq!(
        ask("other.example", Record::A).await,
        Err(Failure::NoSuchName)
    );
    assert_eq!(
        ask("NAS.example", Record::Aaaa).await,
        Err(Failure::NoRecord)
    );
    assert!(
        DnsServer::start(&[("nas", "not an address")])
            .await
            .is_err()
    );
}

#[tokio::test]
async fn gives_up_on_silent_servers() {
    // A socket that answers only with something that is no answer.
    let silent = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let address = silent.local_addr().unwrap();
    let junk = tokio::spawn(async move {
        let mut buffer = [0; 512];
        let (_, peer) = silent.recv_from(&mut buffer).await.unwrap();
        silent.send_to(b"junk", peer).await.unwrap();
        silent
    });
    let answer = dns::query(
        address,
        "nas.example",
        Record::A,
        Duration::from_millis(200),
    )
    .await;
    assert_eq!(answer, Err(Failure::Timeout));
    // A port nobody listens on, and a name that cannot be asked.
    drop(junk.await.unwrap());
    let answer = dns::query(
        address,
        "nas.example",
        Record::A,
        Duration::from_millis(200),
    )
    .await;
    assert!(
        matches!(answer, Err(Failure::Network(_) | Failure::Timeout)),
        "{answer:?}"
    );
    let invalid = dns::query(address, "a..b", Record::A, Duration::from_millis(50)).await;
    assert_eq!(invalid, Err(Failure::Network("invalid label".to_owned())));
    // Over IPv6 to the loopback interface, which may not exist.
    let v6 = "[::1]:9".parse().unwrap();
    let answer = dns::query(v6, "nas.example", Record::A, Duration::from_millis(50)).await;
    assert!(answer.is_err(), "{answer:?}");
}

#[tokio::test]
async fn ignores_what_is_not_a_query() {
    let dns = DnsServer::start(&[("nas.example", "192.0.2.7")])
        .await
        .unwrap();
    let socket = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    socket.send_to(b"junk", dns.address()).await.unwrap();
    let ask = dns::query(
        dns.address(),
        "nas.example",
        Record::A,
        Duration::from_secs(2),
    );
    assert_eq!(ask.await, Ok(vec!["192.0.2.7".parse().unwrap()]));
    assert_eq!(dns.queries(), ["nas.example"]);
}
