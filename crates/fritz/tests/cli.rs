//! End-to-end tests of the `fritz` binary.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use axum::Router;
use axum::http::StatusCode;
use axum::routing::{get, post};

const DESCRIPTION: &str = include_str!("../../fritz-core/tests/data/tr64desc.xml");
const DEVICE_INFO: &str = include_str!("../../fritz-core/tests/data/deviceinfoSCPD.xml");
const HOSTS: &str = include_str!("../../fritz-core/tests/data/hostsSCPD.xml");
const ONTEL: &str = include_str!("../../fritz-core/tests/data/ontelSCPD.xml");

const FINGERPRINT: &str = "00112233445566778899AABBCCDDEEFF00112233445566778899AABBCCDDEEFF";

/// Runs `fritz` with the given arguments, environment and input, without the
/// `FRITZ_*` variables of the caller. The keychain is off, and the
/// configuration file does not exist unless `FRITZ_CONFIG` is given.
fn fritz_input(args: &[&str], env: &[(&str, &str)], input: &str) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_fritz"));
    for (name, _) in std::env::vars().filter(|(n, _)| n.starts_with("FRITZ_")) {
        command.env_remove(name);
    }
    let missing = Path::new(env!("CARGO_TARGET_TMPDIR")).join("missing/config.toml");
    let mut child = command
        .args(args)
        .env("FRITZ_NO_KEYCHAIN", "1")
        .env("FRITZ_CONFIG", missing)
        .envs(env.iter().copied())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("run fritz");
    // fritz may exit without reading the input.
    let _ = child.stdin.take().unwrap().write_all(input.as_bytes());
    child.wait_with_output().expect("run fritz")
}

fn fritz_with(args: &[&str], env: &[(&str, &str)]) -> Output {
    fritz_input(args, env, "")
}

fn fritz(args: &[&str]) -> Output {
    fritz_with(args, &[])
}

async fn fritz_async(args: Vec<String>, env: Vec<(&'static str, &'static str)>) -> Output {
    let env = env.into_iter().map(|(n, v)| (n.to_owned(), v.to_owned()));
    fritz_blocking(args, env.collect(), "").await
}

/// Runs `fritz` off the runtime, so a device on the runtime can answer.
async fn fritz_blocking(args: Vec<String>, env: Vec<(String, String)>, input: &str) -> Output {
    let input = input.to_owned();
    tokio::task::spawn_blocking(move || {
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        let env: Vec<(&str, &str)> = env.iter().map(|(n, v)| (n.as_str(), v.as_str())).collect();
        fritz_input(&args, &env, &input)
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
    ["capture", "--host", url, "--user", "agent", "--out"]
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
        vec!["capture".into(), "--plan".into(), "--host".into(), url],
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
    let output = fritz(&["capture", "--plan", "--host", &url]);
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
        "--host",
        &url,
        "--fingerprint",
        FINGERPRINT,
    ]);
    assert!(
        stderr(&output).contains("cannot read the descriptions"),
        "{}",
        stderr(&output)
    );
    let output = fritz(&["capture", "--plan", "--host", &url]);
    assert!(
        stderr(&output).contains("run `fritz tls trust`"),
        "{}",
        stderr(&output)
    );

    // A capture over https needs no warning.
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("raw");
    let mut args = capture_args(&url, &out);
    args.extend(["--fingerprint".to_owned(), FINGERPRINT.to_owned()]);
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let output = fritz_with(&args, &[("FRITZ_PASSWORD", "secret")]);
    let stderr = stderr(&output);
    assert!(stderr.contains("cannot read the descriptions"), "{stderr}");
    assert!(!stderr.contains("warning"), "{stderr}");
    assert!(!out.exists());
}

#[test]
fn needs_a_user_and_a_directory_to_capture() {
    let output = fritz(&["capture"]);
    assert!(!output.status.success());
    assert!(stderr(&output).contains("--out"), "{}", stderr(&output));

    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("raw");
    let args = ["capture", "--host", "http://127.0.0.1:9", "--out"];
    let output = fritz(&[&args[..], &[out.to_str().unwrap()]].concat());
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("no user is set for http://127.0.0.1:9/"),
        "{}",
        stderr(&output)
    );
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
        stderr(&output).contains(
            "no password for agent@127.0.0.1, run `fritz auth login` or set FRITZ_PASSWORD"
        ),
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

