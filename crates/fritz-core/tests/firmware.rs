//! The firmware state of the simulated devices, read with getters only.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use fritz_core::firmware::{self, Firmware, State};
use fritz_core::read::Reader;
use fritz_sim::{Answer, Fixture, Injection, Sim};

const FIXTURE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../fritz-sim/fixtures/fritzbox-7690/8.25"
);
const REPEATER: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../fritz-sim/fixtures/fritz-repeater-2400/8.25"
);

/// A download URL with a token, which must never be shown.
const DOWNLOAD: &str = "https://download.example.com/fw.image?token=f4ke70ken";

async fn read(fixture: Fixture) -> (Firmware, Vec<String>) {
    let sim = Sim::start(fixture).await.unwrap();
    let client = sim.client().build().unwrap();
    let mut reader = Reader::new(client).await.unwrap();
    let firmware = firmware::firmware(&mut reader).await.unwrap();
    let called = sim
        .requests()
        .into_iter()
        .filter(|r| r.authorized)
        .filter_map(|r| r.action)
        .collect();
    (firmware, called)
}

fn ok(out: &[(&str, &str)]) -> Answer {
    Answer::Ok(
        out.iter()
            .map(|(n, v)| ((*n).to_owned(), (*v).to_owned()))
            .collect::<BTreeMap<_, _>>(),
    )
}

#[tokio::test]
async fn reads_a_device_without_an_update() {
    let (firmware, called) = read(Fixture::load(FIXTURE).unwrap()).await;
    assert_eq!(
        firmware,
        Firmware {
            state: State::NoUpdate,
            installed: Some("285.08.25".to_owned()),
            build_type: Some("Release".to_owned()),
            offered: None,
            info_url: None,
            update_state: Some("Stopped".to_owned()),
            auto_update: Some("all".to_owned()),
            last_update_time: Some("2026-08-02T03:28:11+02:00".to_owned()),
            last_update_result: Some("succeeded".to_owned()),
            previous_version: Some("285.08.22".to_owned()),
        }
    );
    // Only the getters; never X_AVM-DE_CheckUpdate.
    assert_eq!(called, ["GetInfo", "GetInfo", "X_AVM-DE_GetInfo"]);
}

#[tokio::test]
async fn reads_an_offered_update() {
    let fixture = Fixture::load(FIXTURE).unwrap().with_answer(
        "UserInterface1",
        "GetInfo",
        None,
        ok(&[
            ("NewUpgradeAvailable", "1"),
            ("NewX_AVM-DE_BuildType", "Release"),
            ("NewX_AVM-DE_DownloadURL", DOWNLOAD),
            ("NewX_AVM-DE_InfoURL", "https://example.com/info.txt"),
            ("NewX_AVM-DE_UpdateState", "UpdateAvailable"),
            ("NewX_AVM-DE_Version", "285.08.30"),
        ]),
    );
    let (firmware, _) = read(fixture).await;
    assert_eq!(firmware.state, State::Available);
    assert_eq!(firmware.offered.as_deref(), Some("285.08.30"));
    assert_eq!(
        firmware.info_url.as_deref(),
        Some("https://example.com/info.txt")
    );
    let json = serde_json::to_string(&firmware).unwrap();
    assert!(!json.contains("f4ke70ken"), "{json}");
}

#[tokio::test]
async fn reads_a_repeater_without_the_extended_getter() {
    let fixture = Fixture::load(REPEATER).unwrap().with_answer(
        "UserInterface1",
        "X_AVM-DE_GetInfo",
        None,
        Answer::Fault {
            code: 401,
            description: "Invalid Action".to_owned(),
        },
    );
    let (firmware, _) = read(fixture).await;
    assert_eq!(
        firmware,
        Firmware {
            state: State::NoUpdate,
            installed: Some("169.08.25".to_owned()),
            build_type: Some("Release".to_owned()),
            update_state: Some("Stopped".to_owned()),
            ..Firmware::default()
        }
    );
}

#[tokio::test]
async fn reports_unknown_without_the_rights() {
    let sim = Sim::start(Fixture::load(FIXTURE).unwrap()).await.unwrap();
    sim.inject(Injection::fault("UserInterface1", "GetInfo", 606));
    let client = sim.client().build().unwrap();
    let mut reader = Reader::new(client).await.unwrap();
    let firmware = firmware::firmware(&mut reader).await.unwrap();
    assert_eq!(firmware.state, State::Unknown);
    assert_eq!(firmware.installed.as_deref(), Some("285.08.25"));
    assert_eq!(firmware.offered, None);
    // The extended getter still answers.
    assert_eq!(firmware.auto_update.as_deref(), Some("all"));
}

#[tokio::test]
async fn reports_devices_without_the_service_as_unsupported() {
    // The repeater without the service UserInterface.
    let dir = tempfile::tempdir().unwrap();
    let repeater = Path::new(REPEATER);
    let description = fs::read_to_string(repeater.join("tr64desc.xml")).unwrap();
    let start =
        description.find("<service>\n<serviceType>urn:dslforum-org:service:UserInterface:1");
    let start = start.unwrap();
    let end = start + description[start..].find("</service>\n").unwrap() + "</service>\n".len();
    let description = format!("{}{}", &description[..start], &description[end..]);
    fs::write(dir.path().join("tr64desc.xml"), description).unwrap();
    for sub in ["scpd", "responses/DeviceInfo1"] {
        fs::create_dir_all(dir.path().join(sub)).unwrap();
    }
    for file in [
        "scpd/deviceinfoSCPD.xml",
        "responses/DeviceInfo1/GetInfo.json",
    ] {
        fs::copy(repeater.join(file), dir.path().join(file)).unwrap();
    }
    let (firmware, called) = read(Fixture::load(dir.path()).unwrap()).await;
    assert_eq!(
        firmware,
        Firmware {
            state: State::Unsupported,
            installed: Some("169.08.25".to_owned()),
            ..Firmware::default()
        }
    );
    assert_eq!(called, ["GetInfo"]);
}
