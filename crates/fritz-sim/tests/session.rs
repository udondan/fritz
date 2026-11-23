//! The session of the web interface and the Smart Home REST API of the
//! simulated 7690.

use fritz_core::personal::web_log;
use fritz_core::session::{Error, Session};
use fritz_core::smarthome::history::{HOMEAUTO, History, Range, history};
use fritz_core::{settings, smarthome};
use fritz_sim::{Fixture, Injection, PASSWORD, Sim, USERNAME};
use tr064::{Client, SecretString};

const FIXTURE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/fritzbox-7690/8.25");
const SID: &str = "AVM-SID 5e55104d0c0ffee5";

async fn sim() -> Sim {
    Sim::start(Fixture::load(FIXTURE).unwrap()).await.unwrap()
}

/// A client for the web interface, without credentials.
fn client(sim: &Sim) -> Client {
    Client::builder(sim.url().clone()).build().unwrap()
}

async fn login(sim: &Sim, password: &str) -> Result<Session, Error> {
    Session::login(client(sim), USERNAME, &SecretString::from(password)).await
}

#[tokio::test]
async fn reads_the_devices_in_a_session() {
    let sim = sim().await;
    let session = login(&sim, PASSWORD).await.unwrap();
    let rights: Vec<_> = session
        .rights()
        .iter()
        .map(|r| format!("{}: {}", r.area, r.access))
        .collect();
    assert_eq!(rights, ["HomeAuto: readwrite", "BoxAdmin: readwrite"]);

    let devices = smarthome::devices(&session).await.unwrap();
    let names: Vec<_> = devices.iter().map(|d| d.name.as_str()).collect();
    assert_eq!(names, ["device-1", "device-2", "device-3", "device-4"]);
    assert_eq!(devices[0].units[0].power, Some(23.45));

    // Other documents of the API are not in the fixture.
    assert!(
        session
            .get("/api/v0/smarthome/configuration")
            .await
            .is_err()
    );

    session.logout().await.unwrap();
    let after = client(&sim)
        .get_with_headers(smarthome::OVERVIEW, &[("Authorization", SID)])
        .await;
    assert!(after.is_err(), "the session is still open");

    // The login sends the answer to the challenge, never the password.
    let posts: Vec<_> = sim
        .requests()
        .into_iter()
        .filter(|r| r.method == "POST")
        .collect();
    assert_eq!(posts.len(), 2);
    assert!(posts.iter().all(|r| r.path == "/login_sid.lua"));
}

#[tokio::test]
async fn reads_the_history_of_devices() {
    let sim = sim().await;
    let session = login(&sim, PASSWORD).await.unwrap();
    let kinds = |history: &History| {
        history
            .series
            .iter()
            .map(|s| format!("{} {:?}", s.kind, s.range))
            .collect::<Vec<_>>()
    };

    let socket = history(&session, "device-1", None).await.unwrap().unwrap();
    assert_eq!(socket.ain, "11630 0000001");
    assert_eq!(
        kinds(&socket),
        [
            "power Hours",
            "voltage Hours",
            "energy Days",
            "energy Months",
            "temperature Hours"
        ]
    );
    let days = history(&session, "11630 0000001", Some(Range::Days))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(kinds(&days), ["energy Days"]);
    assert_eq!(
        days.series[0].values,
        [Some(100.0), Some(95.0), None, Some(110.0)]
    );

    let thermostat = history(&session, "device-2", None).await.unwrap().unwrap();
    assert_eq!(kinds(&thermostat), ["temperature Hours"]);
    let sensor = history(&session, "DEVICE-4", None).await.unwrap().unwrap();
    assert_eq!(kinds(&sensor), ["temperature Hours", "humidity Hours"]);
    assert_eq!(sensor.series[1].values, [Some(55.0), None, Some(60.0)]);
    assert_eq!(sensor.series[1].newest, None);
    // A lamp keeps no history.
    let lamp = history(&session, "device-3", None).await.unwrap().unwrap();
    assert_eq!(lamp.series, []);
    assert_eq!(history(&session, "other", None).await.unwrap(), None);

    // Only the command that reads.
    let commands: Vec<_> = sim
        .requests()
        .into_iter()
        .filter(|r| r.path == HOMEAUTO)
        .map(|r| r.method)
        .collect();
    assert_eq!(commands, ["POST"; 5]);
}

#[tokio::test]
async fn needs_a_session_for_the_api() {
    let sim = sim().await;
    let without = client(&sim).get(smarthome::OVERVIEW).await;
    assert!(without.is_err());
    // The session ID alone is not enough before the login.
    let guessed = client(&sim)
        .get_with_headers(smarthome::OVERVIEW, &[("Authorization", SID)])
        .await;
    assert!(guessed.is_err());
}

#[tokio::test]
async fn reports_a_wrong_password() {
    let sim = sim().await;
    let error = login(&sim, "wrong").await.unwrap_err();
    assert!(matches!(error, Error::Rejected(8)), "{error}");
    // One attempt only.
    let posts = sim.requests().iter().filter(|r| r.method == "POST").count();
    assert_eq!(posts, 1);
}

#[tokio::test]
async fn reports_a_blocked_login() {
    let sim = sim().await;
    sim.inject(Injection::LockLogin);
    let error = login(&sim, PASSWORD).await.unwrap_err();
    assert!(matches!(error, Error::Blocked(60)), "{error}");
    // Blocked logins are not even tried.
    assert!(sim.requests().iter().all(|r| r.method == "GET"));
}

