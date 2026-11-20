//! A client against the simulator serving the recorded fixture.

use std::time::Duration;

use fritz_sim::{Fixture, Injection, Request, Sim};
use tr064::{Client, DeviceDescription, Error, SecretString};

const FIXTURE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../fixtures/fritzbox-7690/8.25"
);

async fn start() -> (Sim, Client, DeviceDescription) {
    let sim = Sim::start(Fixture::load(FIXTURE).unwrap()).await.unwrap();
    let client = sim.client().build().unwrap();
    let description = client.description().await.unwrap();
    (sim, client, description)
}

fn upnp_code(error: Error) -> u16 {
    match error {
        Error::Upnp { code, .. } => code,
        other => panic!("not a UPnP error: {other:?}"),
    }
}

#[tokio::test]
async fn serves_the_documents() {
    let (_sim, client, description) = start().await;
    let wlan = description.service("WLANConfiguration1").unwrap();
    let scpd = client.scpd(wlan).await.unwrap();
    assert!(scpd.action("SetEnable").is_some());
    for path in ["/missing.xml", "/"] {
        assert!(
            matches!(
                client.get(path).await,
                Err(Error::Status { status: 404, .. })
            ),
            "{path}"
        );
    }
}

#[tokio::test]
async fn serves_lists_for_the_session_only() {
    let (_sim, client, description) = start().await;
    let hosts = description.service("Hosts1").unwrap();
    let out = client
        .call(hosts, "X_AVM-DE_GetHostListPath", &[])
        .await
        .unwrap();
    let path = out.get("NewX_AVM-DE_HostListPath").unwrap();
    assert!(path.starts_with("/devicehostlist.lua?sid="), "{path}");
    assert!(!path.contains("***"), "{path}");
    let list = client.get(path).await.unwrap();
    assert!(list.contains("<HostName>device-1</HostName>"), "{list}");
    for path in [
        "/devicehostlist.lua",
        "/devicehostlist.lua?sid=0000000000000000",
    ] {
        assert!(
            matches!(
                client.get(path).await,
                Err(Error::Status { status: 403, .. })
            ),
            "{path}"
        );
    }
    assert!(matches!(
        client.get("/missinglist.lua").await,
        Err(Error::Status { status: 404, .. })
    ));
}

#[tokio::test]
async fn answers_getters_as_recorded() {
    let (_sim, client, description) = start().await;
    let wlan = description.service("WLANConfiguration1").unwrap();
    let info = client.call(wlan, "GetInfo", &[]).await.unwrap();
    assert_eq!(info.get("NewSSID"), Some("wlan-1"));
    assert_eq!(info.get("NewChannel"), Some("6"));
}

#[tokio::test]
async fn writes_change_the_getters_of_the_same_service() {
    let (sim, client, description) = start().await;
    let wlan = description.service("WLANConfiguration1").unwrap();
    let other = description.service("WLANConfiguration2").unwrap();
    let before = client.call(other, "GetInfo", &[]).await.unwrap();

    let out = client
        .call(wlan, "SetEnable", &[("NewEnable", "0")])
        .await
        .unwrap();
    assert!(out.is_empty());

    let info = client.call(wlan, "GetInfo", &[]).await.unwrap();
    assert_eq!(info.get("NewEnable"), Some("0"));
    assert_eq!(
        sim.state("WLANConfiguration1", "Enable").as_deref(),
        Some("0")
    );
    let after = client.call(other, "GetInfo", &[]).await.unwrap();
    assert_eq!(after.get("NewEnable"), before.get("NewEnable"));
    assert_eq!(sim.state("WLANConfiguration1", "Missing"), None);
    assert_eq!(sim.state("Missing1", "Enable"), None);
}

#[tokio::test]
async fn discarded_writes_change_nothing() {
    let (sim, client, description) = start().await;
    let wlan = description.service("WLANConfiguration1").unwrap();
    sim.inject(Injection::DiscardWrites);
    client
        .call(wlan, "SetChannel", &[("NewChannel", "11")])
        .await
        .unwrap();
    let info = client.call(wlan, "GetInfo", &[]).await.unwrap();
    assert_eq!(info.get("NewChannel"), Some("6"));

    sim.clear_injections();
    client
        .call(wlan, "SetChannel", &[("NewChannel", "11")])
        .await
        .unwrap();
    let info = client.call(wlan, "GetInfo", &[]).await.unwrap();
    assert_eq!(info.get("NewChannel"), Some("11"));
}

#[tokio::test]
async fn injected_faults_fail_valid_calls() {
    let (sim, client, description) = start().await;
    let wlan = description.service("WLANConfiguration1").unwrap();
    sim.inject(Injection::fault("WLANConfiguration1", "SetEnable", 866));
    let error = client
        .call(wlan, "SetEnable", &[("NewEnable", "1")])
        .await
        .unwrap_err();
    assert!(
        matches!(&error, Error::Upnp { code: 866, description }
            if description == "second factor authentication required"),
        "{error:?}"
    );
    // Arguments are checked first, and other actions are not affected.
    let error = client.call(wlan, "SetEnable", &[]).await.unwrap_err();
    assert_eq!(upnp_code(error), 402);
    client.call(wlan, "GetInfo", &[]).await.unwrap();
    let other = description.service("WLANConfiguration2").unwrap();
    client
        .call(other, "SetEnable", &[("NewEnable", "1")])
        .await
        .unwrap();
}

/// An action, its arguments and the UPnP error they cause.
type Case<'a> = (&'a str, &'a [(&'a str, &'a str)], u16);

