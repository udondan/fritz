//! What a device needs to join a wireless network, and the narrow path to
//! its passphrase. The passphrases of these networks are fake and exist
//! only in the tests, never in the fixtures.

use std::time::Duration;

use fritz_core::capture::{self, Decision, Options};
use fritz_core::generic;
use fritz_core::read::Reader;
use fritz_core::wlan::join::{self, Auth, Error};
use fritz_sim::{Answer, Fixture, Injection, Sim};
use secrecy::ExposeSecret as _;

const FIXTURE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../fritz-sim/fixtures/fritzbox-7690/8.25"
);

/// A fake passphrase with every character that needs escaping.
const SPECIAL: &str = r#"fake\pass;phrase,with:"quotes""#;

fn answer(pairs: &[(&str, &str)]) -> Answer {
    Answer::Ok(
        pairs
            .iter()
            .map(|(n, v)| ((*n).to_owned(), (*v).to_owned()))
            .collect(),
    )
}

/// A network of the first service, with its SSID, beacon type, whether its
/// SSID is hidden, and its passphrase.
fn network(ssid: &str, beacon_type: &str, hidden: bool, passphrase: &str) -> Fixture {
    let advertised = if hidden { "0" } else { "1" };
    Fixture::load(FIXTURE)
        .unwrap()
        .with_answer(
            "WLANConfiguration1",
            "GetInfo",
            None,
            answer(&[
                ("NewSSID", ssid),
                ("NewBeaconType", beacon_type),
                ("NewBasicEncryptionModes", "None"),
                ("NewEnable", "1"),
                ("NewChannel", "6"),
            ]),
        )
        .with_answer(
            "WLANConfiguration1",
            "GetBeaconAdvertisement",
            None,
            answer(&[("NewBeaconAdvertisementEnabled", advertised)]),
        )
        .with_answer(
            "WLANConfiguration1",
            "GetSecurityKeys",
            None,
            keys(passphrase),
        )
}

/// The answer of `GetSecurityKeys`, with fake keys besides the passphrase.
fn keys(passphrase: &str) -> Answer {
    answer(&[
        ("NewWEPKey0", "fake-wep-key-0"),
        ("NewPreSharedKey", "fake-pre-shared-key"),
        ("NewKeyPassphrase", passphrase),
    ])
}

/// How often the simulator was asked for `action`.
fn calls(sim: &Sim, action: &str) -> usize {
    sim.requests()
        .into_iter()
        .filter(|r| r.action.as_deref() == Some(action))
        .count()
}

async fn reader(sim: &Sim) -> Reader {
    Reader::new(sim.client().build().unwrap()).await.unwrap()
}

#[tokio::test]
async fn encodes_every_kind_of_network() {
    for (ssid, beacon_type, hidden, passphrase, payload) in [
        (
            "wlan-wpa2",
            "11i",
            false,
            "fake-wpa2-passphrase",
            "WIFI:T:WPA;S:wlan-wpa2;P:fake-wpa2-passphrase;;",
        ),
        (
            "wlan-wpa3",
            "WPA3",
            false,
            "fake-wpa3-passphrase",
            "WIFI:T:WPA;S:wlan-wpa3;P:fake-wpa3-passphrase;;",
        ),
        (
            "wlan-mixed",
            "11iandWPA3",
            false,
            "fake-mixed-passphrase",
            "WIFI:T:WPA;S:wlan-mixed;P:fake-mixed-passphrase;;",
        ),
        (
            "wlan-tkip",
            "WPAand11i",
            false,
            "fake-tkip-passphrase",
            "WIFI:T:WPA;S:wlan-tkip;P:fake-tkip-passphrase;;",
        ),
        (
            "wlan-open",
            "None",
            false,
            "",
            "WIFI:T:nopass;S:wlan-open;;",
        ),
        ("wlan-owe", "OWE", false, "", "WIFI:T:nopass;S:wlan-owe;;"),
        (
            "wlan-owe-transition",
            "OWETrans",
            false,
            "",
            "WIFI:T:nopass;S:wlan-owe-transition;;",
        ),
        (
            "wlan-hidden",
            "11i",
            true,
            "fake-hidden-passphrase",
            "WIFI:T:WPA;S:wlan-hidden;P:fake-hidden-passphrase;H:true;;",
        ),
        // Verbatim, though the read path hides session IDs in values.
        (
            "Home?sid=123&sid=456",
            "11i",
            false,
            "fake-sid-passphrase",
            "WIFI:T:WPA;S:Home?sid=123&sid=456;P:fake-sid-passphrase;;",
        ),
        (
            r#"Café;"Gäste",x:y\z"#,
            "11iandWPA3",
            false,
            SPECIAL,
            r#"WIFI:T:WPA;S:Café\;\"Gäste\"\,x\:y\\z;P:fake\\pass\;phrase\,with\:\"quotes\";;"#,
        ),
    ] {
        let sim = Sim::start(network(ssid, beacon_type, hidden, passphrase))
            .await
            .unwrap();
        let mut reader = reader(&sim).await;
        let join = join::join(&mut reader, "1").await.unwrap();
        assert_eq!(join.payload().expose_secret(), payload, "{ssid}");
        assert_eq!(join.ssid, ssid);
        assert_eq!(join.hidden, hidden, "{ssid}");
        assert_eq!(join.enabled, Some(true));
        assert!(!join.guest);
        // The passphrase is read only for networks that need one.
        let wpa = join.auth == Auth::Wpa;
        assert_eq!(calls(&sim, "GetSecurityKeys"), usize::from(wpa), "{ssid}");
        assert_eq!(join.passphrase.is_some(), wpa, "{ssid}");
        assert!(!format!("{join:?}").contains("fake-"), "{join:?}");
    }
}

