//! The state of the simulated 7690, read through actions that only read.

use fritz_core::read::Reader;
use fritz_core::status::{self, Right};
use fritz_core::wlan::{self, Views};
use fritz_sim::{Fixture, Injection, Sim};
use tr064::SecretString;

const FIXTURE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../fritz-sim/fixtures/fritzbox-7690/8.25");

async fn sim() -> Sim {
    Sim::start(Fixture::load(FIXTURE).unwrap()).await.unwrap()
}

async fn reader(sim: &Sim) -> Reader {
    Reader::new(sim.client().build().unwrap()).await.unwrap()
}

/// The actions the simulator was asked for, in order.
fn called(sim: &Sim) -> Vec<String> {
    sim.requests()
        .into_iter()
        .filter(|r| r.authorized)
        .filter_map(|r| r.action)
        .collect()
}

#[tokio::test]
async fn reads_the_overview() {
    let sim = sim().await;
    let overview = status::overview(&mut reader(&sim).await).await.unwrap();
    assert_eq!(overview.model.as_deref(), Some("FRITZ!Box 7690"));
    assert_eq!(overview.firmware.as_deref(), Some("285.08.25"));
    assert_eq!(overview.uptime, Some(760_415));
    assert_eq!(overview.update_available, Some(false));
    assert_eq!(overview.two_factor, Some(false));
    assert_eq!(overview.rights.len(), 6);
    assert_eq!(
        overview.rights[0],
        Right {
            area: "BoxAdmin".to_owned(),
            access: "readwrite".to_owned()
        }
    );
    let internet = overview.internet.unwrap();
    assert_eq!(internet.access_type.as_deref(), Some("DSL"));
    assert_eq!(internet.link.as_deref(), Some("Up"));
    assert_eq!(internet.status.as_deref(), Some("Connected"));
    assert_eq!(internet.external_address.as_deref(), Some("2001:db8::1"));
    assert_eq!(internet.uptime, Some(34_585));
    // The connection is over PPP, so WANIPConnection is not asked.
    assert!(!sim.requests().iter().any(|r| r.path.contains("wanipconn")));
}

#[tokio::test]
async fn leaves_out_what_cannot_be_read() {
    let sim = sim().await;
    for (service, action) in [
        ("DeviceInfo1", "GetInfo"),
        ("UserInterface1", "GetInfo"),
        ("LANConfigSecurity1", "X_AVM-DE_GetCurrentUser"),
        ("WANCommonInterfaceConfig1", "GetCommonLinkProperties"),
        ("WANPPPConnection1", "GetInfo"),
        ("Hosts1", "GetHostNumberOfEntries"),
        ("Hosts1", "X_AVM-DE_GetHostListPath"),
        ("Hosts1", "X_AVM-DE_GetMeshListPath"),
        ("WLANConfiguration1", "GetInfo"),
    ] {
        sim.inject(Injection::fault(service, action, 606));
    }
    let mut reader = reader(&sim).await;
    let overview = status::overview(&mut reader).await.unwrap();
    assert_eq!(overview.model, None);
    assert_eq!(overview.update_available, None);
    assert_eq!(overview.two_factor, Some(false));
    assert_eq!(overview.rights, []);
    // WANIPConnection1 has no GetInfo answer in the fixture either.
    assert_eq!(overview.internet, None);

    let internet = status::internet(&mut reader).await.unwrap();
    assert_eq!(internet.connection.access_type, None);
    assert_eq!(internet.dsl, None, "only for DSL connections");
    assert_eq!(internet.provider.as_deref(), Some("provider-1"));

    let hosts = status::hosts(&mut reader).await.unwrap();
    assert_eq!((hosts.reported, hosts.hosts.len()), (0, 0));
    assert_eq!(status::mesh(&mut reader).await.unwrap(), None);
    let wlans = wlan::wlans(&mut reader, Views::default()).await.unwrap();
    let services: Vec<&str> = wlans.iter().map(|w| w.service.as_str()).collect();
    assert_eq!(services, ["WLANConfiguration2", "WLANConfiguration3"]);
}

