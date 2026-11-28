//! The wireless networks of the simulated devices and their clients.

use fritz_core::read::Reader;
use fritz_core::wlan::{self, Client, Hint, Night, Security, Statistics, Views, Wlan, Wps};
use fritz_sim::{Answer, Fixture, Injection, Sim};

const FIXTURE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../fritz-sim/fixtures/fritzbox-7690/8.25"
);

const CLIENTS: Views = Views {
    clients: true,
    details: false,
};

const DETAILS: Views = Views {
    clients: false,
    details: true,
};

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

#[tokio::test]
async fn reads_the_details() {
    let (sim, wlans) = read(Fixture::load(FIXTURE).unwrap(), DETAILS).await;
    let details = wlans[0].details.as_ref().unwrap();
    assert_eq!(
        details.security,
        Security {
            beacon_type: Some("11i".to_owned()),
            encryption: Some("WPA2, AES".to_owned()),
            possible_beacon_types: Some(vec![
                "None".to_owned(),
                "11i".to_owned(),
                "11iandWPA3".to_owned()
            ]),
            basic_encryption: Some("None".to_owned()),
            basic_authentication: Some("None".to_owned()),
            hidden: Some(false),
            mac_filter: Some(false),
            user_isolation: None,
        }
    );
    assert_eq!(details.radio.channel, Some(6));
    assert_eq!(details.radio.auto_channel, Some(false));
    assert_eq!(
        details.radio.possible_channels,
        Some((1..=13).collect::<Vec<u32>>())
    );
    assert_eq!(details.radio.bssid.as_deref(), Some("AA:BB:CC:00:00:35"));
    assert_eq!(details.radio.max_bit_rate.as_deref(), Some("Auto"));
    assert_eq!(details.radio.iptv_optimized, Some(true));
    assert_eq!(
        details.wps,
        Wps {
            enabled: None,
            mode: Some("stop".to_owned()),
            status: Some("err_common".to_owned()),
        }
    );
    assert_eq!(
        details.night,
        Night {
            schedule: None,
            no_forced_off: Some(true),
        }
    );
    assert_eq!(
        details.statistics,
        Statistics {
            packets_sent: Some(0),
            packets_received: Some(0),
        }
    );
    assert_eq!(details.hints, []);
    let guest = wlans[2].details.as_ref().unwrap();
    assert_eq!(guest.security.user_isolation, Some(true));
    assert_eq!(guest.radio.channel, Some(6));
    // Keys are never asked for, and the packets come from one getter.
    for action in ["GetSecurityKeys", "X_AVM-DE_GetWLANHybridMode"] {
        assert_eq!(calls(&sim, action), 0, "{action}");
    }
    assert_eq!(calls(&sim, "GetStatistics"), 3);
    assert_eq!(calls(&sim, "GetPacketStatistics"), 0);
    let json = serde_json::to_value(&wlans[0]).unwrap();
    assert_eq!(json["details"]["security"]["encryption"], "WPA2, AES");
    assert_eq!(json["details"]["hints"], serde_json::json!([]));
    let (_, plain) = read(Fixture::load(FIXTURE).unwrap(), Views::default()).await;
    let json = serde_json::to_value(&plain[0]).unwrap();
    assert!(json.get("details").is_none(), "{json}");
}

#[tokio::test]
async fn reads_the_details_without_the_getters() {
    let fixture = Fixture::load(FIXTURE).unwrap();
    let sim = Sim::start(fixture).await.unwrap();
    for action in [
        "GetChannelInfo",
        "GetBeaconAdvertisement",
        "X_AVM-DE_GetWPSInfo",
        "X_AVM-DE_GetNightControl",
        "GetStatistics",
        "GetPacketStatistics",
        "X_AVM-DE_GetIPTVOptimized",
        "X_AVM-DE_GetWLANExtInfo",
    ] {
        sim.inject(Injection::fault("WLANConfiguration1", action, 401));
    }
    sim.inject(Injection::fault("WLANConfiguration2", "GetStatistics", 401));
    let mut reader = Reader::new(sim.client().build().unwrap()).await.unwrap();
    let wlans = wlan::wlans(&mut reader, DETAILS).await.unwrap();
    let details = wlans[0].details.as_ref().unwrap();
    assert_eq!(details.security.encryption.as_deref(), Some("WPA2, AES"));
    assert_eq!(details.security.hidden, None);
    assert_eq!(details.security.user_isolation, None);
    // The channel of GetInfo.
    assert_eq!(details.radio.channel, Some(6));
    assert_eq!(details.radio.auto_channel, None);
    assert_eq!(details.radio.possible_channels, None);
    assert_eq!(details.radio.iptv_optimized, None);
    assert_eq!(details.wps, Wps::default());
    assert_eq!(details.night, Night::default());
    assert_eq!(details.statistics, Statistics::default());
    // GetPacketStatistics instead of GetStatistics, on both networks.
    let other = wlans[1].details.as_ref().unwrap();
    assert_eq!(other.statistics.packets_sent, Some(0));
    assert_eq!(calls(&sim, "GetPacketStatistics"), 2);
}

