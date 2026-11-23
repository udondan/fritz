//! What of the simulated devices can be reached from the internet, read
//! with getters and `GET` requests of the web interface only.

use std::collections::BTreeMap;

use fritz_core::exposure::{self, Exposure, Kind, Severity, State};
use fritz_core::read::Reader;
use fritz_core::session::Session;
use fritz_sim::{Answer, Fixture, PASSWORD, Sim, USERNAME};
use tr064::{Client, SecretString};

const FIXTURE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/fritzbox-7690/8.25");

fn ok(out: &[(&str, &str)]) -> Answer {
    Answer::Ok(
        out.iter()
            .map(|(n, v)| ((*n).to_owned(), (*v).to_owned()))
            .collect::<BTreeMap<_, _>>(),
    )
}

async fn read(fixture: Fixture, web: bool) -> (Exposure, Sim) {
    let sim = Sim::start(fixture).await.unwrap();
    let session = if web {
        let client = Client::builder(sim.url().clone()).build().unwrap();
        let password = SecretString::from(PASSWORD);
        Some(Session::login(client, USERNAME, &password).await.unwrap())
    } else {
        None
    };
    let client = sim.client().build().unwrap();
    let mut reader = Reader::new(client).await.unwrap();
    let exposure = exposure::exposure(&mut reader, session).await.unwrap();
    (exposure, sim)
}

fn summary(exposure: &Exposure) -> Vec<(Kind, State, Option<usize>, Severity)> {
    exposure
        .items
        .iter()
        .map(|i| (i.kind, i.state, i.count, i.severity))
        .collect()
}

#[tokio::test]
async fn reads_the_recorded_device() {
    use Severity::{Info, Notice};
    use State::{Off, On, Unknown};
    let (exposure, sim) = read(Fixture::load(FIXTURE).unwrap(), true).await;
    assert_eq!(
        summary(&exposure),
        [
            (Kind::RemoteAccess, On, None, Notice),
            (Kind::Myfritz, On, Some(0), Info),
            (Kind::DynamicDns, Off, None, Info),
            (Kind::PortForwarding, Off, Some(0), Info),
            (Kind::ExposedHost, Off, None, Info),
            // UPnP is on, but may not change settings.
            (Kind::Upnp, Off, None, Info),
            (Kind::Ftp, Off, None, Info),
            (Kind::SharedLinks, Off, Some(0), Info),
            // The fixture has no module `vpn`.
            (Kind::Vpn, Unknown, None, Info),
            (Kind::Tr069, On, None, Info),
            (Kind::Usp, Off, Some(0), Info),
        ]
    );
    let access = exposure.remote_access.as_ref().unwrap();
    assert_eq!(access.port, Some(49032));
    assert_eq!(access.lets_encrypt, Some(false));
    let myfritz = exposure.myfritz.as_ref().unwrap();
    assert_eq!(myfritz.address.as_deref(), Some("server-3.example.net"));
    assert_eq!(myfritz.registered, Some(true));
    let usp = exposure.usp.as_ref().unwrap();
    assert_eq!(usp.controllers.len(), 1);
    assert_eq!(usp.controllers[0].host.as_deref(), Some("device-49"));
    assert_eq!(usp.myfritz, Some(false));
    assert_eq!(
        exposure.tr069.as_ref().unwrap().url.as_deref(),
        Some("https://url-2.example.net/")
    );
    assert!(!exposure.has_warning());
    // No login names, mail addresses or link URLs.
    let json = serde_json::to_string(&exposure).unwrap();
    assert!(!json.contains("user-1@example.com"), "{json}");
    assert!(!json.contains("Username"), "{json}");

    // Only getters, and the web interface only with GET.
    let requests = sim.requests();
    for request in &requests {
        if let Some(action) = &request.action {
            assert!(
                action.starts_with("Get") || action.starts_with("X_AVM-DE_Get"),
                "{action}"
            );
        }
        if request.path.starts_with("/api/") {
            assert_eq!(request.method, "GET", "{request:?}");
        }
    }
}

