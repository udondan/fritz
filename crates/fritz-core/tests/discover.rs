//! Finding devices, with the simulator on loopback and answers given in
//! advance. No multicast: the SSDP search goes to a socket on loopback.

use std::net::{IpAddr, Ipv4Addr};
use std::time::Duration;

use axum::Router;
use axum::routing::get;
use fritz_core::discover::{
    Discovery, Fixed, Gateway as _, Multicast, Options, Resolver as _, Role, SEARCH_TARGETS,
    Source, Ssdp as _, System,
};
use fritz_sim::{Fixture, Injection, Sim};
use tokio::net::UdpSocket;

const LOOPBACK: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);

/// A description of a device of another manufacturer.
const OTHER: &str = r#"<?xml version="1.0"?>
<root xmlns="urn:dslforum-org:device-1-0">
<device>
<deviceType>urn:dslforum-org:device:InternetGatewayDevice:1</deviceType>
<manufacturer>Other</manufacturer>
<modelName>Router</modelName>
</device>
</root>"#;

/// A FRITZ! description of another device type.
const MEDIA: &str = r#"<?xml version="1.0"?>
<root xmlns="urn:dslforum-org:device-1-0">
<device>
<deviceType>urn:schemas-upnp-org:device:MediaServer:1</deviceType>
<manufacturer>AVM</manufacturer>
<modelName>FRITZ!Box 7690</modelName>
</device>
</root>"#;

async fn sim(fixture: &str) -> Sim {
    Sim::start(Fixture::bundled(fixture).unwrap())
        .await
        .unwrap()
}

