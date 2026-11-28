//! The users and login settings of the simulated device, read with getters
//! only.

use std::collections::BTreeMap;

use fritz_core::read::Reader;
use fritz_core::users::{self, Login, User, Users};
use fritz_sim::{Answer, Fixture, Injection, Sim};

const FIXTURE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../fritz-sim/fixtures/fritzbox-7690/8.25"
);

async fn read(sim: &Sim) -> (Users, Vec<String>) {
    let client = sim.client().build().unwrap();
    let mut reader = Reader::new(client).await.unwrap();
    let users = users::users(&mut reader).await.unwrap();
    let called = sim
        .requests()
        .into_iter()
        .filter(|r| r.authorized)
        .filter_map(|r| r.action)
        .collect();
    (users, called)
}

fn ok(out: &[(&str, &str)]) -> Answer {
    Answer::Ok(
        out.iter()
            .map(|(n, v)| ((*n).to_owned(), (*v).to_owned()))
            .collect::<BTreeMap<_, _>>(),
    )
}

fn user(name: &str, last_login: bool) -> User {
    User {
        name: name.to_owned(),
        last_login,
    }
}

#[tokio::test]
async fn reads_several_users() {
    let sim = Sim::start(Fixture::load(FIXTURE).unwrap()).await.unwrap();
    let (users, called) = read(&sim).await;
    assert_eq!(
        users.users,
        Some(vec![
            user("user-1", false),
            user("user-2", false),
            user("user-3", false),
            user("user-4", true),
        ])
    );
    assert_eq!(users.rights.len(), 6);
    assert_eq!(
        users.login,
        Login {
            anonymous: Some(false),
            button: Some(false),
            default_password: Some(false),
            two_factor: Some(false),
        }
    );
    assert_eq!(users.hints, ["two-factor confirmation off"]);
    // Only the getters.
    assert_eq!(
        called,
        [
            "X_AVM-DE_GetUserList",
            "X_AVM-DE_GetCurrentUser",
            "X_AVM-DE_GetAnonymousLogin",
            "GetInfo",
            "GetInfo",
        ]
    );
}

#[tokio::test]
async fn hints_at_anonymous_login_and_the_default_password() {
    let fixture = Fixture::load(FIXTURE)
        .unwrap()
        .with_answer(
            "LANConfigSecurity1",
            "X_AVM-DE_GetUserList",
            None,
            ok(&[(
                "NewX_AVM-DE_UserList",
                "<List><Username last_user=\"1\">admin</Username></List>",
            )]),
        )
        .with_answer(
            "LANConfigSecurity1",
            "X_AVM-DE_GetAnonymousLogin",
            None,
            ok(&[
                ("NewX_AVM-DE_AnonymousLoginEnabled", "1"),
                ("NewX_AVM-DE_ButtonLoginEnabled", "0"),
            ]),
        )
        .with_answer(
            "LANConfigSecurity1",
            "GetInfo",
            None,
            ok(&[("NewX_AVM-DE_IsDefaultPasswordActive", "1")]),
        )
        .with_answer(
            "X_AVM-DE_Auth1",
            "GetInfo",
            None,
            ok(&[("NewEnabled", "1")]),
        );
    let sim = Sim::start(fixture).await.unwrap();
    let (users, _) = read(&sim).await;
    assert_eq!(users.users, Some(vec![user("admin", true)]));
    assert_eq!(users.login.anonymous, Some(true));
    assert_eq!(users.login.default_password, Some(true));
    assert_eq!(
        users.hints,
        ["default password active", "login without password allowed"]
    );
}

#[tokio::test]
async fn reports_a_list_it_may_not_read_as_missing() {
    let sim = Sim::start(Fixture::load(FIXTURE).unwrap()).await.unwrap();
    sim.inject(Injection::fault(
        "LANConfigSecurity1",
        "X_AVM-DE_GetUserList",
        606,
    ));
    let (users, _) = read(&sim).await;
    assert_eq!(users.users, None);
    // The rest still answers.
    assert_eq!(users.login.anonymous, Some(false));
    let json = serde_json::to_value(&users).unwrap();
    assert!(json["users"].is_null(), "{json}");
}
