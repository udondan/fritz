//! Looking up one host of the simulated 7690.

use fritz_core::host::{self, Attachment, Lookup};
use fritz_core::read::Reader;
use fritz_core::status::Host;
use fritz_sim::{Answer, Fixture, Injection, Sim};

const FIXTURE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/fritzbox-7690/8.25");

async fn reader(sim: &Sim) -> Reader {
    Reader::new(sim.client().build().unwrap()).await.unwrap()
}

async fn sim() -> Sim {
    Sim::start(Fixture::load(FIXTURE).unwrap()).await.unwrap()
}

#[tokio::test]
async fn looks_up_a_host_by_mac_address() {
    let sim = sim().await;
    let mut reader = reader(&sim).await;
    let lookup = Lookup::mac("aa:bb:cc:00:00:03").unwrap();
    let detail = host::lookup(&mut reader, &lookup).await.unwrap().unwrap();
    assert_eq!(detail.host.name.as_deref(), Some("device-2"));
    assert_eq!(detail.host.ip.as_deref(), Some("192.0.2.2"));
    assert!(detail.host.active);
    assert_eq!(detail.host.interface.as_deref(), Some("802.11"));
    assert_eq!(detail.host.address_source.as_deref(), Some("DHCP"));
    assert_eq!(detail.host.guest, Some(false));
    assert_eq!(detail.host.internet_access.as_deref(), Some("granted"));
    assert_eq!(detail.host.speed, Some(866));
    assert_eq!(detail.lease_remaining, None);
    assert_eq!(
        detail.attached_to,
        Some(Attachment {
            node: Some("device-50".to_owned()),
            kind: Some("WLAN".to_owned()),
            interface: Some("AP:5G:0".to_owned()),
            band: Some("5 GHz".to_owned()),
            rate_down: Some(866_700),
            rate_up: Some(433_300),
        })
    );
}

#[tokio::test]
async fn looks_up_a_host_by_ip_address() {
    let sim = sim().await;
    let mut reader = reader(&sim).await;
    let lookup = Lookup::ip("192.0.2.30").unwrap();
    let detail = host::lookup(&mut reader, &lookup).await.unwrap().unwrap();
    assert_eq!(detail.host.name.as_deref(), Some("device-47"));
    assert_eq!(detail.host.mac.as_deref(), Some("AA:BB:CC:00:00:32"));
    let attached = detail.attached_to.unwrap();
    assert_eq!(attached.node.as_deref(), Some("device-51"));
    assert_eq!(attached.kind.as_deref(), Some("LAN"));
    assert_eq!(attached.band, None);

    let unknown = Lookup::ip("192.0.2.200").unwrap();
    assert_eq!(host::lookup(&mut reader, &unknown).await.unwrap(), None);
    let unknown = Lookup::mac("AA:BB:CC:00:00:FF").unwrap();
    assert_eq!(host::lookup(&mut reader, &unknown).await.unwrap(), None);
}

#[tokio::test]
async fn looks_up_hosts_without_the_entry_or_the_host_list() {
    let sim = sim().await;
    let mut reader = reader(&sim).await;
    let lookup = Lookup::mac("AA:BB:CC:00:00:03").unwrap();

    // Without the host list, the entry alone.
    sim.inject(Injection::fault("Hosts1", "X_AVM-DE_GetHostListPath", 606));
    let detail = host::lookup(&mut reader, &lookup).await.unwrap().unwrap();
    assert_eq!(detail.host.name.as_deref(), Some("device-2"));
    assert_eq!(detail.host.mac.as_deref(), Some("AA:BB:CC:00:00:03"));
    assert_eq!(detail.host.guest, None);

    // Without the entry, the host list alone.
    sim.clear_injections();
    sim.inject(Injection::fault("Hosts1", "GetSpecificHostEntry", 606));
    let detail = host::lookup(&mut reader, &lookup).await.unwrap().unwrap();
    assert_eq!(detail.host.name.as_deref(), Some("device-2"));
    assert_eq!(detail.host.address_source, None);
    assert_eq!(detail.host.speed, Some(866));

    // Neither has the host.
    sim.inject(Injection::fault("Hosts1", "X_AVM-DE_GetHostListPath", 606));
    assert_eq!(host::lookup(&mut reader, &lookup).await.unwrap(), None);

    // The device looks up an address without the host list.
    sim.clear_injections();
    sim.inject(Injection::fault("Hosts1", "X_AVM-DE_GetHostListPath", 606));
    let lookup = Lookup::ip("192.0.2.2").unwrap();
    let detail = host::lookup(&mut reader, &lookup).await.unwrap().unwrap();
    assert_eq!(detail.host.ip.as_deref(), Some("192.0.2.2"));
    assert_eq!(detail.host.name.as_deref(), Some("device-2"));
}

#[tokio::test]
async fn fails_when_the_device_cannot_answer() {
    let sim = sim().await;
    let mut reader = reader(&sim).await;
    let lookup = Lookup::mac("AA:BB:CC:00:00:03").unwrap();
    let listed = Host {
        mac: Some("AA:BB:CC:00:00:03".to_owned()),
        ..Host::default()
    };

    sim.inject(Injection::fault("Hosts1", "GetSpecificHostEntry", 868));
    assert!(host::lookup(&mut reader, &lookup).await.is_err());
    assert!(host::detail(&mut reader, listed).await.is_err());

    sim.clear_injections();
    sim.inject(Injection::fault("Hosts1", "X_AVM-DE_GetHostListPath", 868));
    assert!(host::lookup(&mut reader, &lookup).await.is_err());
}

#[tokio::test]
async fn adds_the_details_to_a_host_of_the_list() {
    let entry = Answer::Ok(
        [
            ("NewHostName", "device-2"),
            ("NewActive", "1"),
            ("NewAddressSource", "DHCP"),
            ("NewLeaseTimeRemaining", "3600"),
        ]
        .into_iter()
        .map(|(n, v)| (n.to_owned(), v.to_owned()))
        .collect(),
    );
    let fixture =
        Fixture::load(FIXTURE)
            .unwrap()
            .with_answer("Hosts1", "GetSpecificHostEntry", None, entry);
    let sim = Sim::start(fixture).await.unwrap();
    let mut reader = reader(&sim).await;
    let listed = Host {
        name: Some("device-2".to_owned()),
        mac: Some("AA:BB:CC:00:00:03".to_owned()),
        ..Host::default()
    };
    let detail = host::detail(&mut reader, listed).await.unwrap();
    assert_eq!(detail.lease_remaining, Some(3600));
    assert!(detail.host.active);
    assert_eq!(
        detail.attached_to.unwrap().node.as_deref(),
        Some("device-50")
    );

    // Without a MAC address, nothing is looked up.
    let before = sim.requests().len();
    let detail = host::detail(&mut reader, Host::default()).await.unwrap();
    assert_eq!(detail, host::Detail::default());
    assert_eq!(sim.requests().len(), before);
}
