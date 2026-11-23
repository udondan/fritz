//! Restarting a device against the simulator.

#![cfg(feature = "write")]

use std::time::Duration;

use fritz_core::change::reboot::{State, WARNING, Wait};
use fritz_core::change::{Changer, Error, Permit, Settings};
use fritz_core::identity::{Identity, Targets};
use fritz_sim::{Fixture, Injection, Sim};

const REPEATER: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../fritz-sim/fixtures/fritz-repeater-2400/8.25"
);

const ROUTER: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../fritz-sim/fixtures/fritzbox-7690/8.25"
);

const UDN: &str = "uuid:00000000-0000-4000-8000-000000000001";

const CRITICAL: Permit = Permit {
    write: true,
    critical: true,
};

/// Reads every 10 ms, for at most 5 seconds.
const FAST: Wait = Wait {
    interval: Duration::from_millis(10),
    budget: Duration::from_secs(5),
};

struct Setup {
    sim: Sim,
    dir: tempfile::TempDir,
    identity: Identity,
}

impl Setup {
    async fn new(fixture: &str, serial: &str) -> Self {
        Self {
            sim: Sim::start(Fixture::load(fixture).unwrap()).await.unwrap(),
            dir: tempfile::tempdir().unwrap(),
            identity: Identity::of(UDN, serial),
        }
    }

    async fn repeater() -> Self {
        Self::new(REPEATER, "AABBCC00000A").await
    }

    async fn router() -> Self {
        Self::new(ROUTER, "AABBCC000001").await
    }

