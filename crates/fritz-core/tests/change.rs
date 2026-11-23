//! Planning and applying changes against the simulator.

#![cfg(feature = "write")]

use std::path::Path;

use fritz_core::change::{Change, Changer, Check, Error, PLAN_NOTE, Permit, Settings, SideEffect};
use fritz_core::fault::Fault;
use fritz_core::generic;
use fritz_core::identity::{Identity, Targets};
use fritz_sim::{Fixture, Injection, Sim};

const FIXTURE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../fritz-sim/fixtures/fritzbox-7690/8.25"
);

/// The identity of the device of the fixture.
fn identity() -> Identity {
    Identity::of("uuid:00000000-0000-4000-8000-000000000001", "AABBCC000001")
}

const WRITE: Permit = Permit {
    write: true,
    critical: false,
};

struct Setup {
    sim: Sim,
    dir: tempfile::TempDir,
}

impl Setup {
    async fn new() -> Self {
        Self {
            sim: Sim::start(Fixture::load(FIXTURE).unwrap()).await.unwrap(),
            dir: tempfile::tempdir().unwrap(),
        }
    }

    fn changer(&self, permit: Permit, targets: Targets) -> Changer {
        Changer::new(
            self.sim.client().build().unwrap(),
            Settings {
                permit,
                targets,
                dir: self.dir.path().to_owned(),
                source: "test",
            },
        )
    }

    fn allowed(&self) -> Changer {
        self.changer(WRITE, allow())
    }

    /// The actions the simulator was asked to call.
    fn calls(&self) -> Vec<String> {
        self.sim
            .requests()
            .into_iter()
            .filter_map(|r| r.action)
            .collect()
    }

    /// The events in the audit log.
    fn events(&self) -> Vec<serde_json::Value> {
        audit(self.dir.path())
    }
}

fn audit(dir: &Path) -> Vec<serde_json::Value> {
    std::fs::read_to_string(dir.join("audit.jsonl"))
        .unwrap_or_default()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

fn allow() -> Targets {
    Targets {
        allow: vec![identity()],
        deny: Vec::new(),
    }
}

fn enable(value: &str) -> Vec<(String, String)> {
    vec![("NewEnable".to_owned(), value.to_owned())]
}

#[tokio::test]
async fn plans_and_applies_a_change() {
    let setup = Setup::new().await;
    let mut changer = setup.allowed();
    assert_eq!(changer.identity().await.unwrap(), identity());

    let plan = changer
        .plan("WLANConfiguration1", "SetEnable", &enable("0"))
        .await
        .unwrap();
    assert_eq!(plan.device, identity());
    assert_eq!(plan.service, "WLANConfiguration1");
    assert_eq!(plan.action, "SetEnable");
    assert_eq!(plan.class, "write");
    assert_eq!(plan.expires_in, 300);
    assert_eq!(plan.note, PLAN_NOTE);
    assert_eq!(
        plan.changes,
        [Change {
            argument: "NewEnable".to_owned(),
            before: Some("1".to_owned()),
            after: "0".to_owned(),
        }]
    );
    assert!(!setup.calls().contains(&"SetEnable".to_owned()));
    assert_eq!(
        setup.sim.state("WLANConfiguration1", "Enable").as_deref(),
        Some("1")
    );

    let outcome = changer
        .apply("WLANConfiguration1", "SetEnable", &enable("0"), &plan.token)
        .await
        .unwrap();
    assert_eq!(
        setup.sim.state("WLANConfiguration1", "Enable").as_deref(),
        Some("0")
    );
    assert_eq!(outcome.verified, Some(true));
    assert_eq!(
        outcome.checks,
        [Check {
            argument: "NewEnable".to_owned(),
            expected: "0".to_owned(),
            actual: Some("0".to_owned()),
        }]
    );
    assert_eq!(outcome.device, identity());
    assert_eq!(outcome.service, "WLANConfiguration1");
    assert_eq!(outcome.action, "SetEnable");
    assert_eq!(outcome.side_effects, []);

    // A token is used once.
    let error = changer
        .apply("WLANConfiguration1", "SetEnable", &enable("0"), &plan.token)
        .await
        .unwrap_err();
    assert!(matches!(error, Error::InvalidToken), "{error:?}");

    let events: Vec<_> = setup.events().iter().map(|e| e["event"].clone()).collect();
    assert_eq!(events, ["planned", "sending", "applied", "refused"]);
    let applied = &setup.events()[2];
    assert_eq!(applied["source"], "test");
    assert_eq!(applied["device"], identity().to_string());
    assert_eq!(applied["verified"], true);
}

#[tokio::test]
async fn binds_tokens_to_the_change() {
    let setup = Setup::new().await;
    let mut changer = setup.allowed();
    let plan = changer
        .plan("WLANConfiguration1", "SetEnable", &enable("0"))
        .await
        .unwrap();
    let error = changer
        .apply("WLANConfiguration1", "SetEnable", &enable("1"), &plan.token)
        .await
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "the token is unknown, used, expired or for another change"
    );
    let error = changer
        .apply("WLANConfiguration2", "SetEnable", &enable("0"), &plan.token)
        .await
        .unwrap_err();
    assert!(matches!(error, Error::InvalidToken), "{error:?}");
    assert!(!setup.calls().contains(&"SetEnable".to_owned()));
    changer
        .apply("WLANConfiguration1", "SetEnable", &enable("0"), &plan.token)
        .await
        .unwrap();
}

