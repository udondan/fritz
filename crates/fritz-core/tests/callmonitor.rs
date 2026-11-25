//! The call monitor against the simulated one.

use fritz_core::callmonitor::{Monitor, PORT};
use fritz_sim::CallMonitor;

const RING: &str = "23.11.26 18:02:11;RING;0;030987;030123;SIP0;";
const CONNECT: &str = "23.11.26 18:02:15;CONNECT;0;10;030987;";
const DISCONNECT: &str = "23.11.26 18:04:40;DISCONNECT;0;145;";

fn lines(lines: &[&str]) -> Vec<String> {
    lines.iter().map(|l| (*l).to_owned()).collect()
}

#[tokio::test(start_paused = true)]
async fn connects_again_without_repeating_events() {
    let sessions = vec![
        lines(&[RING, "garbage;RING;\u{1b}[2J", CONNECT]),
        vec![],
        lines(&[CONNECT, &"9".repeat(600), DISCONNECT]),
    ];
    let sim = CallMonitor::start(sessions).await.unwrap();
    let mut monitor = Monitor::connect("[127.0.0.1]", sim.port()).await.unwrap();
    let mut notes = Vec::new();
    let mut note = |text: String| notes.push(text);
    let mut kinds = Vec::new();
    for _ in 0..3 {
        kinds.push(monitor.next(&mut note).await.kind);
    }
    assert_eq!(kinds, ["ring", "connect", "disconnect"]);
    assert_eq!(
        notes,
        [
            "the call monitor closed the connection",
            "connecting again in 1s",
            "the call monitor closed the connection",
            "connecting again in 2s",
        ]
    );
    assert_eq!(sim.connections(), 3);
}

#[tokio::test]
async fn keeps_trying_to_connect() {
    let sim = CallMonitor::start(vec![vec![], lines(&[RING])])
        .await
        .unwrap();
    let mut monitor = Monitor::connect("127.0.0.1", sim.port()).await.unwrap();
    drop(sim);
    let mut notes = Vec::new();
    let next = tokio::time::timeout(std::time::Duration::from_millis(1500), async {
        monitor.next(&mut |text| notes.push(text)).await
    });
    assert!(next.await.is_err());
    assert!(
        notes
            .iter()
            .any(|n| n.starts_with("cannot connect to the call monitor: ")),
        "{notes:?}"
    );
}

#[tokio::test]
async fn explains_how_to_turn_it_on() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let error = Monitor::connect("127.0.0.1", port).await.unwrap_err();
    let text = error.to_string();
    assert!(
        text.starts_with(&format!(
            "cannot connect to the call monitor at 127.0.0.1:{port}: "
        )),
        "{text}"
    );
    assert!(
        text.ends_with(
            "dial #96*5* on a phone connected to the device to turn it on, #96*4* turns it off"
        ),
        "{text}"
    );
    assert_eq!(PORT, 1012);
}
