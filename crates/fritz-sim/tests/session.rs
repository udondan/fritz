//! The session of the web interface and the Smart Home REST API of the
//! simulated 7690.

use fritz_core::session::{Error, Session};
use fritz_core::smarthome;
use fritz_sim::{Fixture, Injection, PASSWORD, Sim, USERNAME};
use tr064::{Client, SecretString};

const FIXTURE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../fixtures/fritzbox-7690/8.25"
);
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