#[tokio::test]
async fn needs_the_permit_before_anything_is_called() {
    let setup = Setup::new().await;
    let mut changer = setup.changer(Permit::default(), allow());
    let error = changer
        .plan("WLANConfiguration1", "SetEnable", &enable("0"))
        .await
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "SetEnable is write; it needs FRITZ_ALLOW_WRITE=1"
    );
    let error = changer
        .apply("WLANConfiguration1", "SetEnable", &enable("0"), "token")
        .await
        .unwrap_err();
    assert!(matches!(error, Error::NotPermitted { .. }), "{error:?}");
    assert_eq!(setup.calls(), Vec::<String>::new());
    let events = setup.events();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0]["event"], "refused");
    assert_eq!(
        events[0]["error"],
        "SetEnable is write; it needs FRITZ_ALLOW_WRITE=1"
    );
    assert!(events[0].get("device").is_none());

    let error = setup
        .changer(WRITE, allow())
        .plan("DeviceInfo1", "GetInfo", &[])
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), "GetInfo is read, not a change");
}

#[tokio::test]
async fn needs_both_permits_for_critical_changes() {
    let setup = Setup::new().await;
    let error = setup
        .changer(WRITE, allow())
        .plan("DeviceConfig1", "Reboot", &[])
        .await
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "Reboot is critical: changes DeviceConfig; it needs FRITZ_ALLOW_WRITE=1 and FRITZ_ALLOW_CRITICAL=1"
    );
    let all = Permit {
        write: true,
        critical: true,
    };
    let plan = setup
        .changer(all, allow())
        .plan("DeviceConfig1", "Reboot", &[])
        .await
        .unwrap();
    assert_eq!(plan.class, "critical: changes DeviceConfig");
    assert_eq!(plan.changes, []);
    assert!(!setup.calls().contains(&"Reboot".to_owned()));
}

#[tokio::test]
async fn changes_only_allowed_devices() {
    let setup = Setup::new().await;
    let error = setup
        .changer(WRITE, Targets::default())
        .plan("WLANConfiguration1", "SetEnable", &enable("0"))
        .await
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        format!(
            "the device {} is not on the allow list for changes",
            identity()
        )
    );
    let denied = Targets {
        allow: vec![identity()],
        deny: vec![identity()],
    };
    let error = setup
        .changer(WRITE, denied)
        .plan("WLANConfiguration1", "SetEnable", &enable("0"))
        .await
        .unwrap_err();
    assert!(matches!(error, Error::DeniedDevice(_)), "{error:?}");
    assert!(!setup.calls().contains(&"SetEnable".to_owned()));
    let refused = &setup.events()[1];
    assert_eq!(refused["device"], identity().to_string());
}

#[tokio::test]
async fn checks_the_arguments() {
    let setup = Setup::new().await;
    let error = setup
        .allowed()
        .plan("WLANConfiguration1", "SetEnable", &[])
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), "SetEnable needs the input NewEnable");
    let twice = [enable("0"), enable("1")].concat();
    let error = setup
        .allowed()
        .plan("WLANConfiguration1", "SetEnable", &twice)
        .await
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "SetEnable got the input NewEnable more than once"
    );
}