/// The recorded device with everything open.
fn everything_on() -> Fixture {
    Fixture::load(FIXTURE)
        .unwrap()
        .with_answer(
            "X_AVM-DE_RemoteAccess1",
            "GetDDNSInfo",
            None,
            ok(&[
                ("NewEnabled", "1"),
                ("NewDomain", "home.example"),
                ("NewProviderName", "provider"),
            ]),
        )
        .with_answer(
            "X_AVM-DE_MyFritz1",
            "GetNumberOfServices",
            None,
            ok(&[("NewNumberOfServices", "1")]),
        )
        .with_answer(
            "X_AVM-DE_MyFritz1",
            "GetServiceByIndex",
            Some(0),
            ok(&[
                ("NewEnabled", "1"),
                ("NewName", "nas"),
                ("NewScheme", "https"),
                ("NewPort", "443"),
            ]),
        )
        .with_answer(
            "WANPPPConnection1",
            "GetPortMappingNumberOfEntries",
            None,
            ok(&[("NewPortMappingNumberOfEntries", "1")]),
        )
        .with_answer(
            "WANPPPConnection1",
            "GetGenericPortMappingEntry",
            Some(0),
            ok(&[
                ("NewEnabled", "1"),
                ("NewProtocol", "TCP"),
                ("NewExternalPort", "443"),
                ("NewInternalClient", "192.168.178.10"),
                ("NewInternalPort", "443"),
                ("NewPortMappingDescription", "nas"),
            ]),
        )
        .with_answer(
            "X_AVM-DE_Storage1",
            "GetInfo",
            None,
            ok(&[
                ("NewFTPWANEnable", "1"),
                ("NewFTPWANPort", "43051"),
                ("NewFTPWANSSLOnly", "0"),
            ]),
        )
        .with_answer(
            "X_AVM-DE_Filelinks1",
            "GetNumberOfFilelinkEntries",
            None,
            ok(&[("NewNumberOfEntries", "1")]),
        )
        .with_answer(
            "X_AVM-DE_Filelinks1",
            "GetGenericFilelinkEntry",
            Some(0),
            ok(&[
                ("NewValid", "1"),
                ("NewPath", "/photos"),
                ("NewUrl", "https://link.example/token"),
                ("NewExpire", "7"),
                ("NewExpireDate", "2026-12-01T00:00:00"),
            ]),
        )
        .with_answer(
            "X_AVM-DE_USPController1",
            "GetUSPMyFRITZEnable",
            None,
            ok(&[("NewUSPMyFRITZEnabled", "1")]),
        )
        .with_api(
            "/api/v0/generic/forwardrules",
            r#"{"use_exposed_host": "1"}"#,
        )
        .with_api(
            "/api/v0/generic/box",
            r#"{"upnp_activated": "1", "upnp_control_activated": "1"}"#,
        )
        .with_api(
            "/api/v0/generic/vpn",
            r#"{"wireguard": [{"name": "phone"}, {"name": "laptop"}]}"#,
        )
}

#[tokio::test]
async fn warns_about_everything_on() {
    use Severity::{Info, Notice, Warning};
    let (exposure, _) = read(everything_on(), true).await;
    let on: Vec<(Kind, Option<usize>, Severity)> = exposure
        .items
        .iter()
        .filter(|i| i.state == State::On)
        .map(|i| (i.kind, i.count, i.severity))
        .collect();
    assert_eq!(
        on,
        [
            (Kind::RemoteAccess, None, Notice),
            (Kind::Myfritz, Some(1), Notice),
            (Kind::DynamicDns, None, Info),
            (Kind::PortForwarding, Some(1), Notice),
            (Kind::ExposedHost, None, Warning),
            (Kind::Upnp, None, Notice),
            (Kind::Ftp, None, Warning),
            (Kind::SharedLinks, Some(1), Notice),
            (Kind::Vpn, Some(2), Info),
            (Kind::Tr069, None, Info),
            (Kind::Usp, Some(0), Notice),
        ]
    );
    assert!(exposure.has_warning());
    let links = exposure.shared_links.as_ref().unwrap();
    assert_eq!(links[0].expires.as_deref(), Some("2026-12-01T00:00:00"));
    let json = serde_json::to_string(&exposure).unwrap();
    assert!(!json.contains("link.example"), "{json}");

    let hidden = exposure.without_identifying("(hidden)");
    let json = serde_json::to_string(&hidden).unwrap();
    for value in ["home.example", "/photos", "phone", "server-3", "device-49"] {
        assert!(!json.contains(value), "{value} in {json}");
    }
}

#[tokio::test]
async fn keeps_what_cannot_be_read_unknown() {
    // Without the web interface, whether UPnP may open ports is unknown.
    let (exposure, _) = read(everything_on(), false).await;
    let upnp = &exposure.items[5];
    assert_eq!((upnp.kind, upnp.state), (Kind::Upnp, State::Unknown));
    assert_eq!(upnp.hints, ["needs a login to the web interface"]);
    // Without the rights to read the forwardings, they are unknown, not
    // none.
    let fault = Answer::Fault {
        code: 606,
        description: "Action not authorized".to_owned(),
    };
    let fixture = Fixture::load(FIXTURE).unwrap().with_answer(
        "WANPPPConnection1",
        "GetPortMappingNumberOfEntries",
        None,
        fault,
    );
    let (exposure, _) = read(fixture, true).await;
    assert_eq!(exposure.port_forwardings, None);
    let forwardings = &exposure.items[3];
    assert_eq!(
        (forwardings.kind, forwardings.state),
        (Kind::PortForwarding, State::Unknown)
    );
}

#[tokio::test]
async fn reads_a_repeater_as_unknown() {
    let repeater = Fixture::bundled("fritz-repeater-2400/8.25").unwrap();
    let (exposure, _) = read(repeater, false).await;
    for item in &exposure.items {
        assert_eq!(item.state, State::Unknown, "{item:?}");
        assert_eq!(item.severity, Severity::Info, "{item:?}");
    }
    assert_eq!(exposure.port_forwardings, None);
    assert!(!exposure.has_warning());
}