/// A configuration file in a temporary directory.
struct Config {
    _dir: tempfile::TempDir,
    path: String,
}

impl Config {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("fritz/config.toml").display().to_string();
        Self { _dir: dir, path }
    }

    fn env(&self) -> Vec<(String, String)> {
        vec![("FRITZ_CONFIG".to_owned(), self.path.clone())]
    }

    fn run(&self, args: &[&str]) -> Output {
        fritz_with(args, &[("FRITZ_CONFIG", &self.path)])
    }

    fn text(&self) -> String {
        std::fs::read_to_string(&self.path).unwrap()
    }
}

fn strings(args: &[&str]) -> Vec<String> {
    args.iter().copied().map(str::to_owned).collect()
}

#[test]
fn creates_and_shows_profiles() {
    let config = Config::new();
    let output = config.run(&[
        "config",
        "init",
        "--host",
        "http://127.0.0.1:49000",
        "--user",
        "agent",
    ]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(
        stdout(&output),
        format!("saved the profile default to {}\n", config.path)
    );
    let output = config.run(&[
        "--profile",
        "lab",
        "config",
        "init",
        "--host",
        "lab.example",
    ]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(
        config.text(),
        "default_profile = \"default\"\n\n\
         [profile.default]\nhost = \"http://127.0.0.1:49000\"\nusername = \"agent\"\n\n\
         [profile.lab]\nhost = \"lab.example\"\n"
    );

    let output = fritz_with(
        &["config", "show"],
        &[("FRITZ_CONFIG", &config.path), ("FRITZ_PASSWORD", "secret")],
    );
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(
        stdout(&output),
        format!(
            "config       {}\n\
             profile      default\n\
             address      http://127.0.0.1:49000/\n\
             user         agent\n\
             certificate  none, the connection is not encrypted\n\
             password     from the environment variable FRITZ_PASSWORD\n",
            config.path
        )
    );

    let output = config.run(&["--profile", "lab", "config", "show"]);
    assert!(output.status.success(), "{}", stderr(&output));
    let stdout = stdout(&output);
    assert!(
        stdout.contains("address      https://lab.example:49443/\n"),
        "{stdout}"
    );
    assert!(stdout.contains("user         none\n"), "{stdout}");
    assert!(stdout.contains("password     no user is set"), "{stdout}");

    // Without a host, the profile names the default one.
    let output = config.run(&["--profile", "bare", "config", "init"]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(
        config
            .text()
            .contains("[profile.bare]\nhost = \"fritz.box\"\n"),
        "{}",
        config.text()
    );

    let output = config.run(&["--profile", "missing", "config", "show"]);
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("there is no profile named missing"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn shows_the_defaults_without_a_file() {
    let output = fritz(&["config", "show"]);
    assert!(output.status.success(), "{}", stderr(&output));
    let stdout = stdout(&output);
    assert!(stdout.contains("profile      none\n"), "{stdout}");
    assert!(
        stdout.contains("address      https://fritz.box:49443/\n"),
        "{stdout}"
    );
    assert!(
        stdout.contains("certificate  not trusted, run `fritz tls trust`\n"),
        "{stdout}"
    );
}

#[test]
fn reports_an_invalid_configuration() {
    let config = Config::new();
    std::fs::create_dir_all(Path::new(&config.path).parent().unwrap()).unwrap();
    std::fs::write(&config.path, "unknown = 1\n").unwrap();
    let output = config.run(&["config", "show"]);
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("is not a valid configuration"),
        "{}",
        stderr(&output)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn trusts_the_certificate_on_first_use() {
    let fixture = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/fritzbox-7690/8.25"
    );
    let options = fritz_sim::Options {
        https: true,
        ..fritz_sim::Options::default()
    };
    let sim = fritz_sim::Sim::start_with(fritz_sim::Fixture::load(fixture).unwrap(), options)
        .await
        .unwrap();
    let host = sim.url().as_str().to_owned();
    let found = sim.fingerprint().unwrap().to_string();
    let config = Config::new();
    let run = |args: &[&str], input: &'static str| {
        let mut all = strings(&["--host", &host]);
        all.extend(strings(args));
        fritz_blocking(all, config.env(), input)
    };

    let output = run(&["tls", "fingerprint"], "").await;
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(stdout(&output), format!("{found}\n"));

    let output = run(&["tls", "trust"], "n\n").await;
    assert!(!output.status.success());
    let error = stderr(&output);
    assert!(
        error.contains(&format!("presents the certificate\n  {found}\n")),
        "{error}"
    );
    assert!(error.contains("the certificate was not trusted"), "{error}");
    assert!(!Path::new(&config.path).exists());

    let output = run(&["tls", "trust"], "y\n").await;
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(
        stdout(&output),
        format!("the profile default trusts the certificate {found} of {host}\n")
    );
    assert!(config.text().contains(&found), "{}", config.text());

    let output = run(&["tls", "trust"], "").await;
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(
        stdout(&output),
        format!("the certificate of {host} is already trusted\n")
    );

    // The profile pins the certificate for later commands.
    let output = fritz_blocking(strings(&["capture", "--plan"]), config.env(), "").await;
    assert!(output.status.success(), "{}", stderr(&output));
    let output = fritz_blocking(strings(&["config", "show"]), config.env(), "").await;
    assert!(
        stdout(&output).contains(&format!("certificate  {found}\n")),
        "{}",
        stdout(&output)
    );

    let output = run(&["tls", "trust", "--fingerprint", FINGERPRINT], "").await;
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains(&format!("{host} presents the certificate {found}, not ")),
        "{}",
        stderr(&output)
    );
    let output = run(&["tls", "trust", "--fingerprint", &found], "").await;
    assert!(output.status.success(), "{}", stderr(&output));

    // Another certificate was trusted before.
    let other = format!("{}00", "00:".repeat(31));
    let text = config.text().replace(&found, &other);
    std::fs::write(&config.path, text).unwrap();
    let output = fritz_blocking(strings(&["tls", "trust"]), config.env(), "yes\n").await;
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(
        stderr(&output).contains(&format!("warning: the certificate of {host} changed")),
        "{}",
        stderr(&output)
    );
    assert!(config.text().contains(&found), "{}", config.text());

    let output = fritz_blocking(strings(&["tls", "forget"]), config.env(), "").await;
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(
        stdout(&output),
        "the profile default no longer trusts a certificate\n"
    );
    let output = fritz_blocking(strings(&["tls", "forget"]), config.env(), "").await;
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("the profile default trusts no certificate"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn has_no_certificate_without_https() {
    let output = fritz(&["--host", "http://127.0.0.1:9", "tls", "fingerprint"]);
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("http://127.0.0.1:9/ is not https, so it has no certificate"),
        "{}",
        stderr(&output)
    );
    let url = unreachable_url("https");
    let output = fritz(&["--host", &url, "tls", "trust"]);
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("cannot get the certificate of"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn reports_where_the_password_comes_from() {
    let args = ["--host", "192.168.178.1", "--user", "agent", "auth"];
    let output = fritz_with(
        &[&args[..], &["status"]].concat(),
        &[("FRITZ_PASSWORD", "secret")],
    );
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(
        stdout(&output),
        "the password of agent@192.168.178.1 comes from the environment variable FRITZ_PASSWORD\n"
    );

    let output = fritz(&[&args[..], &["status"]].concat());
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("no password for agent@192.168.178.1"),
        "{}",
        stderr(&output)
    );

    for command in ["login", "logout"] {
        let output = fritz_input(&[&args[..], &[command]].concat(), &[], "secret\n");
        assert!(!output.status.success());
        assert!(
            stderr(&output).contains("the keychain is disabled by FRITZ_NO_KEYCHAIN"),
            "{}",
            stderr(&output)
        );
    }
}