#[tokio::test]
async fn reads_values_back_only_from_the_same_entry() {
    let setup = Setup::new().await;
    let arguments = vec![
        ("NewMACAddress".to_owned(), "AA:BB:CC:00:00:01".to_owned()),
        ("NewX_AVM-DE_FriendlyName".to_owned(), "renamed".to_owned()),
    ];
    let plan = setup
        .allowed()
        .plan("Hosts1", "X_AVM-DE_SetFriendlyNameByMAC", &arguments)
        .await
        .unwrap();
    // X_AVM-DE_GetFriendlyName returns the name of the device, not of the
    // host with this address.
    assert_eq!(plan.changes[1].before, None, "{:?}", plan.changes);
    assert!(
        !setup
            .calls()
            .contains(&"X_AVM-DE_GetFriendlyName".to_owned())
    );
}

#[tokio::test]
async fn tells_when_the_device_keeps_the_old_value() {
    let setup = Setup::new().await;
    let mut changer = setup.allowed();
    let plan = changer
        .plan("WLANConfiguration1", "SetEnable", &enable("0"))
        .await
        .unwrap();
    setup.sim.inject(Injection::DiscardWrites);
    let outcome = changer
        .apply("WLANConfiguration1", "SetEnable", &enable("0"), &plan.token)
        .await
        .unwrap();
    assert_eq!(outcome.verified, Some(false));
    assert_eq!(outcome.checks[0].actual.as_deref(), Some("1"));
}

#[tokio::test]
async fn reports_faults_of_the_change() {
    let setup = Setup::new().await;
    let mut changer = setup.allowed();
    let plan = changer
        .plan("WLANConfiguration1", "SetEnable", &enable("0"))
        .await
        .unwrap();
    setup
        .sim
        .inject(Injection::fault("WLANConfiguration1", "SetEnable", 866));
    let error = changer
        .apply("WLANConfiguration1", "SetEnable", &enable("0"), &plan.token)
        .await
        .unwrap_err();
    assert!(
        matches!(
            error,
            Error::Call(generic::Error::Fault {
                fault: Fault::ConfirmationRequired,
                ..
            })
        ),
        "{error:?}"
    );
    let failed = setup.events().pop().unwrap();
    assert_eq!(failed["event"], "failed");
}

#[tokio::test]
async fn leaves_out_values_no_getter_returns() {
    let setup = Setup::new().await;
    let mut changer = setup.allowed();
    // The only getter of the variable fails with an error of the device.
    setup
        .sim
        .inject(Injection::fault("WLANConfiguration1", "GetInfo", 820));
    let plan = changer
        .plan("WLANConfiguration1", "SetEnable", &enable("0"))
        .await
        .unwrap();
    assert_eq!(plan.changes[0].before, None);
    let outcome = changer
        .apply("WLANConfiguration1", "SetEnable", &enable("0"), &plan.token)
        .await
        .unwrap();
    assert_eq!(outcome.verified, None);
    assert_eq!(outcome.checks[0].actual, None);

    setup.sim.clear_injections();
    setup
        .sim
        .inject(Injection::fault("WLANConfiguration1", "GetInfo", 606));
    let plan = changer
        .plan("WLANConfiguration1", "SetEnable", &enable("1"))
        .await
        .unwrap();
    assert_eq!(plan.changes[0].before, None);
}

#[tokio::test]
async fn fails_when_values_cannot_be_read() {
    let setup = Setup::new().await;
    let mut changer = setup.allowed();
    changer.identity().await.unwrap();
    setup.sim.inject(Injection::LockLogin);
    let error = changer
        .plan("WLANConfiguration1", "SetEnable", &enable("0"))
        .await
        .unwrap_err();
    assert!(matches!(error, Error::Call(_)), "{error:?}");
}

#[tokio::test]
async fn reads_values_back_with_the_inputs_of_getters() {
    let setup = Setup::new().await;
    let mut changer = setup.allowed();
    let arguments = vec![
        ("NewMACAddress".to_owned(), "AA:BB:CC:00:00:01".to_owned()),
        ("NewHostName".to_owned(), "device-renamed".to_owned()),
    ];
    let plan = changer
        .plan("Hosts1", "X_AVM-DE_SetHostNameByMACAddress", &arguments)
        .await
        .unwrap();
    assert_eq!(plan.changes[1].argument, "NewHostName");
    assert!(plan.changes[1].before.is_some(), "{:?}", plan.changes);
    let calls = setup.calls();
    assert!(
        calls.contains(&"GetSpecificHostEntry".to_owned()),
        "{calls:?}"
    );
    // Its index is not an input of the change.
    assert!(
        !calls.contains(&"GetGenericHostEntry".to_owned()),
        "{calls:?}"
    );
    let outcome = changer
        .apply(
            "Hosts1",
            "X_AVM-DE_SetHostNameByMACAddress",
            &arguments,
            &plan.token,
        )
        .await
        .unwrap();
    assert_eq!(outcome.verified, Some(true), "{:?}", outcome.checks);
}