    fn changer(&self, permit: Permit) -> Changer {
        Changer::new(
            self.sim.client().build().unwrap(),
            Settings {
                permit,
                targets: Targets {
                    allow: vec![self.identity.clone()],
                    deny: Vec::new(),
                },
                dir: self.dir.path().to_owned(),
                source: "test",
            },
        )
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
    fn events(&self) -> Vec<String> {
        std::fs::read_to_string(self.dir.path().join("audit.jsonl"))
            .unwrap_or_default()
            .lines()
            .map(|l| serde_json::from_str::<serde_json::Value>(l).unwrap()["event"].to_string())
            .map(|e| e.trim_matches('"').to_owned())
            .collect()
    }
}

#[tokio::test]
async fn restarts_a_repeater_and_waits_until_it_is_back() {
    let setup = Setup::repeater().await;
    setup.sim.inject(Injection::reboot(3));
    let mut changer = setup.changer(CRITICAL);

    let plan = changer.plan_reboot().await.unwrap();
    assert_eq!(plan.device, setup.identity);
    assert_eq!(plan.model.as_deref(), Some("FRITZ!Repeater 2400"));
    assert_eq!(plan.name.as_deref(), Some("device-3"));
    assert_eq!(plan.uptime, Some(1_540_118));
    assert_eq!(plan.class, "critical: changes DeviceConfig");
    assert_eq!(plan.warning, WARNING);
    assert_eq!(plan.expires_in, 300);
    assert!(!setup.calls().contains(&"Reboot".to_owned()));

    let reboot = changer.reboot(&plan.token).await.unwrap();
    assert_eq!(reboot.report().outcome, State::Accepted);
    assert_eq!(reboot.report().uptime_before, Some(1_540_118));
    assert_eq!(setup.events(), ["planned", "sending", "applied"]);

    let mut seen = Vec::new();
    let report = changer
        .wait(&reboot, FAST, |state| seen.push(state))
        .await
        .unwrap();
    assert_eq!(seen, [State::Rebooting]);
    assert_eq!(report.outcome, State::Back);
    assert_eq!(report.device, setup.identity);
    assert_eq!(report.uptime, Some(0));
    assert_eq!(report.error, None);
    assert!(report.waited.is_some());

    // After the restart, only GetInfo is read, and the restart is sent once.
    let calls = setup.calls();
    let sent = calls.iter().position(|c| c == "Reboot").unwrap();
    assert!(
        calls[sent + 1..].iter().all(|c| c == "GetInfo"),
        "{calls:?}"
    );
    assert_eq!(calls.iter().filter(|c| *c == "Reboot").count(), 1);
}

#[tokio::test]
async fn needs_the_permit_for_critical_changes() {
    let setup = Setup::router().await;
    let mut changer = setup.changer(Permit {
        write: true,
        critical: false,
    });
    let error = changer.plan_reboot().await.unwrap_err();
    assert!(matches!(error, Error::NotPermitted { .. }), "{error:?}");
    let error = changer.reboot("token").await.unwrap_err();
    assert!(matches!(error, Error::NotPermitted { .. }), "{error:?}");
    assert!(!setup.calls().contains(&"Reboot".to_owned()));
}

#[tokio::test]
async fn needs_the_token_of_the_plan() {
    let setup = Setup::router().await;
    let mut changer = setup.changer(CRITICAL);
    let error = changer.reboot("wrong").await.unwrap_err();
    assert!(matches!(error, Error::InvalidToken), "{error:?}");
    assert!(!setup.calls().contains(&"Reboot".to_owned()));
    assert_eq!(setup.events(), ["refused"]);
}

#[tokio::test]
async fn refuses_when_the_uptime_cannot_be_read() {
    let setup = Setup::router().await;
    let mut changer = setup.changer(CRITICAL);
    let plan = changer.plan_reboot().await.unwrap();
    setup
        .sim
        .inject(Injection::fault("DeviceInfo1", "GetInfo", 501));
    let error = changer.reboot(&plan.token).await.unwrap_err();
    assert!(matches!(error, Error::Call(_)), "{error:?}");
    assert!(!setup.calls().contains(&"Reboot".to_owned()));
    assert_eq!(setup.events(), ["planned", "refused"]);
}

#[tokio::test]
async fn tells_when_it_is_unknown_whether_the_device_got_the_restart() {
    let setup = Setup::router().await;
    setup
        .sim
        .inject(Injection::disconnect("DeviceConfig1", "Reboot"));
    let mut changer = setup.changer(CRITICAL);
    let plan = changer.plan_reboot().await.unwrap();
    let reboot = changer.reboot(&plan.token).await.unwrap();
    let report = reboot.report();
    assert_eq!(report.outcome, State::Unknown);
    assert!(
        report.error.as_deref().unwrap().contains("HTTP"),
        "{report:?}"
    );
    assert_eq!(setup.events(), ["planned", "sending", "failed"]);
}

#[tokio::test]
async fn passes_on_errors_of_the_device() {
    let setup = Setup::router().await;
    setup
        .sim
        .inject(Injection::fault("DeviceConfig1", "Reboot", 501));
    let mut changer = setup.changer(CRITICAL);
    let plan = changer.plan_reboot().await.unwrap();
    let error = changer.reboot(&plan.token).await.unwrap_err();
    assert!(matches!(error, Error::Call(_)), "{error:?}");
}

#[tokio::test]
async fn times_out_when_the_device_does_not_come_back() {
    let setup = Setup::router().await;
    setup.sim.inject(Injection::reboot(u32::MAX));
    let mut changer = setup.changer(CRITICAL);
    let plan = changer.plan_reboot().await.unwrap();
    let reboot = changer.reboot(&plan.token).await.unwrap();
    let wait = Wait {
        interval: Duration::from_millis(10),
        budget: Duration::from_millis(100),
    };
    let mut seen = Vec::new();
    let report = changer
        .wait(&reboot, wait, |state| seen.push(state))
        .await
        .unwrap();
    assert_eq!(seen, [State::Rebooting]);
    assert_eq!(report.outcome, State::Timeout);
    assert_eq!(report.uptime, None);
}

#[tokio::test]
async fn times_out_when_the_device_does_not_restart() {
    let setup = Setup::router().await;
    let mut changer = setup.changer(CRITICAL);
    let plan = changer.plan_reboot().await.unwrap();
    let reboot = changer.reboot(&plan.token).await.unwrap();
    let wait = Wait {
        interval: Duration::from_millis(10),
        budget: Duration::from_millis(50),
    };
    let report = changer.wait(&reboot, wait, |_| {}).await.unwrap();
    assert_eq!(report.outcome, State::Timeout);
}

#[tokio::test]
async fn tells_when_another_device_comes_back() {
    let setup = Setup::router().await;
    setup.sim.inject(Injection::Reboot {
        down: 1,
        serial: Some("AABBCC0000FF".to_owned()),
    });
    let mut changer = setup.changer(CRITICAL);
    let plan = changer.plan_reboot().await.unwrap();
    let reboot = changer.reboot(&plan.token).await.unwrap();
    let error = changer.wait(&reboot, FAST, |_| {}).await.unwrap_err();
    let other = Identity::of(UDN, "AABBCC0000FF");
    assert!(
        matches!(&error, Error::OtherDevice(i) if *i == other),
        "{error:?}"
    );
}
