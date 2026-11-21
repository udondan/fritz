//! End-to-end tests of `fritz fixture`.

use std::fs;
use std::path::Path;
use std::process::{Command, Output};

/// Runs `fritz fixture` with the given arguments in `dir`.
fn fixture(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_fritz"))
        .arg("fixture")
        .args(args)
        .current_dir(dir)
        .output()
        .expect("run fritz")
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// Writes a raw capture with one answer into `dir/raw`.
fn raw(dir: &Path, ssid: &str) {
    let service = dir.join("raw/responses/WLANConfiguration1");
    fs::create_dir_all(&service).unwrap();
    fs::write(
        service.join("GetInfo.json"),
        format!(r#"{{"status": "ok", "out": {{"NewSSID": "{ssid}"}}}}"#),
    )
    .unwrap();
}

#[test]
fn anonymizes_a_capture_into_a_clean_fixture() {
    let dir = tempfile::tempdir().unwrap();
    raw(dir.path(), "Home");

    let output = fixture(dir.path(), &["anonymize", "raw", "--out", "fixtures/box"]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(stdout(&output), "anonymized 1 files into fixtures/box\n");

    let output = fixture(dir.path(), &["check"]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(stdout(&output), "no personal values found in fixtures\n");
}

#[test]
fn writes_nothing_when_private_patterns_remain() {
    let dir = tempfile::tempdir().unwrap();
    raw(dir.path(), "Home");
    fs::write(dir.path().join("own.txt"), "status\n").unwrap();

    let args = [
        "anonymize",
        "raw",
        "--out",
        "out",
        "--private-patterns",
        "own.txt",
    ];
    let output = fixture(dir.path(), &args);
    assert!(!output.status.success());
    assert_eq!(
        stderr(&output),
        "responses/WLANConfiguration1/GetInfo.json: private pattern\n\
         Error: personal values remain, nothing was written\n"
    );
    assert!(!dir.path().join("out").exists());
}

#[test]
fn reports_errors_of_anonymizing() {
    let dir = tempfile::tempdir().unwrap();
    let output = fixture(dir.path(), &["anonymize", "missing", "--out", "out"]);
    assert!(!output.status.success());
    assert!(
        stderr(&output).starts_with("Error: cannot read or write missing"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn reports_personal_values_without_naming_them() {
    let dir = tempfile::tempdir().unwrap();
    raw(dir.path(), "Home");
    fs::write(dir.path().join(".private-patterns"), "# own\nok\n").unwrap();

    let output = fixture(dir.path(), &["check", "raw"]);
    assert!(!output.status.success());
    assert_eq!(
        stderr(&output),
        "responses/WLANConfiguration1/GetInfo.json: WLAN name\n\
         responses/WLANConfiguration1/GetInfo.json: private pattern\n\
         Error: 2 personal values found in raw\n"
    );
    assert!(!stderr(&output).contains("Home"));
}

#[test]
fn reports_a_missing_pattern_file() {
    let dir = tempfile::tempdir().unwrap();
    let output = fixture(dir.path(), &["check", "--private-patterns", "missing"]);
    assert!(!output.status.success());
    assert!(
        stderr(&output).starts_with("Error: cannot read missing"),
        "{}",
        stderr(&output)
    );
}
