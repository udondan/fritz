//! The calls, the phonebooks and the event log of the simulated 7690.

use std::collections::BTreeMap;

use fritz_core::generic::Error;
use fritz_core::personal::{self, Contact, ContactNumber};
use fritz_core::read::Reader;
use fritz_sim::{Answer, Fixture, Sim};

const FIXTURE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../fixtures/fritzbox-7690/8.25"
);

const ONTEL: &str = "X_AVM-DE_OnTel1";

fn fixture() -> Fixture {
    Fixture::load(FIXTURE).unwrap()
}

async fn reader(sim: &Sim) -> Reader {
    Reader::new(sim.client().build().unwrap()).await.unwrap()
}

fn ok(out: &[(&str, &str)]) -> Answer {
    Answer::Ok(
        out.iter()
            .map(|(n, v)| ((*n).to_owned(), (*v).to_owned()))
            .collect::<BTreeMap<_, _>>(),
    )
}

fn fault(code: u16) -> Answer {
    Answer::Fault {
        code,
        description: String::new(),
    }
}

#[tokio::test]
async fn reads_the_calls() {
    let sim = Sim::start(fixture()).await.unwrap();
    let mut reader = reader(&sim).await;
    let calls = personal::calls(&mut reader, 50).await.unwrap();
    let kinds: Vec<_> = calls.iter().map(|c| c.kind).collect();
    assert_eq!(kinds, ["missed", "outgoing", "incoming"]);
    assert_eq!(calls[1].number.as_deref(), Some("phone-2"));
    assert_eq!(calls[1].own_number.as_deref(), Some("phone-1"));
    assert_eq!(calls[1].date.as_deref(), Some("2026-11-20 09:15"));
    assert_eq!(calls[2].minutes, Some(65));
    // The device gets the limit, and the list is cut to it anyway.
    assert_eq!(personal::calls(&mut reader, 1).await.unwrap().len(), 1);
}

#[tokio::test]
async fn reads_no_calls_without_a_call_list() {
    let sim = Sim::start(fixture().with_answer(ONTEL, "GetCallList", None, fault(606)))
        .await
        .unwrap();
    let calls = personal::calls(&mut reader(&sim).await, 50).await.unwrap();
    assert_eq!(calls, []);
}

#[tokio::test]
async fn refuses_an_invalid_call_list() {
    let url = ("NewCallListURL", "/phonebook.lua?sid=***");
    let sim = Sim::start(fixture().with_answer(ONTEL, "GetCallList", None, ok(&[url])))
        .await
        .unwrap();
    let error = personal::calls(&mut reader(&sim).await, 50)
        .await
        .unwrap_err();
    assert!(
        matches!(
            error,
            Error::InvalidDocument {
                document: "call list",
                ..
            }
        ),
        "{error}"
    );
}

#[tokio::test]
async fn reads_the_phonebooks() {
    // The second phonebook has no answer, so it is skipped.
    let list = ok(&[("NewPhonebookList", "0, 1,x")]);
    let sim = Sim::start(fixture().with_answer(ONTEL, "GetPhonebookList", None, list))
        .await
        .unwrap();
    let phonebooks = personal::phonebooks(&mut reader(&sim).await).await.unwrap();
    assert_eq!(phonebooks.len(), 1);
    assert_eq!(phonebooks[0].name.as_deref(), Some("name-5"));
    assert_eq!(
        phonebooks[0].contacts[1],
        Contact {
            name: Some("name-8".to_owned()),
            numbers: vec![ContactNumber {
                number: "phone-5".to_owned(),
                kind: Some("intern".to_owned()),
            }],
        }
    );
    let found = personal::search(phonebooks, "NAME-6");
    assert_eq!(found[0].contacts.len(), 1);
}

#[tokio::test]
async fn reads_no_phonebooks_without_a_list() {
    let fixture = fixture().with_answer(ONTEL, "GetPhonebookList", None, fault(606));
    let sim = Sim::start(fixture).await.unwrap();
    let phonebooks = personal::phonebooks(&mut reader(&sim).await).await.unwrap();
    assert_eq!(phonebooks, []);
}

#[tokio::test]
async fn refuses_an_invalid_phonebook() {
    let url = ("NewPhonebookURL", "/calllist.lua?sid=***");
    let fixture = fixture().with_answer(ONTEL, "GetPhonebook", Some(0), ok(&[url]));
    let sim = Sim::start(fixture).await.unwrap();
    let error = personal::phonebooks(&mut reader(&sim).await)
        .await
        .unwrap_err();
    assert!(
        matches!(
            error,
            Error::InvalidDocument {
                document: "phonebook",
                ..
            }
        ),
        "{error}"
    );
}

#[tokio::test]
async fn reads_the_log() {
    let sim = Sim::start(fixture()).await.unwrap();
    // The fixture holds no real log, only what replaced it.
    let log = personal::log(&mut reader(&sim).await, 10).await.unwrap();
    assert_eq!(log.len(), 1);
    assert_eq!(log[0].time, None);

    let lines = "20.11.26 18:05:42 DSL is ready.\n19.11.26 07:00:01 WLAN on.";
    let answer = ok(&[("NewDeviceLog", lines)]);
    let sim = Sim::start(fixture().with_answer("DeviceInfo1", "GetDeviceLog", None, answer))
        .await
        .unwrap();
    let log = personal::log(&mut reader(&sim).await, 1).await.unwrap();
    assert_eq!(log.len(), 1);
    assert_eq!(log[0].time.as_deref(), Some("2026-11-20 18:05:42"));
    assert_eq!(log[0].message, "DSL is ready.");

    let missing = fixture().with_answer("DeviceInfo1", "GetDeviceLog", None, fault(606));
    let sim = Sim::start(missing).await.unwrap();
    let log = personal::log(&mut reader(&sim).await, 10).await.unwrap();
    assert_eq!(log, []);
}
