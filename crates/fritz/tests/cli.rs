//! End-to-end tests of the `fritz` binary.

use std::process::Command;

#[test]
fn prints_version() {
    let output = Command::new(env!("CARGO_BIN_EXE_fritz"))
        .arg("--version")
        .output()
        .expect("run fritz");

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("utf-8 output");
    assert_eq!(
        stdout.trim(),
        format!("fritz {}", env!("CARGO_PKG_VERSION"))
    );
}
