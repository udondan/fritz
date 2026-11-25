//! The counters of the LAN interfaces and the wireless networks of the
//! simulated 7690, and how they change between reads.

use fritz_core::interfaces::{self, Counters};
use fritz_core::read::Reader;
use fritz_core::session::Session;
use fritz_sim::{Fixture, Injection, Sim};
use tr064::SecretString;

const FIXTURE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../fritz-sim/fixtures/fritzbox-7690/8.25");

async fn sim() -> Sim {
    Sim::start(Fixture::load(FIXTURE).unwrap()).await.unwrap()
}

async fn reader(sim: &Sim) -> Reader {
    Reader::new(sim.client().build().unwrap()).await.unwrap()
}

#[tokio::test]
async fn reads_the_counters() {
    let sim = sim().await;
    let read = interfaces::interfaces(&mut reader(&sim).await)
        .await
        .unwrap();
    assert_eq!(read.lan.len(), 1);
    let lan = &read.lan[0];
    assert_eq!(lan.service, "LANEthernetInterfaceConfig1");
    assert_eq!(lan.status.as_deref(), Some("Up"));
    assert_eq!(lan.enabled, Some(true));
    assert_eq!(lan.speed.as_deref(), Some("Auto"));
    assert_eq!(lan.duplex.as_deref(), Some("Auto"));
    assert_eq!(
        lan.counters,
        Counters {
            bytes_sent: Some(1_137_729_983_971),
            bytes_received: Some(876_448_304_265),
            packets_sent: Some(1_029_439_075),
            packets_received: Some(709_553_279),
            ..Counters::default()
        }
    );
    assert_eq!(read.wlan.len(), 3);
    let wlan = &read.wlan[0];
    assert_eq!(wlan.ssid.as_deref(), Some("wlan-1"));
    assert_eq!(wlan.band.as_deref(), Some("2.4 GHz"));
    assert_eq!(wlan.channel, Some(6));
    assert_eq!(wlan.counters.packets_sent, Some(0));
    assert_eq!(wlan.counters.bytes_sent, None);
    assert_eq!(read.ports, []);
}

#[tokio::test]
async fn leaves_out_what_cannot_be_read() {
    let sim = sim().await;
    for (service, action) in [
        ("LANEthernetInterfaceConfig1", "GetInfo"),
        ("LANEthernetInterfaceConfig1", "GetStatistics"),
        ("WLANConfiguration1", "GetInfo"),
        ("WLANConfiguration2", "GetStatistics"),
        ("WLANConfiguration2", "GetPacketStatistics"),
    ] {
        sim.inject(Injection::fault(service, action, 606));
    }
    let read = interfaces::interfaces(&mut reader(&sim).await)
        .await
        .unwrap();
    assert_eq!(read.lan, []);
    assert_eq!(read.wlan.len(), 2);
    assert_eq!(read.wlan[0].service, "WLANConfiguration2");
    assert_eq!(read.wlan[0].counters, Counters::default());

    sim.clear_injections();
    sim.inject(Injection::fault(
        "LANEthernetInterfaceConfig1",
        "GetInfo",
        606,
    ));
    let read = interfaces::interfaces(&mut reader(&sim).await)
        .await
        .unwrap();
    assert_eq!(read.lan[0].status, None);
    assert_eq!(read.lan[0].counters.packets_sent, Some(1_029_439_075));
}

#[tokio::test]
async fn computes_rates_from_counters_that_change() {
    let sim = sim().await;
    let lan = "LANEthernetInterfaceConfig1";
    sim.inject(Injection::counter(lan, "Stats.BytesSent", 5000, None));
    sim.inject(Injection::counter(lan, "Stats.PacketsReceived", 10, None));
    // A 32-bit counter that wraps around.
    sim.inject(Injection::counter(
        "WLANConfiguration1",
        "TotalPacketsSent",
        1_500_000_000,
        Some(1 << 32),
    ));
    let mut reader = reader(&sim).await;
    let first = interfaces::interfaces(&mut reader).await.unwrap();
    let second = interfaces::interfaces(&mut reader).await.unwrap();
    let sample = second.since(&first, 2.0);
    let lan = &sample.interfaces[0];
    assert_eq!(lan.changes.bytes_sent, Some(5000));
    assert_eq!(lan.bytes_sent_per_second, Some(2500.0));
    assert_eq!(lan.changes.packets_received, Some(10));
    assert_eq!(lan.changes.bytes_received, Some(0));
    let wlan = &sample.interfaces[1];
    assert_eq!(wlan.ssid.as_deref(), Some("wlan-1"));
    // GetStatistics and GetPacketStatistics both read it, so it grew
    // twice between the answers of GetStatistics.
    assert_eq!(first.wlan[0].counters.packets_sent, Some(1_500_000_000));
    assert_eq!(wlan.changes.packets_sent, Some(3_000_000_000));
    assert!(
        second.wlan[0].counters.packets_sent < first.wlan[0].counters.packets_sent,
        "the counter wrapped around"
    );
}

#[tokio::test]
async fn reads_the_ports() {
    let sim = sim().await;
    let client = tr064::Client::builder(sim.url().clone()).build().unwrap();
    let password = SecretString::from(fritz_sim::PASSWORD);
    let session = Session::login(client, fritz_sim::USERNAME, &password)
        .await
        .unwrap();
    let ports = interfaces::ports(&session).await.unwrap();
    session.logout().await.unwrap();
    assert_eq!(ports.len(), 2);
    assert_eq!(ports[0].name.as_deref(), Some("LAN 1"));
    assert_eq!(ports[0].connected, Some(true));
    assert_eq!(ports[0].speed, Some(1000));
    assert_eq!(ports[0].max_speed, Some(2500));
    assert_eq!(ports[0].energy_saving.as_deref(), Some("on"));
    assert_eq!(ports[1].connected, Some(false));
}
