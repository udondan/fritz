//! The wireless networks of the simulated devices and their clients.

use fritz_core::read::Reader;
use fritz_core::wlan::{self, Client, Views, Wlan};
use fritz_sim::{Answer, Fixture, Injection, Sim};

const FIXTURE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/fritzbox-7690/8.25");

const CLIENTS: Views = Views { clients: true };

async fn read(fixture: Fixture, views: Views) -> (Sim, Vec<Wlan>) {
    let sim = Sim::start(fixture).await.unwrap();
    let mut reader = Reader::new(sim.client().build().unwrap()).await.unwrap();
    let wlans = wlan::wlans(&mut reader, views).await.unwrap();
    (sim, wlans)
}

/// How often the simulator was asked for `action`.
fn calls(sim: &Sim, action: &str) -> usize {
    sim.requests()
        .into_iter()
        .filter(|r| r.authorized && r.action.as_deref() == Some(action))
        .count()
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
async fn reads_wireless_networks() {
    let (sim, wlans) = read(Fixture::load(FIXTURE).unwrap(), Views::default()).await;
    assert_eq!(wlans.len(), 3);
    let first = &wlans[0];
    assert_eq!(first.service, "WLANConfiguration1");
    assert_eq!(first.ssid.as_deref(), Some("wlan-1"));
    assert_eq!(first.band.as_deref(), Some("2.4 GHz"));
    assert_eq!(first.channel, Some(6));
    assert_eq!(first.standard.as_deref(), Some("be"));
    assert_eq!(first.enabled, Some(true));
    assert_eq!(first.status.as_deref(), Some("Up"));
    assert_eq!(first.client_count, Some(4));
    assert_eq!(first.clients, None);
    assert_eq!(wlans[1].band.as_deref(), Some("5 GHz"));
    assert_eq!(wlans[2].band, None);
    assert_eq!(wlans[2].enabled, Some(false));
    let guests: Vec<bool> = wlans.iter().map(|w| w.guest).collect();
    assert_eq!(guests, [false, false, true]);
    assert_eq!(calls(&sim, "X_AVM-DE_GetWLANDeviceListPath"), 0);
    assert_eq!(calls(&sim, "X_AVM-DE_GetHostListPath"), 0);
    let json = serde_json::to_value(&wlans[2]).unwrap();
    assert_eq!(json["guest"], true);
    assert!(json.get("clients").is_none(), "{json}");
}

#[tokio::test]
async fn reads_the_clients_from_the_list() {
    let (sim, wlans) = read(Fixture::load(FIXTURE).unwrap(), CLIENTS).await;
    let counts: Vec<usize> = wlans
        .iter()
        .map(|w| w.clients.as_ref().unwrap().len())
        .collect();
    assert_eq!(counts, [4, 11, 0]);
    assert_eq!(
        wlans[1].clients.as_ref().unwrap()[0],
        Client {
            name: Some("device-14".to_owned()),
            ip: Some("192.0.2.15".to_owned()),
            mac: Some("AA:BB:CC:00:00:10".to_owned()),
            signal: Some(91),
            rate: Some(2401),
        }
    );
    // One list for all networks.
    assert_eq!(calls(&sim, "X_AVM-DE_GetWLANDeviceListPath"), 1);
    assert_eq!(calls(&sim, "GetGenericAssociatedDeviceInfo"), 0);
    let json = serde_json::to_value(&wlans[2]).unwrap();
    assert_eq!(json["clients"], serde_json::json!([]));
}

#[tokio::test]
async fn reads_one_client_per_call_without_lists() {
    let fixture = Fixture::load(FIXTURE).unwrap();
    let sim = Sim::start(fixture).await.unwrap();
    for service in [
        "WLANConfiguration1",
        "WLANConfiguration2",
        "WLANConfiguration3",
    ] {
        sim.inject(Injection::fault(
            service,
            "X_AVM-DE_GetWLANDeviceListPath",
            401,
        ));
    }
    let mut reader = Reader::new(sim.client().build().unwrap()).await.unwrap();
    let wlans = wlan::wlans(&mut reader, CLIENTS).await.unwrap();
    let (_, from_lists) = read(Fixture::load(FIXTURE).unwrap(), CLIENTS).await;
    assert_eq!(wlans, from_lists);
    assert_eq!(calls(&sim, "GetGenericAssociatedDeviceInfo"), 15);
}

#[tokio::test]
async fn reads_the_list_from_another_network() {
    let fixture = Fixture::load(FIXTURE).unwrap();
    let sim = Sim::start(fixture).await.unwrap();
    sim.inject(Injection::fault(
        "WLANConfiguration1",
        "X_AVM-DE_GetWLANDeviceListPath",
        401,
    ));
    let mut reader = Reader::new(sim.client().build().unwrap()).await.unwrap();
    let wlans = wlan::wlans(&mut reader, CLIENTS).await.unwrap();
    let counts: Vec<usize> = wlans
        .iter()
        .map(|w| w.clients.as_ref().unwrap().len())
        .collect();
    assert_eq!(counts, [4, 11, 0]);
    assert_eq!(calls(&sim, "GetGenericAssociatedDeviceInfo"), 0);
}

#[tokio::test]
async fn reads_one_client_per_call_if_the_list_disagrees() {
    let fixture = Fixture::load(FIXTURE).unwrap().with_answer(
        "WLANConfiguration1",
        "GetTotalAssociations",
        None,
        answer(&[("NewTotalAssociations", "2")]),
    );
    let (sim, wlans) = read(fixture, CLIENTS).await;
    let clients = wlans[0].clients.as_ref().unwrap();
    assert_eq!(clients.len(), 2);
    assert_eq!(clients[0].mac.as_deref(), Some("AA:BB:CC:00:00:36"));
    assert_eq!(wlans[1].clients.as_ref().unwrap().len(), 11);
    assert_eq!(calls(&sim, "GetGenericAssociatedDeviceInfo"), 2);
}

#[tokio::test]
async fn finds_the_guest_network_by_its_type() {
    let kind = |kind| answer(&[("NewX_AVM-DE_APType", kind)]);
    let fixture = Fixture::load(FIXTURE)
        .unwrap()
        .with_answer(
            "WLANConfiguration1",
            "X_AVM-DE_GetWLANExtInfo",
            None,
            kind("guest"),
        )
        .with_answer(
            "WLANConfiguration3",
            "X_AVM-DE_GetWLANExtInfo",
            None,
            kind("normal"),
        );
    let (_, wlans) = read(fixture, Views::default()).await;
    let guests: Vec<bool> = wlans.iter().map(|w| w.guest).collect();
    assert_eq!(guests, [true, false, false]);
}

#[tokio::test]
async fn reads_the_clients_of_every_fixture() {
    for name in Fixture::bundled_names() {
        let (_, wlans) = read(Fixture::bundled(&name).unwrap(), CLIENTS).await;
        for wlan in &wlans {
            let clients = wlan.clients.as_ref().unwrap();
            assert_eq!(
                u32::try_from(clients.len()).ok(),
                wlan.client_count,
                "{name}"
            );
            assert!(clients.iter().all(|c| c.mac.is_some()), "{name}");
        }
        assert!(wlans.iter().any(|w| w.guest), "{name}");
    }
}
