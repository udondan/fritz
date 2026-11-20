//! End-to-end tests of the `fritz` binary.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use axum::Router;
use axum::http::StatusCode;
use axum::routing::{get, post};

const DESCRIPTION: &str = include_str!("../../fritz-core/tests/data/tr64desc.xml");
const DEVICE_INFO: &str = include_str!("../../fritz-core/tests/data/deviceinfoSCPD.xml");
const HOSTS: &str = include_str!("../../fritz-core/tests/data/hostsSCPD.xml");
const ONTEL: &str = include_str!("../../fritz-core/tests/data/ontelSCPD.xml");

const FINGERPRINT: &str = "00112233445566778899AABBCCDDEEFF00112233445566778899AABBCCDDEEFF";

/// Runs `fritz` with the given arguments and environment, without the
/// `FRITZ_*` variables of the caller.
fn fritz_with(args: &[&str], env: &[(&str, &str)]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_fritz"));
    for (name, _) in std::env::vars().filter(|(n, _)| n.starts_with("FRITZ_")) {
        command.env_remove(name);
    }
    command
        .args(args)
        .envs(env.iter().copied())
        .output()
        .expect("run fritz")
}

fn fritz(args: &[&str]) -> Output {
    fritz_with(args, &[])
}

async fn fritz_async(args: Vec<String>, env: Vec<(&'static str, &'static str)>) -> Output {
    tokio::task::spawn_blocking(move || {
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        fritz_with(&args, &env)
    })
    .await
    .unwrap()
}

/// How the fake device answers action calls.
#[derive(Clone)]
enum Control {
    /// It has no control endpoints, so every call fails with 404.
    Missing,
    /// It asks for an authentication scheme the client does not support.
    Unauthorized,
    /// It deletes the given directory and fails the call.
    Remove(PathBuf),
}

/// A device that serves its descriptions and answers calls as given.
async fn device(control: Control) -> String {
    let mut app = Router::new()
        .route("/tr64desc.xml", get(|| async { DESCRIPTION }))
        .route("/deviceinfoSCPD.xml", get(|| async { DEVICE_INFO }))
        .route("/hostsSCPD.xml", get(|| async { HOSTS }))
        .route("/x_contactSCPD.xml", get(|| async { ONTEL }));
    match control {
        Control::Missing => {}
        Control::Unauthorized => {
            app = app.route(
                "/upnp/control/{service}",
                post(|| async { StatusCode::UNAUTHORIZED }),
            );
        }
        Control::Remove(dir) => {
            app = app.route(
                "/upnp/control/{service}",
                post(move || async move {
                    let _ = std::fs::remove_dir_all(&dir);
                    StatusCode::NOT_FOUND
                }),
            );
        }
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    url
}

/// A URL where nothing listens.
fn unreachable_url(scheme: &str) -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("{scheme}://{}", listener.local_addr().unwrap());
    drop(listener);
    url
}

fn capture_args(url: &str, out: &Path) -> Vec<String> {
    ["capture", "--url", url, "--user", "agent", "--out"]
        .into_iter()
        .map(str::to_owned)
        .chain([out.display().to_string()])
        .collect()
}

fn stdout(output: &Output) -> String {
    String::from_utf8(output.stdout.clone()).expect("utf-8 output")
}

fn stderr(output: &Output) -> String {
    String::from_utf8(output.stderr.clone()).expect("utf-8 output")
}

#[test]
fn prints_version() {
    let output = fritz(&["--version"]);
    assert!(output.status.success());
    assert_eq!(
        stdout(&output).trim(),
        format!("fritz {}", env!("CARGO_PKG_VERSION"))
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn plans_a_capture() {
    let url = device(Control::Missing).await;
    let output = fritz_async(
        vec!["capture".into(), "--plan".into(), "--url".into(), url],
        vec![],
    )
    .await;
    assert!(output.status.success(), "{}", stderr(&output));
    let stdout = stdout(&output);
    assert!(
        stdout.contains("iterate      GetGenericHostEntry (NewIndex)"),
        "{stdout}"
    );
    assert!(stdout.contains("4 services: 5 to call"), "{stdout}");
}

#[test]
fn reports_an_unreachable_device() {
    let url = unreachable_url("http");
    let output = fritz(&["capture", "--plan", "--url", &url]);
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains(&format!("cannot read the descriptions of {url}")),
        "{}",
        stderr(&output)
    );
}

#[test]
fn pins_the_certificate_for_https() {
    let url = unreachable_url("https");
    let output = fritz(&[
        "capture",
        "--plan",
        "--url",
        &url,
        "--fingerprint",
        FINGERPRINT,
    ]);
    assert!(
        stderr(&output).contains("cannot read the descriptions"),
        "{}",
        stderr(&output)
    );
    let output = fritz(&["capture", "--plan", "--url", &url]);
    assert!(stderr(&output).contains("pin"), "{}", stderr(&output));
}

#[test]
fn needs_a_user_and_a_directory_to_capture() {
    let output = fritz(&["capture"]);
    assert!(!output.status.success());
    let stderr = stderr(&output);
    assert!(stderr.contains("--user"), "{stderr}");
    assert!(stderr.contains("--out"), "{stderr}");
}

#[test]
fn needs_the_password_in_the_environment() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("raw");
    let args = capture_args("http://127.0.0.1:9", &out);
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let output = fritz(&args);
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("set FRITZ_PASSWORD to the password of agent"),
        "{}",
        stderr(&output)
    );
    assert!(!out.exists());
}

#[test]
fn refuses_to_write_into_a_git_repository() {
    let out = Path::new(env!("CARGO_MANIFEST_DIR")).join("raw-capture");
    let args = capture_args("http://127.0.0.1:9", &out);
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let output = fritz_with(&args, &[("FRITZ_PASSWORD", "secret")]);
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("inside the git repository"),
        "{}",
        stderr(&output)
    );
    assert!(!out.exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn captures_into_a_new_directory() {
    let url = device(Control::Missing).await;
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("raw");
    let output = fritz_async(capture_args(&url, &out), vec![("FRITZ_PASSWORD", "secret")]).await;
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(
        stdout(&output),
        format!(
            "6 calls: 0 ok, 0 faults, 6 failed, saved to {}\n",
            out.display()
        )
    );
    let stderr = stderr(&output);
    assert!(
        stderr.contains("warning: the answers travel unencrypted"),
        "{stderr}"
    );
    assert!(stderr.contains("DeviceInfo1 GetInfo: failed"), "{stderr}");
    assert!(
        out.join("responses/Hosts1/GetGenericHostEntry@0.json")
            .exists()
    );

    // The directory exists now.
    let output = fritz_async(capture_args(&url, &out), vec![("FRITZ_PASSWORD", "secret")]).await;
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("cannot create"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn saves_what_it_has_when_it_stops() {
    let url = device(Control::Unauthorized).await;
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("raw");
    let output = fritz_async(capture_args(&url, &out), vec![("FRITZ_PASSWORD", "secret")]).await;
    assert!(!output.status.success());
    assert!(
        stdout(&output).starts_with("0 calls"),
        "{}",
        stdout(&output)
    );
    assert!(
        stderr(&output).contains("the capture stopped early"),
        "{}",
        stderr(&output)
    );
    assert!(out.join("tr64desc.xml").exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn reports_when_it_cannot_write() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("raw");
    let url = device(Control::Remove(out.clone())).await;
    let output = fritz_async(capture_args(&url, &out), vec![("FRITZ_PASSWORD", "secret")]).await;
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("cannot write the capture to"),
        "{}",
        stderr(&output)
    );
}