#[tokio::test]
async fn reads_the_details_of_an_older_repeater() {
    let fixture = Fixture::bundled("fritzwlan-repeater-1750e/7.32").unwrap();
    let (_, wlans) = read(fixture, DETAILS).await;
    let details = wlans[0].details.as_ref().unwrap();
    // GetChannelInfo has no automatic channel on 7.32.
    assert_eq!(details.radio.auto_channel, None);
    assert_eq!(details.radio.channel, Some(6));
    assert_eq!(details.wps.enabled, Some(false));
    assert_eq!(details.statistics.packets_received, Some(38949));
    // The second network has no night time switch-off.
    let second = wlans[1].details.as_ref().unwrap();
    assert_eq!(second.night, Night::default());
}

#[tokio::test]
async fn hints_at_weak_settings() {
    let fixture = Fixture::load(FIXTURE)
        .unwrap()
        .with_answer(
            "WLANConfiguration1",
            "GetInfo",
            None,
            answer(&[
                ("NewBeaconType", "None"),
                ("NewSSID", "open"),
                ("NewChannel", "6"),
            ]),
        )
        .with_answer(
            "WLANConfiguration1",
            "GetBeaconAdvertisement",
            None,
            answer(&[("NewBeaconAdvertisementEnabled", "0")]),
        )
        .with_answer(
            "WLANConfiguration1",
            "X_AVM-DE_GetWPSInfo",
            None,
            answer(&[
                ("NewX_AVM-DE_WPSMode", "stop"),
                ("NewX_AVM-DE_WPSStatus", "inactive"),
            ]),
        )
        .with_answer(
            "WLANConfiguration2",
            "GetInfo",
            None,
            answer(&[("NewBeaconType", "WPAand11i"), ("NewChannel", "48")]),
        );
    let (_, wlans) = read(fixture, DETAILS).await;
    let hints = |i: usize| wlans[i].details.as_ref().unwrap().hints.clone();
    assert_eq!(
        hints(0),
        [Hint::OpenNetwork, Hint::WpsEnabled, Hint::HiddenSsid]
    );
    assert_eq!(hints(1), [Hint::TkipAllowed]);
    assert_eq!(hints(2), []);
    let json = serde_json::to_value(&wlans[0]).unwrap();
    assert_eq!(
        json["details"]["hints"],
        serde_json::json!(["open_network", "wps_enabled", "hidden_ssid"])
    );
}

#[tokio::test]
async fn reads_the_networks_a_name_names() {
    let sim = Sim::start(Fixture::load(FIXTURE).unwrap()).await.unwrap();
    let mut reader = Reader::new(sim.client().build().unwrap()).await.unwrap();
    let services =
        |wlans: Vec<Wlan>| -> Vec<String> { wlans.into_iter().map(|w| w.service).collect() };
    let wlans = wlan::named(&mut reader, "2", DETAILS).await.unwrap();
    assert!(wlans[0].details.is_some());
    assert_eq!(services(wlans), ["WLANConfiguration2"]);
    let wlans = wlan::named(&mut reader, "wlan-1", Views::default())
        .await
        .unwrap();
    assert_eq!(
        services(wlans),
        ["WLANConfiguration1", "WLANConfiguration2"]
    );
    let wlans = wlan::named(&mut reader, "nothing", DETAILS).await.unwrap();
    assert_eq!(wlans, []);
    // Only the named network is read in detail.
    assert_eq!(calls(&sim, "X_AVM-DE_GetWPSInfo"), 1);
}

#[tokio::test]
async fn reads_the_details_of_every_fixture() {
    let views = Views {
        clients: true,
        details: true,
    };
    for name in Fixture::bundled_names() {
        let (_, wlans) = read(Fixture::bundled(&name).unwrap(), views).await;
        for wlan in &wlans {
            let details = wlan.details.as_ref().unwrap();
            assert!(details.security.encryption.is_some(), "{name}");
            assert!(details.radio.bssid.is_some(), "{name}");
            assert!(details.statistics.packets_sent.is_some(), "{name}");
            assert!(wlan.clients.is_some(), "{name}");
        }
    }
}