#[tokio::test]
async fn hides_secrets() {
    let setup = Setup::new().await;
    let all = Permit {
        write: true,
        critical: true,
    };
    let mut changer = setup.changer(all, allow());
    let plan = changer
        .plan("WLANConfiguration1", "SetSecurityKeys", &keys())
        .await
        .unwrap();
    assert_eq!(plan.class, "critical: sets a secret");
    assert!(
        plan.changes
            .iter()
            .all(|c| c.after == "***" && c.before.as_deref().is_none_or(|b| b == "***")),
        "{:?}",
        plan.changes
    );
    let outcome = changer
        .apply(
            "WLANConfiguration1",
            "SetSecurityKeys",
            &keys(),
            &plan.token,
        )
        .await
        .unwrap();
    assert_eq!(outcome.verified, None);
    let text = std::fs::read_to_string(setup.dir.path().join("audit.jsonl")).unwrap();
    assert!(!text.contains("geheim"), "{text}");
}

/// The inputs of `SetSecurityKeys`, with secrets.
fn keys() -> Vec<(String, String)> {
    [
        "NewWEPKey0",
        "NewWEPKey1",
        "NewWEPKey2",
        "NewWEPKey3",
        "NewPreSharedKey",
        "NewKeyPassphrase",
    ]
    .iter()
    .map(|n| ((*n).to_owned(), format!("geheim-{n}")))
    .collect()
}

#[tokio::test]
async fn reports_side_effects() {
    let setup = Setup::new().await;
    let mut changer = setup.allowed();
    setup.sim.inject(Injection::side_effect(
        "WLANConfiguration1",
        "SetEnable",
        "Status",
        "Disabled",
    ));
    let plan = changer
        .plan("WLANConfiguration1", "SetEnable", &enable("0"))
        .await
        .unwrap();
    // The plan reads nothing it does not change.
    assert_eq!(
        setup.sim.state("WLANConfiguration1", "Status").as_deref(),
        Some("Up")
    );
    let outcome = changer
        .apply("WLANConfiguration1", "SetEnable", &enable("0"), &plan.token)
        .await
        .unwrap();
    assert_eq!(outcome.verified, Some(true));
    assert_eq!(
        outcome.side_effects,
        [SideEffect {
            getter: "GetInfo".to_owned(),
            argument: "NewStatus".to_owned(),
            before: Some("Up".to_owned()),
            after: Some("Disabled".to_owned()),
        }]
    );
}

#[tokio::test]
async fn reads_nothing_when_the_snapshot_fails() {
    let setup = Setup::new().await;
    let mut changer = setup.allowed();
    let plan = changer
        .plan("WLANConfiguration1", "SetEnable", &enable("0"))
        .await
        .unwrap();
    // A client that gives up soon, with the same tokens.
    let client = setup
        .sim
        .client()
        .timeout(std::time::Duration::from_millis(200))
        .build()
        .unwrap();
    let settings = Settings {
        permit: WRITE,
        targets: allow(),
        dir: setup.dir.path().to_owned(),
        source: "test",
    };
    let mut impatient = Changer::new(client, settings);
    impatient.identity().await.unwrap();
    setup
        .sim
        .inject(Injection::Delay(std::time::Duration::from_secs(2)));
    let error = impatient
        .apply("WLANConfiguration1", "SetEnable", &enable("0"), &plan.token)
        .await
        .unwrap_err();
    assert!(matches!(error, Error::Call(_)), "{error:?}");
    setup.sim.clear_injections();
    assert!(!setup.calls().contains(&"SetEnable".to_owned()));
    // The token is still valid.
    changer
        .apply("WLANConfiguration1", "SetEnable", &enable("0"), &plan.token)
        .await
        .unwrap();
}
