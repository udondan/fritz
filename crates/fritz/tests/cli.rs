//! End-to-end tests of the `fritz` binary.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use axum::Router;
use axum::http::StatusCode;
use axum::routing::{get, post};
use fritz_sim::Injection;

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

const FIXTURE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../fixtures/fritzbox-7690/8.25"
);

/// The session ID of the simulator.
const SESSION: &str = "5e55104d0c0ffee5";

/// Runs `fritz tr64` against `sim` as its user, with the given password.
async fn tr64(sim: &fritz_sim::Sim, args: &[&str], password: Option<&str>) -> Output {
    let mut all = vec!["tr64"];
    all.extend(args);
    run(sim, &all, password).await
}

/// Runs `fritz` against `sim` as its user, with the given password.
async fn run(sim: &fritz_sim::Sim, args: &[&str], password: Option<&str>) -> Output {
    let mut all = strings(&["--host", sim.url().as_str(), "--user", fritz_sim::USERNAME]);
    all.extend(strings(args));
    let env = password
        .map(|p| ("FRITZ_PASSWORD".to_owned(), p.to_owned()))
        .into_iter()
        .collect();
    fritz_blocking(all, env, "").await
}

/// The output of a successful run.
fn success(output: &Output) -> String {
    assert!(output.status.success(), "{}", stderr(output));
    stdout(output)
}

/// The error of a failed run.
fn failure(output: &Output) -> String {
    assert!(!output.status.success(), "{}", stdout(output));
    stderr(output)
}

#[tokio::test(flavor = "multi_thread")]
async fn lists_and_describes_actions_without_login() {
    let fixture = fritz_sim::Fixture::load(FIXTURE).unwrap();
    let sim = fritz_sim::Sim::start(fixture).await.unwrap();

    let services = success(&tr64(&sim, &["services"], None).await);
    assert!(services.starts_with("DeviceInfo:1"), "{services}");
    assert!(
        services.contains("\nWLANConfiguration:3   ")
            && services.contains("  WLANConfiguration3\n"),
        "{services}"
    );

    let actions = success(&tr64(&sim, &["actions", "DeviceConfig:1"], None).await);
    for line in [
        "critical  Reboot\n",
        "secret    X_AVM-DE_GetConfigFile\n",
        "read      X_AVM-DE_GetUserAgreement\n",
    ] {
        assert!(actions.contains(line), "{line} in {actions}");
    }

    let describe = success(&tr64(&sim, &["describe", "WANIPConnection1", "GetInfo"], None).await);
    assert!(describe.starts_with("GetInfo (read)\n"), "{describe}");
    assert!(describe.contains("  out  NewUptime "), "{describe}");
    assert!(describe.contains("ui4 (0..4294967295)\n"), "{describe}");
    let describe =
        success(&tr64(&sim, &["describe", "Hosts1", "X_AVM-DE_HostDoUpdate"], None).await);
    assert!(
        describe.starts_with("X_AVM-DE_HostDoUpdate (critical: the name says update)\n"),
        "{describe}"
    );
    assert!(describe.contains("  in   NewMACAddress"), "{describe}");
    let describe = success(
        &tr64(
            &sim,
            &["describe", "Hosts1", "X_AVM-DE_GetHostListPath"],
            None,
        )
        .await,
    );
    assert!(describe.contains("string\n"), "{describe}");

    let error = failure(&tr64(&sim, &["describe", "Hosts1", "Nothing"], None).await);
    assert!(error.contains("has no action Nothing"), "{error}");
    let error = failure(&tr64(&sim, &["actions", "Nothing1"], None).await);
    assert!(
        error.contains("cannot read the service Nothing1"),
        "{error}"
    );
    assert!(sim.requests().iter().all(|r| r.action.is_none()));
}