#[tokio::test]
async fn ignores_the_logout_of_another_session() {
    let sim = sim().await;
    let session = login(&sim, PASSWORD).await.unwrap();
    client(&sim)
        .post_form(
            "/login_sid.lua",
            &[("logout", "1"), ("sid", "0123456789abcdef")],
        )
        .await
        .unwrap();
    assert!(smarthome::devices(&session).await.is_ok());
}

#[tokio::test]
async fn rejects_a_login_while_locked() {
    // A client that skips the block time still cannot log in.
    let sim = sim().await;
    sim.inject(Injection::LockLogin);
    let answer = client(&sim)
        .post_form(
            "/login_sid.lua",
            &[("username", USERNAME), ("response", "x")],
        )
        .await
        .unwrap();
    assert!(answer.contains("<BlockTime>60</BlockTime>"), "{answer}");
}

#[tokio::test]
async fn reads_the_settings() {
    let sim = sim().await;
    let session = login(&sim, PASSWORD).await.unwrap();

    let defaults = settings::settings(&session, &[]).await;
    let names: Vec<_> = defaults.modules.keys().map(String::as_str).collect();
    let expected: Vec<_> = settings::MODULES
        .iter()
        .filter(|(_, _, default)| *default)
        .map(|(name, _, _)| *name)
        .collect();
    let mut sorted = expected.clone();
    sorted.sort_unstable();
    assert_eq!(names, sorted);
    assert!(
        defaults.unavailable.is_empty(),
        "{:?}",
        defaults.unavailable
    );
    assert_eq!(defaults.modules["dnscfg"]["ipv4_firstdns"], "198.51.100.53");
    assert_eq!(
        defaults.modules["mesh"],
        serde_json::json!({"role": "auto", "is_master": "1"})
    );
    assert_eq!(
        defaults.modules["connections"]["connection"][0]["username"],
        "***"
    );

    let modules = ["wlan", "missing", "../box"].map(str::to_owned);
    let chosen = settings::settings(&session, &modules).await;
    assert_eq!(chosen.modules["wlan"]["pskvalue"], "***");
    assert_eq!(chosen.modules["wlan"]["guest_pskvalue"], "***");
    assert_eq!(chosen.modules["wlan"]["ssid"], "wlan-1");
    assert!(chosen.unavailable["missing"].contains("404"), "{chosen:?}");
    assert!(chosen.unavailable["../box"].contains("not a module"));
    // An invalid name is refused before anything is sent.
    assert!(!sim.requests().iter().any(|r| r.path.contains("..")));
    session.logout().await.unwrap();

    // A device without the modules of the mesh, like a repeater.
    let repeater = Fixture::load(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/fixtures/fritz-repeater-2400/8.25"
    ))
    .unwrap();
    let sim = Sim::start(repeater).await.unwrap();
    let session = login(&sim, PASSWORD).await.unwrap();
    let mesh = settings::settings(&session, &[settings::MESH.to_owned()]).await;
    assert!(mesh.unavailable["mesh"].contains("404"), "{mesh:?}");
}

/// The number of pages of `/data.lua` the simulator served.
fn pages(sim: &Sim) -> usize {
    sim.requests()
        .iter()
        .filter(|r| r.path == "/data.lua")
        .count()
}

#[tokio::test]
async fn reads_the_event_log_without_choosing_a_category() {
    let sim = sim().await;
    let session = login(&sim, PASSWORD).await.unwrap();
    assert_eq!(sim.log_filter().as_deref(), Some("all"));
    let log = web_log(&session, "usb").await.unwrap();
    assert!(log.iter().any(|e| e.category.as_deref() == Some("usb")));
    assert!(log.iter().any(|e| e.category.as_deref() == Some("wlan")));
    assert_eq!(pages(&sim), 1);
    assert_eq!(sim.log_filter().as_deref(), Some("all"));
}

#[tokio::test]
async fn reads_the_category_the_web_interface_shows() {
    let page = r#"{"data": {"filter": "net", "log": [
        {"date": "19.11.26", "time": "23:58:40", "msg": "Up.", "group": "net"}
    ]}}"#;
    let fixture = Fixture::load(FIXTURE).unwrap().with_page("log", page);
    let sim = Sim::start(fixture).await.unwrap();
    let session = login(&sim, PASSWORD).await.unwrap();
    let internet = web_log(&session, "internet").await.unwrap();
    assert_eq!(internet[0].message, "Up.");
    let error = web_log(&session, "system").await.unwrap_err().to_string();
    assert!(
        error.contains("it shows only the category internet"),
        "{error}"
    );
    assert_eq!(pages(&sim), 2);
    assert_eq!(sim.log_filter().as_deref(), Some("net"));
}

#[tokio::test]
async fn reads_a_page_that_names_no_category() {
    let page = r#"{"data": {"log": [{"msg": "Restarted.", "group": "sys"}]}}"#;
    let fixture = Fixture::load(FIXTURE).unwrap().with_page("log", page);
    let sim = Sim::start(fixture).await.unwrap();
    let session = login(&sim, PASSWORD).await.unwrap();
    let system = web_log(&session, "system").await.unwrap();
    assert_eq!(system[0].message, "Restarted.");
    assert_eq!(sim.log_filter(), None);
}
