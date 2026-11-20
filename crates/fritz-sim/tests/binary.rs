//! The `fritz-sim` binary.
#![cfg(unix)]

use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};

use tr064::{Client, Url};

const FIXTURE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../fixtures/fritzbox-7690/8.25"
);

#[tokio::test]
async fn serves_a_fixture_until_interrupted() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_fritz-sim"))
        .args(["--fixture", FIXTURE, "--port", "0"])
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut line = String::new();
    BufReader::new(child.stdout.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    let url = line.trim().strip_prefix("listening on ").unwrap();

    let client = Client::builder(Url::parse(url).unwrap()).build().unwrap();
    let description = client.description().await.unwrap();
    assert!(description.service("Hosts1").is_ok());

    let interrupted = Command::new("kill")
        .args(["-INT", &child.id().to_string()])
        .status()
        .unwrap();
    assert!(interrupted.success());
    assert!(child.wait().unwrap().success());
}

#[test]
fn fails_without_a_fixture() {
    let output = Command::new(env!("CARGO_BIN_EXE_fritz-sim"))
        .args(["--fixture", "/nonexistent"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("cannot read /nonexistent/tr64desc.xml"),
        "{stderr}"
    );
}
