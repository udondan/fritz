//! The powerline adapters of the simulated devices, read with getters only.

use std::collections::BTreeMap;

use fritz_core::powerline::{self, Adapter, Powerline};
use fritz_core::read::Reader;
use fritz_sim::{Answer, Fixture, Sim};

const FIXTURE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../fritz-sim/fixtures/fritzbox-7690/8.25");
const REPEATER: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../fritz-sim/fixtures/fritz-repeater-2400/8.25"
);
const SERVICE: &str = "X_AVM-DE_Homeplug1";

async fn read(fixture: Fixture) -> (Powerline, Vec<String>) {
    let sim = Sim::start(fixture).await.unwrap();
    let client = sim.client().build().unwrap();
    let mut reader = Reader::new(client).await.unwrap();
    let powerline = powerline::powerline(&mut reader).await.unwrap();
    let called = sim
        .requests()
        .into_iter()
        .filter(|r| r.authorized)
        .filter_map(|r| r.action)
        .collect();
    (powerline, called)
}

fn ok(out: &[(&str, &str)]) -> Answer {
    Answer::Ok(
        out.iter()
            .map(|(n, v)| ((*n).to_owned(), (*v).to_owned()))
            .collect::<BTreeMap<_, _>>(),
    )
}

/// The fixture of the box with two adapters, one of them inactive.
fn with_adapters() -> Fixture {
    Fixture::load(FIXTURE)
        .unwrap()
        .with_answer(
            SERVICE,
            "GetNumberOfDeviceEntries",
            None,
            ok(&[("NewNumberOfEntries", "2")]),
        )
        .with_answer(
            SERVICE,
            "GetGenericDeviceEntry",
            Some(0),
            ok(&[
                ("NewMACAddress", "02:00:00:00:00:01"),
                ("NewActive", "1"),
                ("NewName", "Office"),
                ("NewModel", "FRITZ!Powerline 1260"),
                ("NewUpdateAvailable", "1"),
                ("NewUpdateSuccessful", "succeeded"),
            ]),
        )
        .with_answer(
            SERVICE,
            "GetGenericDeviceEntry",
            Some(1),
            ok(&[
                ("NewMACAddress", "02:00:00:00:00:02"),
                ("NewActive", "0"),
                ("NewName", ""),
                ("NewModel", "FRITZ!Powerline 540E"),
                ("NewUpdateAvailable", "0"),
                ("NewUpdateSuccessful", "unknown"),
            ]),
        )
}

#[tokio::test]
async fn reads_active_and_inactive_adapters() {
    let (powerline, called) = read(with_adapters()).await;
    assert_eq!(
        powerline,
        Powerline {
            supported: true,
            adapters: vec![
                Adapter {
                    name: Some("Office".to_owned()),
                    model: Some("FRITZ!Powerline 1260".to_owned()),
                    mac: Some("02:00:00:00:00:01".to_owned()),
                    active: true,
                    update_available: Some(true),
                    update_result: Some("succeeded".to_owned()),
                },
                Adapter {
                    name: None,
                    model: Some("FRITZ!Powerline 540E".to_owned()),
                    mac: Some("02:00:00:00:00:02".to_owned()),
                    active: false,
                    update_available: Some(false),
                    update_result: None,
                },
            ],
        }
    );
    // Only the getters; never DeviceDoUpdate.
    assert_eq!(
        called,
        [
            "GetNumberOfDeviceEntries",
            "GetGenericDeviceEntry",
            "GetGenericDeviceEntry"
        ]
    );
}

#[tokio::test]
async fn reads_a_box_without_adapters() {
    let (powerline, called) = read(Fixture::load(FIXTURE).unwrap()).await;
    assert_eq!(
        powerline,
        Powerline {
            supported: true,
            adapters: Vec::new(),
        }
    );
    assert_eq!(called, ["GetNumberOfDeviceEntries"]);
}

#[tokio::test]
async fn reports_devices_without_the_service() {
    let (powerline, called) = read(Fixture::load(REPEATER).unwrap()).await;
    assert_eq!(powerline, Powerline::default());
    assert!(!powerline.supported);
    assert!(called.is_empty(), "{called:?}");
}