#[tokio::test(flavor = "multi_thread")]
async fn calls_only_actions_that_read() {
    let fixture = fritz_sim::Fixture::load(FIXTURE).unwrap();
    let sim = fritz_sim::Sim::start(fixture).await.unwrap();
    let password = Some(fritz_sim::PASSWORD);

    let info = success(&tr64(&sim, &["call", "DeviceInfo1", "GetInfo"], password).await);
    assert!(
        info.contains("NewModelName         FRITZ!Box 7690\n"),
        "{info}"
    );
    let path = success(
        &tr64(
            &sim,
            &["call", "Hosts1", "X_AVM-DE_GetHostListPath"],
            password,
        )
        .await,
    );
    assert_eq!(
        path,
        "NewX_AVM-DE_HostListPath  /devicehostlist.lua?sid=***\n"
    );
    let entry = tr64(
        &sim,
        &["call", "Hosts1", "GetGenericHostEntry", "NewIndex=0"],
        password,
    );
    assert!(success(&entry.await).contains("NewActive"));

    // Refused before the password is needed and before anything is sent.
    for (args, message) in [
        (
            &["call", "DeviceConfig1", "Reboot"][..],
            "Reboot is critical: changes DeviceConfig; only actions that read are called",
        ),
        (
            &["call", "WLANConfiguration1", "GetSecurityKeys"],
            "GetSecurityKeys is secret: the name points to a secret",
        ),
        (
            &[
                "call",
                "Hosts1",
                "X_AVM-DE_SetFriendlyName",
                "NewX_AVM-DE_FriendlyName=x",
            ],
            "X_AVM-DE_SetFriendlyName is write; only actions that read are called",
        ),
        (
            &["call", "Hosts1", "GetGenericHostEntry"],
            "GetGenericHostEntry needs the input NewIndex",
        ),
        (
            &[
                "call",
                "Hosts1",
                "GetGenericHostEntry",
                "NewIndex=0",
                "Other=1",
            ],
            "GetGenericHostEntry has no input Other",
        ),
        (
            &["call", "Hosts1", "GetGenericHostEntry", "NewIndex"],
            "NewIndex is not NAME=VALUE",
        ),
    ] {
        let error = failure(&tr64(&sim, args, None).await);
        assert!(error.contains(message), "{message} in {error}");
    }
    let called: Vec<String> = sim
        .requests()
        .into_iter()
        .filter(|r| r.authorized)
        .filter_map(|r| r.action)
        .collect();
    assert_eq!(
        called,
        ["GetInfo", "X_AVM-DE_GetHostListPath", "GetGenericHostEntry"]
    );

    let error = failure(&tr64(&sim, &["call", "DeviceInfo1", "GetInfo"], Some("wrong")).await);
    assert!(error.contains("GetInfo failed"), "{error}");
    let error = failure(&tr64(&sim, &["call", "DeviceInfo1", "GetInfo"], None).await);
    assert!(error.contains("no password for"), "{error}");
}