/// A server with the description at `/tr64desc.xml`.
async fn serve(description: &'static str) -> String {
    let app = Router::new().route("/tr64desc.xml", get(move || async move { description }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    url
}

fn location(url: &str) -> String {
    format!("{}/tr64desc.xml", url.trim_end_matches('/'))
}

fn options(port: u16) -> Options {
    Options {
        port,
        defaults: vec![LOOPBACK],
        search_time: Duration::from_millis(100),
        timeout: Duration::from_millis(500),
    }
}

#[tokio::test]
async fn lists_verified_devices_once() {
    let router = sim("fritzbox-7690/8.25").await;
    let same = sim("fritzbox-7690/8.25").await;
    let repeater = sim("fritz-repeater-2400/8.25").await;
    let old = sim("fritzwlan-repeater-1750e/7.32").await;
    let slow = sim("fritz-repeater-2400/8.25").await;
    slow.inject(Injection::Delay(Duration::from_secs(5)));
    let other = serve(OTHER).await;
    let media = serve(MEDIA).await;
    let port = router.url().port().unwrap();
    let discovery = Discovery {
        network: Fixed {
            addresses: vec![LOOPBACK],
            gateway: Some(LOOPBACK),
            locations: vec![
                location(router.url().as_str()),
                location(same.url().as_str()),
                location(repeater.url().as_str()),
                location(old.url().as_str()),
                location(slow.url().as_str()),
                location(&other),
                location(&media),
                "http://203.0.113.1:49000/tr64desc.xml".to_owned(),
                "garbage".to_owned(),
            ],
        },
        ssdp: Fixed::default(),
        options: options(port),
    };
    // The network gives the SSDP answers here, the search none.
    let discovery = Discovery {
        ssdp: discovery.network.clone(),
        ..discovery
    };
    let report = discovery.run("fritz.box", None).await;

    assert_eq!(report.host, "fritz.box");
    assert_eq!(report.addresses, [LOOPBACK]);
    assert!(report.local);
    assert_eq!(report.gateway, Some(LOOPBACK));
    let mut found: Vec<_> = report
        .devices
        .iter()
        .map(|d| {
            (
                d.model.as_deref().unwrap(),
                d.role,
                d.firmware.as_deref(),
                d.found_by.clone(),
            )
        })
        .collect();
    // Devices of the same role and source are ordered by address, whose
    // ports differ from run to run.
    found.sort_by_key(|device| device.0);
    assert_eq!(
        found,
        [
            (
                "FRITZ!Box 7690",
                Role::Router,
                Some("8.25"),
                vec![
                    Source::Config,
                    Source::Gateway,
                    Source::Ssdp,
                    Source::Default
                ]
            ),
            (
                "FRITZ!Repeater 2400",
                Role::Repeater,
                None,
                vec![Source::Ssdp]
            ),
            (
                "FRITZ!WLAN Repeater 1750E",
                Role::Repeater,
                None,
                vec![Source::Ssdp]
            ),
        ]
    );
    let box_ = &report.devices[0];
    let repeater = report
        .devices
        .iter()
        .find(|d| d.model.as_deref() == Some("FRITZ!Repeater 2400"))
        .unwrap();
    let address = format!("127.0.0.1:{port}");
    assert_eq!(box_.address, address);
    assert_eq!(box_.host, format!("http://{address}"));
    assert_eq!(box_.manufacturer.as_deref(), Some("FRITZ! GmbH"));

    let json = serde_json::to_value(repeater).unwrap();
    assert_eq!(json["role"], "repeater");
    assert_eq!(json["found_by"], serde_json::json!(["ssdp"]));
    assert!(json.get("udn").is_none());
}

#[tokio::test]
async fn reports_a_host_outside_the_local_network() {
    let discovery = Discovery {
        network: Fixed {
            addresses: vec!["203.0.113.1".parse().unwrap()],
            ..Fixed::default()
        },
        ssdp: Fixed::default(),
        options: Options {
            defaults: Vec::new(),
            ..options(49000)
        },
    };
    let report = discovery.run("fritz.box", None).await;
    assert!(!report.local);
    assert_eq!(report.devices, []);

    let discovery = Discovery {
        network: Fixed::default(),
        ..discovery
    };
    let report = discovery.run("fritz.box", None).await;
    assert_eq!(report.addresses, [] as [IpAddr; 0]);
    assert!(!report.local);
}

#[tokio::test]
async fn searches_over_udp_and_collects_the_locations() {
    let responder = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let ssdp = Multicast {
        target: responder.local_addr().unwrap(),
    };
    let answers = tokio::spawn(async move {
        let mut requests = Vec::new();
        let mut buffer = [0; 2048];
        for _ in SEARCH_TARGETS {
            let (length, from) = responder.recv_from(&mut buffer).await.unwrap();
            requests.push(String::from_utf8_lossy(&buffer[..length]).into_owned());
            for answer in [
                "HTTP/1.1 200 OK\r\nLOCATION: http://192.168.178.1:49000/tr64desc.xml\r\n\r\n",
                "HTTP/1.1 200 OK\r\nCACHE-CONTROL: max-age=1800\r\n\r\n",
            ] {
                responder.send_to(answer.as_bytes(), from).await.unwrap();
            }
        }
        requests
    });
    let locations = ssdp
        .search(&SEARCH_TARGETS, Duration::from_millis(500))
        .await;
    assert_eq!(locations, ["http://192.168.178.1:49000/tr64desc.xml"]);
    let requests = answers.await.unwrap();
    assert!(requests[0].starts_with("M-SEARCH * HTTP/1.1\r\n"));
    assert!(requests[0].contains("MAN: \"ssdp:discover\"\r\n"));
    assert!(requests[0].contains(&format!("ST: {}\r\n", SEARCH_TARGETS[0])));
    assert!(requests[1].contains(&format!("ST: {}\r\n", SEARCH_TARGETS[1])));
}

#[tokio::test]
async fn gives_up_on_a_search_that_cannot_be_sent() {
    // Port 0 is no destination.
    let ssdp = Multicast {
        target: (LOOPBACK, 0).into(),
    };
    let locations = ssdp
        .search(&SEARCH_TARGETS, Duration::from_millis(50))
        .await;
    assert_eq!(locations, [] as [String; 0]);
}

#[tokio::test]
async fn asks_the_system() {
    let addresses = System.resolve("localhost").await;
    assert!(addresses.iter().all(IpAddr::is_loopback), "{addresses:?}");
    assert_eq!(
        System.resolve("[::1]").await,
        ["::1".parse::<IpAddr>().unwrap()]
    );
    // Reads the routing table, which differs from machine to machine.
    let _ = System.gateway().await;
}

/// A name resolution that never answers, and no gateway.
#[derive(Debug, Clone)]
struct Hanging;

impl fritz_core::discover::Resolver for Hanging {
    fn resolve(&self, _host: &str) -> impl Future<Output = Vec<IpAddr>> + Send {
        std::future::pending()
    }
}

impl fritz_core::discover::Gateway for Hanging {
    fn gateway(&self) -> impl Future<Output = Option<IpAddr>> + Send {
        std::future::ready(None)
    }
}

#[tokio::test]
async fn goes_on_when_the_name_resolution_hangs() {
    let router = sim("fritzbox-7690/8.25").await;
    let discovery = Discovery {
        network: Hanging,
        ssdp: Fixed::default(),
        options: options(router.url().port().unwrap()),
    };
    let report = tokio::time::timeout(Duration::from_secs(5), discovery.run("fritz.box", None))
        .await
        .expect("the resolution is bounded");
    assert_eq!(report.addresses, [] as [IpAddr; 0]);
    assert_eq!(report.devices.len(), 1);
    assert_eq!(report.devices[0].found_by, [Source::Default]);
}

#[tokio::test]
async fn asks_the_configured_host_at_its_port() {
    let router = sim("fritzbox-7690/8.25").await;
    let discovery = Discovery {
        network: Fixed {
            addresses: vec![LOOPBACK],
            ..Fixed::default()
        },
        ssdp: Fixed::default(),
        options: Options {
            defaults: Vec::new(),
            ..options(1)
        },
    };
    let report = discovery.run("fritz.box", router.url().port()).await;
    assert_eq!(report.devices.len(), 1);
    assert_eq!(report.devices[0].found_by, [Source::Config]);
    assert_eq!(discovery.run("fritz.box", None).await.devices, []);
}