#[tokio::test]
async fn names_the_guest_network_guest() {
    // The first network has the SSID `guest`, the third is the guest network.
    let fixture = network("guest", "11i", false, "fake-ssid-passphrase").with_answer(
        "WLANConfiguration3",
        "GetSecurityKeys",
        None,
        keys("fake-guest-passphrase"),
    );
    let sim = Sim::start(fixture).await.unwrap();
    let mut reader = reader(&sim).await;
    for name in [join::GUEST, "Guest", "GUEST"] {
        let join = join::join(&mut reader, name).await.unwrap();
        assert_eq!(join.service, "WLANConfiguration3", "{name}");
        assert!(join.guest);
        assert_eq!(join.enabled, Some(false));
        assert_eq!(
            join.payload().expose_secret(),
            "WIFI:T:WPA;S:wlan-2;P:fake-guest-passphrase;;"
        );
    }
    // Still by its number or service; by SSID, the first network with it.
    for name in ["1", "WLANConfiguration1"] {
        let join = join::join(&mut reader, name).await.unwrap();
        assert_eq!(join.ssid, "guest");
    }
    let join = join::join(&mut reader, "wlan-2").await.unwrap();
    assert_eq!(join.service, "WLANConfiguration3");
}

#[tokio::test]
async fn fails_without_a_network_or_a_passphrase() {
    let fixture = Fixture::load(FIXTURE)
        .unwrap()
        .with_answer(
            "WLANConfiguration3",
            "X_AVM-DE_GetWLANExtInfo",
            None,
            answer(&[("NewX_AVM-DE_APType", "normal")]),
        )
        .with_answer(
            "WLANConfiguration2",
            "GetInfo",
            None,
            answer(&[
                ("NewSSID", "wlan-wep"),
                ("NewBeaconType", "Basic"),
                ("NewBasicEncryptionModes", "WEPEncryption"),
            ]),
        )
        .with_answer(
            "WLANConfiguration3",
            "GetInfo",
            None,
            answer(&[("NewSSID", ""), ("NewBeaconType", "11i")]),
        );
    let sim = Sim::start(fixture).await.unwrap();
    let mut reader = reader(&sim).await;
    let message = |error: Error| error.to_string();
    let error = join::join(&mut reader, "guest").await.unwrap_err();
    assert_eq!(message(error), "the device has no guest network");
    let error = join::join(&mut reader, "nothing").await.unwrap_err();
    assert_eq!(message(error), r#"no wireless network is named "nothing""#);
    let error = join::join(&mut reader, "wlan-wep").await.unwrap_err();
    assert_eq!(
        message(error),
        "WLANConfiguration2 uses the beacon type \"Basic\", which a QR code cannot describe"
    );
    let error = join::join(&mut reader, "3").await.unwrap_err();
    assert_eq!(message(error), "WLANConfiguration3 has no SSID");
    assert_eq!(calls(&sim, "GetSecurityKeys"), 0);
    // The simulator answers an empty passphrase without a recorded one.
    let error = join::join(&mut reader, "1").await.unwrap_err();
    assert_eq!(
        message(error),
        "the device returned no passphrase for WLANConfiguration1"
    );
    sim.inject(Injection::fault(
        "WLANConfiguration1",
        "GetSecurityKeys",
        606,
    ));
    let error = join::join(&mut reader, "1").await.unwrap_err();
    assert!(
        matches!(error, Error::Read(generic::Error::Fault { .. })),
        "{error:?}"
    );
}

#[tokio::test]
async fn keeps_the_passphrase_from_every_other_path() {
    let sim = Sim::start(network("wlan-1", "11i", false, "fake-passphrase"))
        .await
        .unwrap();
    let client = sim.client().build().unwrap();
    let mut reader = reader(&sim).await;
    // The read path refuses it.
    let error = reader
        .read("WLANConfiguration1", "GetSecurityKeys", &[])
        .await
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "GetSecurityKeys is secret: the name points to a secret; only actions that read are called"
    );
    let missing = reader
        .try_read("WLANConfiguration1", "GetSecurityKeys", &[])
        .await;
    assert!(matches!(missing, Err(generic::Error::Refused { .. })));
    // Generic calls, as of `fritz tr064 call` and `fritz_call_action`.
    let (service, scpd) = generic::service(&client, "WLANConfiguration1")
        .await
        .unwrap();
    let action = generic::action(&service, &scpd, "GetSecurityKeys").unwrap();
    let error = generic::call(&client, &service, action, &[])
        .await
        .unwrap_err();
    assert!(matches!(error, generic::Error::Refused { .. }), "{error}");
    assert_eq!(calls(&sim, "GetSecurityKeys"), 0);
    // The capture.
    let plan = capture::plan(&client).await.unwrap();
    let decisions: Vec<&Decision> = plan
        .services
        .iter()
        .filter(|s| s.service.service_id.contains("WLANConfiguration"))
        .flat_map(|s| s.scpd.as_ref().unwrap().actions.iter())
        .filter(|(action, _)| action.name == "GetSecurityKeys")
        .map(|(_, decision)| decision)
        .collect();
    assert_eq!(decisions.len(), 3);
    assert!(
        decisions
            .iter()
            .all(|d| matches!(d, Decision::Denied { .. }))
    );
    let options = Options {
        max_entries: 1,
        delay: Duration::ZERO,
        ..Options::default()
    };
    capture::run(&client, &plan, &options, |_| {}).await;
    assert_eq!(calls(&sim, "GetSecurityKeys"), 0);
    // Only the narrow path asks for it.
    let join = join::join(&mut reader, "wlan-1").await.unwrap();
    assert_eq!(join.passphrase.unwrap().expose_secret(), "fake-passphrase");
    assert_eq!(calls(&sim, "GetSecurityKeys"), 1);
}