#[tokio::test]
async fn checks_calls_against_the_service_description() {
    let (_sim, client, description) = start().await;
    let wlan = description.service("WLANConfiguration1").unwrap();
    let cases: [Case; 6] = [
        ("Reboot", &[], 401),
        ("SetEnable", &[], 402),
        ("SetEnable", &[("NewEnable", "1"), ("NewOther", "1")], 402),
        ("SetEnable", &[("NewEnable", "maybe")], 600),
        ("SetChannel", &[("NewChannel", "x")], 600),
        ("SetBeaconType", &[("NewBeaconType", "Foo")], 600),
    ];
    for (action, arguments, code) in cases {
        let error = client.call(wlan, action, arguments).await.unwrap_err();
        assert_eq!(upnp_code(error), code, "{action} {arguments:?}");
    }
}

#[tokio::test]
async fn answers_indexed_getters_as_recorded() {
    let (_sim, client, description) = start().await;
    let wlan = description.service("WLANConfiguration1").unwrap();
    let action = "GetGenericAssociatedDeviceInfo";
    let entry = client
        .call(wlan, action, &[("NewAssociatedDeviceIndex", "0")])
        .await
        .unwrap();
    assert!(entry.get("NewAssociatedDeviceMACAddress").is_some());
    let error = client
        .call(wlan, action, &[("NewAssociatedDeviceIndex", "4")])
        .await
        .unwrap_err();
    assert_eq!(upnp_code(error), 713);
    let error = client
        .call(wlan, action, &[("NewAssociatedDeviceIndex", "40")])
        .await
        .unwrap_err();
    assert_eq!(upnp_code(error), 713);
}

#[tokio::test]
async fn replays_recorded_faults() {
    let (_sim, client, description) = start().await;
    let wlan = description.service("WLANConfiguration1").unwrap();
    let error = client
        .call(wlan, "X_AVM-DE_GetWLANConnectionInfo", &[])
        .await
        .unwrap_err();
    assert!(matches!(error, Error::Upnp { .. }), "{error:?}");
}

#[tokio::test]
async fn rejects_wrong_credentials() {
    let (sim, _client, description) = start().await;
    let wlan = description.service("WLANConfiguration1").unwrap();
    let client = sim
        .client()
        .credentials(fritz_sim::USERNAME, SecretString::from("wrong"))
        .build()
        .unwrap();
    let error = client.call(wlan, "GetInfo", &[]).await.unwrap_err();
    assert!(matches!(error, Error::AuthenticationFailed), "{error:?}");
}

#[tokio::test]
async fn a_locked_login_rejects_the_right_password() {
    let (sim, client, description) = start().await;
    let wlan = description.service("WLANConfiguration1").unwrap();
    sim.inject(Injection::LockLogin);
    let error = client.call(wlan, "GetInfo", &[]).await.unwrap_err();
    assert!(matches!(error, Error::AuthenticationFailed), "{error:?}");
    // Documents need no login.
    client.description().await.unwrap();
}

#[tokio::test]
async fn delays_make_clients_time_out() {
    let (sim, _client, _description) = start().await;
    let client = sim
        .client()
        .timeout(Duration::from_millis(100))
        .build()
        .unwrap();
    sim.inject(Injection::Delay(Duration::from_secs(5)));
    let error = client.description().await.unwrap_err();
    assert!(matches!(error, Error::Http(_)), "{error:?}");
}

#[tokio::test]
async fn logs_the_requests() {
    let (sim, client, description) = start().await;
    let wlan = description.service("WLANConfiguration1").unwrap();
    client
        .call(wlan, "SetEnable", &[("NewEnable", "1")])
        .await
        .unwrap();
    let requests = sim.requests();
    assert_eq!(
        requests[0],
        Request {
            method: "GET".to_owned(),
            path: "/tr64desc.xml".to_owned(),
            action: None,
            arguments: Vec::new(),
            authorized: false,
        }
    );
    let calls: Vec<_> = requests[1..].iter().map(|r| r.authorized).collect();
    assert_eq!(calls, [false, true]);
    let last = requests.last().unwrap();
    assert_eq!(last.path, wlan.control_url);
    assert_eq!(last.action.as_deref(), Some("SetEnable"));
    assert_eq!(last.arguments, [("NewEnable".to_owned(), "1".to_owned())]);
}

#[tokio::test]
async fn serves_https_with_a_pinned_certificate() {
    let options = fritz_sim::Options {
        https: true,
        ..fritz_sim::Options::default()
    };
    let sim = Sim::start_with(Fixture::load(FIXTURE).unwrap(), options)
        .await
        .unwrap();
    assert_eq!(sim.url().scheme(), "https");
    let fingerprint = sim.fingerprint().unwrap();
    assert_eq!(
        tr064::fetch_fingerprint(sim.url(), Duration::from_secs(5))
            .await
            .unwrap(),
        fingerprint
    );

    // A client with another pin aborts the handshake; the simulator keeps
    // serving the next one.
    let other = Client::builder(sim.url().clone())
        .pin(tr064::Fingerprint::of(b"another certificate"))
        .build()
        .unwrap();
    assert!(matches!(
        other.description().await,
        Err(Error::CertificateMismatch { .. })
    ));
    let client = sim.client().build().unwrap();
    let description = client.description().await.unwrap();
    let info = description.service("DeviceInfo1").unwrap();
    let out = client.call(info, "GetInfo", &[]).await.unwrap();
    assert_eq!(out.get("NewModelName"), Some("FRITZ!Box 7690"));
}
