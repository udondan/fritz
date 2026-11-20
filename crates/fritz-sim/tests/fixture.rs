//! Loading fixtures, and what the simulator makes of incomplete ones.

use std::fs;
use std::path::Path;

use fritz_sim::{Answer, Error, Fixture, Sim};

const FIXTURE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../fixtures/fritzbox-7690/8.25"
);

/// A fixture with the recorded device description, no service descriptions
/// and no responses.
fn bare() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    fs::copy(
        Path::new(FIXTURE).join("tr64desc.xml"),
        dir.path().join("tr64desc.xml"),
    )
    .unwrap();
    fs::create_dir_all(dir.path().join("scpd")).unwrap();
    fs::create_dir_all(dir.path().join("responses/Hosts1")).unwrap();
    dir
}

fn write(dir: &Path, path: &str, content: &str) {
    fs::write(dir.join(path), content).unwrap();
}

fn invalid(dir: &Path) -> String {
    match Fixture::load(dir).unwrap_err() {
        error @ Error::Invalid { .. } => error.to_string(),
        other => panic!("not invalid: {other:?}"),
    }
}

#[test]
fn loads_the_recorded_answers() {
    let fixture = Fixture::load(FIXTURE).unwrap();
    assert!(fixture.description().service("Hosts1").is_ok());
    assert!(matches!(
        fixture.answer("WLANConfiguration1", "GetInfo", None),
        Some(Answer::Ok(out)) if out["NewSSID"] == "wlan-1"
    ));
    assert!(matches!(
        fixture.answer(
            "WLANConfiguration1",
            "GetGenericAssociatedDeviceInfo",
            Some(4)
        ),
        Some(Answer::Fault { code: 713, .. })
    ));
    assert_eq!(
        fixture.answer("WLANConfiguration1", "GetInfo", Some(0)),
        None
    );
    assert_eq!(fixture.answer("Missing1", "GetInfo", None), None);
}

#[test]
fn skips_hidden_files_other_files_and_failed_calls() {
    let dir = bare();
    let path = dir.path();
    write(path, "scpd/.DS_Store", "");
    write(path, "responses/Hosts1/README.md", "");
    write(
        path,
        "responses/Hosts1/GetInfo.json",
        r#"{"status": "failed"}"#,
    );
    let fixture = Fixture::load(path).unwrap();
    assert_eq!(fixture.answer("Hosts1", "GetInfo", None), None);
}

#[test]
fn reports_missing_files() {
    let dir = tempfile::tempdir().unwrap();
    let error = Fixture::load(dir.path()).unwrap_err();
    assert!(matches!(&error, Error::Io { path, .. } if path.ends_with("tr64desc.xml")));
    assert!(error.to_string().starts_with("cannot read "), "{error}");

    let dir = bare();
    fs::remove_dir(dir.path().join("scpd")).unwrap();
    let error = Fixture::load(dir.path()).unwrap_err();
    assert!(matches!(&error, Error::Io { path, .. } if path.ends_with("scpd")));
}

#[test]
fn reports_invalid_files() {
    let dir = bare();
    let path = dir.path();
    write(path, "tr64desc.xml", "<root/");
    assert!(invalid(path).contains("tr64desc.xml is not valid: "));

    let dir = bare();
    let path = dir.path();
    write(path, "scpd/hostsSCPD.xml", "<scpd>");
    assert!(invalid(path).contains("hostsSCPD.xml is not valid: "));

    let dir = bare();
    let path = dir.path();
    write(path, "responses/Hosts1/GetGenericHostEntry@x.json", "{}");
    assert!(invalid(path).contains("GetGenericHostEntry@x.json is not valid: "));

    let dir = bare();
    let path = dir.path();
    write(path, "responses/Hosts1/GetInfo.json", r#"{"status": "?"}"#);
    assert!(invalid(path).ends_with("GetInfo.json is not valid: unknown status"));
}

#[tokio::test]
async fn services_without_description_have_no_actions() {
    let dir = bare();
    write(
        dir.path(),
        "responses/Hosts1/GetHostNumberOfEntries.json",
        r#"{"status": "ok", "out": {"NewHostNumberOfEntries": "3"}}"#,
    );
    let sim = Sim::start(Fixture::load(dir.path()).unwrap())
        .await
        .unwrap();
    let client = sim.client().build().unwrap();
    let description = client.description().await.unwrap();
    let hosts = description.service("Hosts1").unwrap();
    let error = client
        .call(hosts, "GetHostNumberOfEntries", &[])
        .await
        .unwrap_err();
    assert!(
        matches!(error, tr064::Error::Upnp { code: 401, .. }),
        "{error:?}"
    );
}

/// A getter with one output argument.
fn getter(name: &str, argument: &str, variable: &str) -> String {
    format!(
        "<action><name>{name}</name><argumentList><argument><name>{argument}</name>\
         <direction>out</direction><relatedStateVariable>{variable}</relatedStateVariable>\
         </argument></argumentList></action>"
    )
}

#[tokio::test]
async fn unrecorded_getters_answer_from_the_state() {
    let dir = bare();
    let actions = [
        getter("GetRecorded", "NewName", "Name"),
        getter("GetName", "NewName", "Name"),
        getter("GetMode", "NewMode", "Mode"),
        getter("GetOther", "NewOther", "Undeclared"),
    ]
    .concat();
    let scpd = format!(
        "<scpd><actionList>{actions}</actionList><serviceStateTable>\
         <stateVariable><name>Name</name><dataType>string</dataType></stateVariable>\
         <stateVariable><name>Mode</name><dataType>string</dataType>\
         <defaultValue>auto</defaultValue></stateVariable>\
         </serviceStateTable></scpd>"
    );
    write(dir.path(), "scpd/hostsSCPD.xml", &scpd);
    write(
        dir.path(),
        "responses/Hosts1/GetRecorded.json",
        r#"{"status": "ok", "out": {"NewName": "box"}}"#,
    );
    let sim = Sim::start(Fixture::load(dir.path()).unwrap())
        .await
        .unwrap();
    let client = sim.client().build().unwrap();
    let description = client.description().await.unwrap();
    let hosts = description.service("Hosts1").unwrap();
    for (action, argument, value) in [
        ("GetName", "NewName", "box"),
        ("GetMode", "NewMode", "auto"),
        ("GetOther", "NewOther", ""),
    ] {
        let out = client.call(hosts, action, &[]).await.unwrap();
        assert_eq!(out.get(argument), Some(value), "{action}");
    }
}