#[tokio::test]
async fn reads_the_internet_connection() {
    let sim = sim().await;
    let internet = status::internet(&mut reader(&sim).await).await.unwrap();
    assert_eq!(internet.provider.as_deref(), Some("provider-1"));
    assert_eq!(internet.dns_servers, ["2001:db8::2", "2001:db8::3"]);
    assert_eq!(internet.downstream_max, Some(276_310));
    assert_eq!(internet.upstream_max, Some(45_853));
    let dsl = internet.dsl.unwrap();
    assert_eq!(dsl.status.as_deref(), Some("Up"));
    assert_eq!(dsl.downstream_rate, Some(274_793));
    assert_eq!(dsl.upstream_rate, Some(45_853));
    assert_eq!(dsl.downstream_max, Some(276_310));
    assert_eq!(dsl.upstream_max, Some(45_853));
    assert_eq!(dsl.downstream_noise_margin, Some(7.0));
    assert_eq!(dsl.upstream_noise_margin, Some(8.0));
    assert_eq!(dsl.downstream_attenuation, Some(14.0));
    assert_eq!(dsl.upstream_attenuation, Some(10.0));
}

#[tokio::test]
async fn reads_the_host_list() {
    let sim = sim().await;
    let hosts = status::hosts(&mut reader(&sim).await).await.unwrap();
    assert_eq!(hosts.reported, 50);
    assert_eq!(hosts.hosts.len(), 50);
    let second = &hosts.hosts[1];
    assert_eq!(second.name.as_deref(), Some("device-2"));
    assert_eq!(second.ip.as_deref(), Some("192.0.2.2"));
    assert_eq!(second.mac.as_deref(), Some("AA:BB:CC:00:00:03"));
    assert!(second.active);
    assert_eq!(second.interface.as_deref(), Some("802.11"));
    assert_eq!(second.address_source, None, "not in the host list");
    assert_eq!(second.guest, Some(false));
    assert_eq!(second.internet_access.as_deref(), Some("granted"));
    assert_eq!(second.speed, Some(866));
    assert_eq!(called(&sim), ["X_AVM-DE_GetHostListPath"]);
    let fetched: Vec<String> = sim
        .requests()
        .into_iter()
        .filter(|r| r.path.contains(".lua"))
        .map(|r| r.path)
        .collect();
    assert_eq!(fetched, ["/devicehostlist.lua"]);
}

#[tokio::test]
async fn reads_hosts_one_by_one_without_a_host_list() {
    let sim = sim().await;
    sim.inject(Injection::fault("Hosts1", "X_AVM-DE_GetHostListPath", 606));
    let hosts = status::hosts(&mut reader(&sim).await).await.unwrap();
    // The fixture recorded 50 of the 161 entries.
    assert_eq!(hosts.reported, 161);
    assert_eq!(hosts.hosts.len(), 50);
    let first = &hosts.hosts[0];
    assert_eq!(first.name.as_deref(), Some("device-1"));
    assert_eq!(first.ip.as_deref(), Some("192.0.2.1"));
    assert_eq!(first.mac.as_deref(), Some("AA:BB:CC:00:00:02"));
    assert!(!first.active);
    assert_eq!(first.interface, None);
    assert_eq!(first.address_source.as_deref(), Some("DHCP"));
    assert_eq!(first.guest, None, "only in the host list");
    let calls = called(&sim);
    assert_eq!(
        calls.len(),
        53,
        "the list, the count and 51 entries: {calls:?}"
    );
}

#[tokio::test]
async fn reads_the_mesh() {
    let sim = sim().await;
    let mesh = status::mesh(&mut reader(&sim).await)
        .await
        .unwrap()
        .unwrap();
    let meshed: Vec<_> = mesh
        .nodes
        .iter()
        .filter(|n| n.meshed)
        .map(|n| (n.name.as_deref(), n.role.as_deref()))
        .collect();
    assert_eq!(
        meshed,
        [
            (Some("device-50"), Some("master")),
            (Some("device-51"), Some("slave"))
        ]
    );
    assert_eq!(mesh.nodes.len(), 4);
    let links: Vec<_> = mesh
        .links
        .iter()
        .map(|l| (l.from.as_str(), l.to.as_str(), l.interface.as_deref()))
        .collect();
    assert_eq!(
        links,
        [
            ("device-50", "device-51", Some("AP:5G:0")),
            ("device-50", "device-2", Some("AP:5G:0")),
            ("device-51", "device-47", Some("LAN:1")),
        ]
    );
    assert_eq!(mesh.links[1].rate_tx, Some(866_700));
}