#[test]
fn reports_an_unreachable_device_for_tr64() {
    let url = unreachable_url("http");
    let output = fritz(&["--host", &url, "tr64", "services"]);
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("cannot read the device description"),
        "{}",
        stderr(&output)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn prints_the_state_as_json() {
    let fixture = fritz_sim::Fixture::load(FIXTURE).unwrap();
    let sim = fritz_sim::Sim::start(fixture).await.unwrap();
    let password = Some(fritz_sim::PASSWORD);
    let json =
        |output: &Output| -> serde_json::Value { serde_json::from_str(&success(output)).unwrap() };

    // JSON is the default when the output is not a terminal.
    let overview = json(&run(&sim, &["overview"], password).await);
    assert_eq!(overview["model"], "FRITZ!Box 7690");
    assert_eq!(overview["internet"]["status"], "Connected");
    let internet = json(&run(&sim, &["--output", "json", "internet"], password).await);
    assert_eq!(internet["provider"], "provider-1");
    assert_eq!(internet["dsl"]["downstream_noise_margin"], 7.0);
    let wlan = json(&run(&sim, &["wlan"], password).await);
    assert_eq!(wlan.as_array().unwrap().len(), 3);
    let ports = json(&run(&sim, &["ports"], password).await);
    assert_eq!(ports["upnp"], true);
    let telephony = json(&run(&sim, &["telephony"], password).await);
    assert_eq!(telephony["handsets"][0]["model"], "FRITZ!Fon C5");
    let capabilities = json(&run(&sim, &["capabilities"], password).await);
    assert_eq!(capabilities["services"], 37);

    let output = run(&sim, &["hosts"], password).await;
    let hosts = json(&output);
    assert_eq!(hosts["hosts"].as_array().unwrap().len(), 50);
    assert_eq!(hosts["hosts"][1]["speed"], 866);
    assert_eq!(stderr(&output), "");
    let active = json(&run(&sim, &["hosts", "--active"], password).await);
    let active = active["hosts"].as_array().unwrap();
    assert!(!active.is_empty() && active.len() < 50, "{active:?}");
    assert!(active.iter().all(|h| h["active"] == true));
    let mesh = json(&run(&sim, &["mesh"], password).await);
    assert_eq!(mesh["nodes"][0]["name"], "device-50");
    assert_eq!(mesh["links"].as_array().unwrap().len(), 3);
    let output = run(&sim, &["smarthome"], password).await;
    let devices = json(&output);
    assert_eq!(devices.as_array().unwrap().len(), 4);
    assert_eq!(devices[0]["units"][0]["power"], 23.45);
    assert_eq!(devices[1]["units"][0]["target"], 21.0);
    // The session ID stays in memory, and the session is closed again.
    assert!(!stdout(&output).contains(SESSION) && !stderr(&output).contains(SESSION));
    let logouts = sim
        .requests()
        .iter()
        .filter(|r| r.method == "POST" && r.path == "/login_sid.lua")
        .count();
    assert_eq!(logouts, 2, "a login and a logout");

    // Without the host list, the hosts are read one by one.
    sim.inject(Injection::fault("Hosts1", "X_AVM-DE_GetHostListPath", 606));
    let output = run(&sim, &["hosts"], password).await;
    assert_eq!(json(&output)["hosts"].as_array().unwrap().len(), 50);
    assert!(stderr(&output).contains("the device reported 161 hosts, read 50"));
}

#[tokio::test(flavor = "multi_thread")]
async fn prints_the_settings() {
    let fixture = fritz_sim::Fixture::load(FIXTURE).unwrap();
    let sim = fritz_sim::Sim::start(fixture).await.unwrap();
    let password = Some(fritz_sim::PASSWORD);
    let output = run(&sim, &["settings"], password).await;
    let settings: serde_json::Value = serde_json::from_str(&success(&output)).unwrap();
    assert_eq!(settings["modules"]["ipv6"]["mtu_override"], "1492");
    assert_eq!(stderr(&output), "");
    let output = run(
        &sim,
        &["--output", "text", "settings", "wlan", "none"],
        password,
    )
    .await;
    let text = stdout(&output);
    assert!(text.contains("wlan.pskvalue        ***\n"), "{text}");
    assert!(
        text.contains("none                 unavailable: "),
        "{text}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn prints_the_personal_lists() {
    let log = fritz_sim::Answer::Ok(
        [(
            "NewDeviceLog".to_owned(),
            "20.11.26 18:05:42 DSL is ready.".to_owned(),
        )]
        .into(),
    );
    let fixture = fritz_sim::Fixture::load(FIXTURE).unwrap().with_answer(
        "DeviceInfo1",
        "GetDeviceLog",
        None,
        log,
    );
    let sim = fritz_sim::Sim::start(fixture).await.unwrap();
    let password = Some(fritz_sim::PASSWORD);
    let output = run(&sim, &["calls", "--limit", "2"], password).await;
    let calls: serde_json::Value = serde_json::from_str(&success(&output)).unwrap();
    assert_eq!(calls[0]["kind"], "missed", "{calls}");
    assert_eq!(calls.as_array().unwrap().len(), 2);
    let output = run(&sim, &["--output", "text", "calls"], password).await;
    assert!(stdout(&output).contains("2026-11-19 20:01  incoming"));

    let output = run(&sim, &["phonebook", "phone-4"], password).await;
    let books: serde_json::Value = serde_json::from_str(&success(&output)).unwrap();
    assert_eq!(books[0]["contacts"][0]["name"], "name-6", "{books}");
    assert_eq!(books[0]["contacts"].as_array().unwrap().len(), 1);
    let output = run(&sim, &["--output", "text", "phonebook"], password).await;
    assert!(stdout(&output).contains("name-5     name-8  phone-5  intern"));

    let output = run(&sim, &["--output", "text", "log"], password).await;
    assert_eq!(
        success(&output),
        "TIME                 MESSAGE\n2026-11-20 18:05:42  DSL is ready.\n"
    );
    let output = run(&sim, &["log", "--limit", "1"], password).await;
    let log: serde_json::Value = serde_json::from_str(&success(&output)).unwrap();
    assert_eq!(log[0]["message"], "DSL is ready.", "{log}");
}

#[tokio::test(flavor = "multi_thread")]
async fn prints_the_state_as_text() {
    let fixture = fritz_sim::Fixture::load(FIXTURE).unwrap();
    let sim = fritz_sim::Sim::start(fixture).await.unwrap();
    let password = Some(fritz_sim::PASSWORD);
    let text =
        async |command: &str| success(&run(&sim, &["--output", "text", command], password).await);

    let overview = text("overview").await;
    assert!(
        overview.starts_with("Model       FRITZ!Box 7690\n"),
        "{overview}"
    );
    assert!(overview.contains("Uptime      8d 19h 13m\n"), "{overview}");
    assert!(
        overview.contains("Internet    Connected over DSL (link Up), 2001:db8::1, for 9h 36m\n"),
        "{overview}"
    );
    let internet = text("internet").await;
    assert!(
        internet.contains("DSL rate       ↓ 274.8 Mbit/s  ↑ 45.9 Mbit/s\n"),
        "{internet}"
    );
    let wlan = text("wlan").await;
    assert!(
        wlan.starts_with("SERVICE             SSID    BAND     CHANNEL"),
        "{wlan}"
    );
    assert!(
        wlan.contains("WLANConfiguration1  wlan-1  2.4 GHz  6        be        Up        4\n"),
        "{wlan}"
    );
    let hosts = text("hosts").await;
    assert!(hosts.starts_with("NAME "), "{hosts}");
    assert!(hosts.contains("device-1 "), "{hosts}");
    let mesh = text("mesh").await;
    assert!(mesh.starts_with("NODE "), "{mesh}");
    assert!(mesh.contains("\nFROM "), "{mesh}");
    assert!(mesh.contains("device-50  device-51"), "{mesh}");
    let smarthome = text("smarthome").await;
    assert!(smarthome.starts_with("NAME "), "{smarthome}");
    assert!(smarthome.contains("23.4 W"), "{smarthome}");
    assert!(smarthome.contains("80%"), "{smarthome}");
    let ports = text("ports").await;
    assert_eq!(ports, "UPnP  on\nNo port forwardings.\n");
    let telephony = text("telephony").await;
    assert!(telephony.starts_with("NUMBER "), "{telephony}");
    assert!(telephony.contains("\nHANDSET "), "{telephony}");
    let capabilities = text("capabilities").await;
    assert!(capabilities.starts_with("Features "), "{capabilities}");
    assert!(capabilities.contains("dect"), "{capabilities}");
}

#[tokio::test(flavor = "multi_thread")]
async fn reports_failed_reads() {
    let fixture = fritz_sim::Fixture::load(FIXTURE).unwrap();
    let sim = fritz_sim::Sim::start(fixture).await.unwrap();
    for (command, message) in [
        ("overview", "cannot read the overview"),
        ("internet", "cannot read the internet connection"),
        ("wlan", "cannot read the wireless networks"),
        ("hosts", "cannot read the hosts"),
        ("mesh", "cannot read the mesh"),
        ("smarthome", "cannot log in to the web interface"),
        ("ports", "cannot read the port forwardings"),
        ("telephony", "cannot read the telephony"),
        ("capabilities", "cannot read the capabilities"),
    ] {
        let error = failure(&run(&sim, &[command], Some("wrong")).await);
        assert!(error.contains(message), "{message} in {error}");
    }
    let error = failure(&run(&sim, &["overview"], None).await);
    assert!(error.contains("no password for"), "{error}");
    let error = failure(&run(&sim, &["smarthome"], Some("wrong")).await);
    assert!(
        error.contains("the device rejected the user or the password"),
        "{error}"
    );
    sim.inject(Injection::fault("Hosts1", "X_AVM-DE_GetMeshListPath", 606));
    let error = failure(&run(&sim, &["mesh"], Some(fritz_sim::PASSWORD)).await);
    assert!(error.contains("the device has no mesh list"), "{error}");
    let url = unreachable_url("http");
    let output = fritz_with(
        &["--host", &url, "--user", "agent", "overview"],
        &[("FRITZ_PASSWORD", "x")],
    );
    let error = failure(&output);
    assert!(
        error.contains("cannot read the device description"),
        "{error}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn reports_second_factor_faults() {
    let fixture = fritz_sim::Fixture::load(FIXTURE).unwrap();
    let sim = fritz_sim::Sim::start(fixture).await.unwrap();
    let password = Some(fritz_sim::PASSWORD);

    // A field the user may not read is left out.
    sim.inject(Injection::fault("UserInterface1", "GetInfo", 866));
    let overview = success(&run(&sim, &["overview"], password).await);
    assert!(overview.contains("FRITZ!Box 7690"), "{overview}");

    // A blocked or busy second factor stops the read instead.
    sim.clear_injections();
    sim.inject(Injection::fault("UserInterface1", "GetInfo", 867));
    let error = failure(&run(&sim, &["overview"], password).await);
    assert!(
        error.contains(
            "blocks confirmations after failed ones; try again in up to an hour (UPnP error 867)"
        ),
        "{error}"
    );
    sim.clear_injections();
    sim.inject(Injection::fault("DeviceInfo1", "GetInfo", 868));
    let error = failure(&run(&sim, &["overview"], password).await);
    assert!(error.contains("(UPnP error 868)"), "{error}");

    sim.clear_injections();
    sim.inject(Injection::fault("DeviceInfo1", "GetInfo", 866));
    let error = failure(&tr64(&sim, &["call", "DeviceInfo1", "GetInfo"], password).await);
    assert!(
        error.contains("confirmed with a second factor, which fritz never does (UPnP error 866)"),
        "{error}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn reports_a_missing_smart_home_api() {
    // A device without the REST API, like one before FRITZ!OS 8.20.
    let dir = tempfile::tempdir().unwrap();
    std::fs::copy(
        Path::new(FIXTURE).join("tr64desc.xml"),
        dir.path().join("tr64desc.xml"),
    )
    .unwrap();
    std::fs::create_dir(dir.path().join("scpd")).unwrap();
    std::fs::create_dir(dir.path().join("responses")).unwrap();
    let fixture = fritz_sim::Fixture::load(dir.path()).unwrap();
    let sim = fritz_sim::Sim::start(fixture).await.unwrap();
    let error = failure(&run(&sim, &["smarthome"], Some(fritz_sim::PASSWORD)).await);
    assert!(
        error.contains("cannot read the Smart Home devices"),
        "{error}"
    );
    assert!(!error.contains(SESSION), "{error}");
    // The session is closed anyway.
    assert_eq!(sim.requests().last().unwrap().path, "/login_sid.lua");
}
