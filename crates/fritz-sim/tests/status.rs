//! The state of the simulated 7690, read through actions that only read.

use fritz_core::read::Reader;
use fritz_core::status::{self, Right};
use fritz_sim::{Fixture, Injection, Sim};
use tr064::SecretString;

const FIXTURE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../fixtures/fritzbox-7690/8.25"
);

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
    let wlans = status::wlans(&mut reader).await.unwrap();
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
async fn reads_hosts_until_the_device_has_no_more() {
    let sim = sim().await;
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
    let calls = called(&sim);
    assert_eq!(calls.len(), 52, "the count and 51 entries: {calls:?}");
}

#[tokio::test]
async fn reads_wireless_networks() {
    let sim = sim().await;
    let wlans = status::wlans(&mut reader(&sim).await).await.unwrap();
    assert_eq!(wlans.len(), 3);
    let first = &wlans[0];
    assert_eq!(first.service, "WLANConfiguration1");
    assert_eq!(first.ssid.as_deref(), Some("wlan-1"));
    assert_eq!(first.band.as_deref(), Some("2.4 GHz"));
    assert_eq!(first.channel, Some(6));
    assert_eq!(first.standard.as_deref(), Some("be"));
    assert_eq!(first.enabled, Some(true));
    assert_eq!(first.status.as_deref(), Some("Up"));
    assert_eq!(first.clients, Some(4));
    assert_eq!(wlans[1].band.as_deref(), Some("5 GHz"));
    assert_eq!(wlans[2].band, None);
    assert_eq!(wlans[2].enabled, Some(false));
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
    assert!(status::wlans(&mut reader).await.is_err());
}