#[tokio::test]
async fn adds_the_addresses_to_the_mesh() {
    let dir = tempfile::tempdir().unwrap();
    copy(FIXTURE.as_ref(), dir.path());
    // The repeater is a host with the MAC of its uplink, in lower case,
    // device-2 is not active, and device-47 has no host entry.
    let path = dir.path().join("lists/devicehostlist.xml");
    let hosts = std::fs::read_to_string(&path)
        .unwrap()
        .replace(
            "<MACAddress>AA:BB:CC:00:00:03</MACAddress>\n<Active>1</Active>",
            "<MACAddress>AA:BB:CC:00:00:03</MACAddress>\n<Active>0</Active>",
        )
        .replace(
            "<MACAddress>AA:BB:CC:00:00:32</MACAddress>",
            "<MACAddress>AA:BB:CC:00:00:99</MACAddress>",
        )
        .replace(
            "</List>",
            "<Item><IPAddress>192.0.2.40</IPAddress>\
             <MACAddress>aa:bb:cc:00:00:53</MACAddress><Active>1</Active>\
             <HostName>device-51</HostName></Item></List>",
        );
    std::fs::write(&path, hosts).unwrap();
    let sim = Sim::start(Fixture::load(dir.path()).unwrap())
        .await
        .unwrap();
    let mut reader = reader(&sim).await;
    let mut mesh = status::mesh(&mut reader).await.unwrap().unwrap();
    assert_eq!(status::add_addresses(&mut reader, &mut mesh).await, None);
    let addresses: Vec<_> = mesh
        .nodes
        .iter()
        .map(|n| (n.name.as_deref().unwrap(), n.ip.as_deref(), n.active))
        .collect();
    assert_eq!(
        addresses,
        [
            ("device-50", Some("192.0.2.34"), None),
            ("device-51", Some("192.0.2.40"), Some(true)),
            ("device-2", Some("192.0.2.2"), Some(false)),
            ("device-47", None, None),
        ]
    );
    // One read of the host list and of the LAN settings.
    let calls = called(&sim);
    assert_eq!(
        calls[calls.len() - 2..],
        ["X_AVM-DE_GetHostListPath", "GetIPRoutersList"]
    );
}

#[tokio::test]
async fn rejects_lists_that_are_not_valid() {
    let dir = tempfile::tempdir().unwrap();
    copy(FIXTURE.as_ref(), dir.path());
    std::fs::write(dir.path().join("lists/devicehostlist.xml"), "<html/>").unwrap();
    std::fs::write(dir.path().join("lists/meshlist.json"), "[]").unwrap();
    let sim = Sim::start(Fixture::load(dir.path()).unwrap())
        .await
        .unwrap();
    let mut reader = reader(&sim).await;
    let error = status::hosts(&mut reader).await.unwrap_err();
    assert!(error.to_string().contains("host list"), "{error}");
    let error = status::mesh(&mut reader).await.unwrap_err();
    assert!(error.to_string().contains("mesh list"), "{error}");
}

#[tokio::test]
async fn has_no_mesh_without_a_path() {
    let dir = tempfile::tempdir().unwrap();
    copy(FIXTURE.as_ref(), dir.path());
    std::fs::write(
        dir.path()
            .join("responses/Hosts1/X_AVM-DE_GetMeshListPath.json"),
        r#"{"out": {"NewX_AVM-DE_MeshListPath": ""}, "status": "ok"}"#,
    )
    .unwrap();
    let sim = Sim::start(Fixture::load(dir.path()).unwrap())
        .await
        .unwrap();
    let mesh = status::mesh(&mut reader(&sim).await).await.unwrap();
    assert_eq!(mesh, None);
}

fn copy(from: &std::path::Path, to: &std::path::Path) {
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            std::fs::create_dir(&target).unwrap();
            copy(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

#[tokio::test]
async fn fails_without_a_login() {
    let sim = sim().await;
    let client = sim
        .client()
        .credentials(fritz_sim::USERNAME, SecretString::from("wrong"))
        .build()
        .unwrap();
    let mut reader = Reader::new(client).await.unwrap();
    assert!(status::overview(&mut reader).await.is_err());
    assert!(status::internet(&mut reader).await.is_err());
    assert!(status::hosts(&mut reader).await.is_err());
    assert!(status::mesh(&mut reader).await.is_err());
    assert!(wlan::wlans(&mut reader, Views::default()).await.is_err());
}
