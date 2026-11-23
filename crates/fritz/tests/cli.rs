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

#[test]
fn prints_the_build() {
    let output = fritz(&["version", "--output", "json"]);
    assert!(output.status.success(), "{}", stderr(&output));
    let build: serde_json::Value = serde_json::from_str(&stdout(&output)).unwrap();
    assert_eq!(build["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(
        build["features"],
        serde_json::json!(if cfg!(feature = "write") {
            vec!["write"]
        } else {
            Vec::<&str>::new()
        })
    );
    assert!(build["target"].as_str().is_some_and(|t| !t.is_empty()));
    assert!(build.get("latest").is_none());
    let text = stdout(&fritz(&["version", "--output", "text"]));
    assert!(
        text.starts_with(&format!("Version   {}\n", env!("CARGO_PKG_VERSION"))),
        "{text}"
    );
    let yaml = stdout(&fritz(&["version", "--output", "yaml"]));
    assert!(
        yaml.starts_with(&format!("version: \"{}\"\n", env!("CARGO_PKG_VERSION"))),
        "{yaml}"
    );
}

/// Runs `fritz version --check` against a fake GitHub API that lists a
/// release with the given tag.
async fn check_version(tag: &str) -> Output {
    let releases = serde_json::json!([{
        "tag_name": tag,
        "html_url": format!("https://github.com/udondan/fritz/releases/tag/{tag}"),
        "body": "## Notes\n\n* a fix",
        "prerelease": false,
        "draft": false,
    }])
    .to_string();
    let app = Router::new().route("/releases", get(move || async move { releases }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/releases", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let args = strings(&["version", "--check", "--output", "json"]);
    fritz_blocking(args, vec![("FRITZ_RELEASES_URL".into(), url)], "").await
}

#[tokio::test(flavor = "multi_thread")]
async fn checks_for_a_newer_version() {
    let output = check_version("fritz-v999.0.0").await;
    assert_eq!(output.status.code(), Some(10), "{}", stderr(&output));
    let report: serde_json::Value = serde_json::from_str(&stdout(&output)).unwrap();
    assert_eq!(report["latest"]["version"], "999.0.0");
    assert_eq!(report["latest"]["newer"], true);
    assert_eq!(
        report["latest"]["notes"],
        serde_json::json!(["## Notes", "* a fix"])
    );

    let current = format!("fritz-v{}", env!("CARGO_PKG_VERSION"));
    let output = check_version(&current).await;
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let report: serde_json::Value = serde_json::from_str(&stdout(&output)).unwrap();
    assert_eq!(report["latest"]["newer"], false);

    let output = check_version("tr064-v999.0.0").await;
    assert_eq!(output.status.code(), Some(1));
    assert!(
        stderr(&output).contains("no release of fritz"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn checks_only_over_https() {
    let output = fritz_with(
        &["version", "--check"],
        &[("FRITZ_RELEASES_URL", "http://example.com/releases")],
    );
    assert_eq!(output.status.code(), Some(1));
    assert!(
        stderr(&output).contains("must be an https URL"),
        "{}",
        stderr(&output)
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
fn needs_a_directory_and_a_password_to_capture() {
    let output = fritz(&["capture"]);
    assert!(!output.status.success());
    assert!(stderr(&output).contains("--out"), "{}", stderr(&output));

    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("raw");
    let args = ["capture", "--host", "http://127.0.0.1:9", "--out"];
    let output = fritz(&[&args[..], &[out.to_str().unwrap()]].concat());
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("no password for 127.0.0.1"),
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
    assert!(
        stdout.contains("user         none, for devices with only a password\n"),
        "{stdout}"
    );
    assert!(
        stdout.contains("password     no password for lab.example,"),
        "{stdout}"
    );

    let output = config.run(&["config", "list", "--output", "text"]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "DEFAULT  PROFILE  HOST                    USER   TRUSTED\n\
         *        default  http://127.0.0.1:49000  agent  no\n\
         \x20        lab      lab.example             -      no\n"
    );

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
fn lists_the_profiles() {
    let output = fritz(&["config", "list", "--output", "text"]);
    assert!(output.status.success(), "{}", stderr(&output));
    let stdout = stdout(&output);
    assert!(stdout.starts_with("no profiles in "), "{stdout}");
    assert!(stdout.ends_with(", run `fritz config init`\n"), "{stdout}");
    let output = fritz(&["config", "list", "--output", "json"]);
    assert_eq!(success(&output), "[]\n");
    // Text in a pipe, set once in the environment.
    let output = fritz_with(&["config", "list"], &[("FRITZ_OUTPUT", "text")]);
    assert!(success(&output).starts_with("no profiles in "));
    // The flag wins over the environment.
    let output = fritz_with(
        &["config", "list", "-o", "json"],
        &[("FRITZ_OUTPUT", "text")],
    );
    assert_eq!(success(&output), "[]\n");

    let config = Config::new();
    let output = config.run(&["config", "init", "--host", "192.168.178.1"]);
    assert!(output.status.success(), "{}", stderr(&output));
    let output = config.run(&["config", "list", "--output", "json"]);
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        value,
        serde_json::json!([{"name": "default", "default": true, "host": "192.168.178.1",
                            "username": null, "fingerprint": null}])
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
    // With YAML, as YAML.
    let output = config.run(&["--output", "yaml", "overview"]);
    assert_eq!(output.status.code(), Some(1));
    let error: serde_json::Value = serde_saphyr::from_str(&stderr(&output)).unwrap();
    let message = error["error"].as_str().unwrap();
    assert!(
        message.contains("is not a valid configuration"),
        "{message}"
    );
}

const SHELLS: [&str; 5] = ["bash", "elvish", "fish", "powershell", "zsh"];

/// The completions of the words for the shell, as the shell gets them.
fn complete(shell: &str, words: &[&str], env: &[(&str, &str)]) -> String {
    let index = words.len().to_string();
    let mut args = vec!["--", "fritz"];
    args.extend(words);
    let mut env = env.to_vec();
    env.extend([
        ("COMPLETE", shell),
        ("_CLAP_IFS", "\n"),
        ("_CLAP_COMPLETE_INDEX", &index),
    ]);
    success(&fritz_with(&args, &env))
}

#[test]
fn prints_completion_scripts() {
    for shell in SHELLS {
        let script = success(&fritz(&["completion", shell]));
        assert!(script.contains("COMPLETE"), "{shell}: {script}");
        assert!(script.contains("fritz"), "{shell}: {script}");
        // The script asks `fritz` for the commands.
        let commands = complete(shell, &[""], &[]);
        for command in ["overview", "hosts", "tr064", "completion"] {
            assert!(commands.contains(command), "{shell}: {commands}");
        }
    }
    let zsh = success(&fritz(&["completion", "zsh"]));
    assert!(zsh.starts_with("#compdef fritz\n"), "{zsh}");
    let output = fritz(&["completion", "sh"]);
    assert!(!output.status.success());
    let output = fritz(&["completion", "--help"]);
    assert!(success(&output).contains("fritz completion zsh > \"${fpath[1]}/_fritz\""));
}

#[test]
fn completes_the_profiles() {
    let config = Config::new();
    let output = config.run(&["config", "init", "--host", "192.0.2.1"]);
    assert!(output.status.success(), "{}", stderr(&output));
    let env = [("FRITZ_CONFIG", config.path.as_str())];
    let output = config.run(&["--profile", "office", "config", "init", "--host", "office"]);
    assert!(output.status.success(), "{}", stderr(&output));
    let profiles = complete("fish", &["--profile", ""], &env);
    assert_eq!(profiles, "default\t192.0.2.1\noffice\toffice\n");
    let profiles = complete("bash", &["hosts", "--profile", "of"], &env);
    assert_eq!(profiles, "office");
    let zsh = complete("zsh", &["--profile", ""], &env);
    assert!(zsh.contains("default:192.0.2.1"), "{zsh}");

    // `--config` wins over the environment.
    let other = Config::new();
    let output = other.run(&["--profile", "lab", "config", "init"]);
    assert!(output.status.success(), "{}", stderr(&output));
    let words = ["--config", other.path.as_str(), "--profile", ""];
    assert_eq!(complete("bash", &words, &env), "lab");
    let words = [format!("--config={}", other.path)];
    let words = [words[0].as_str(), "--profile", ""];
    assert_eq!(complete("bash", &words, &env), "lab");

    // Nothing for a missing or invalid file, and no error.
    assert_eq!(complete("bash", &["--profile", ""], &[]), "");
    std::fs::write(&config.path, "unknown = 1\n").unwrap();
    assert_eq!(complete("bash", &["--profile", ""], &env), "");
}

#[tokio::test(flavor = "multi_thread")]
async fn trusts_the_certificate_on_first_use() {
    let fixture = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../fritz-sim/fixtures/fritzbox-7690/8.25"
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
    "/../fritz-sim/fixtures/fritzbox-7690/8.25"
);

/// The session ID of the simulator.
const SESSION: &str = "5e55104d0c0ffee5";

/// Runs `fritz tr064` against `sim` as its user, with the given password.
async fn tr64(sim: &fritz_sim::Sim, args: &[&str], password: Option<&str>) -> Output {
    let mut all = vec!["tr064"];
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
    // `tr64` is the old name of `tr064`.
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
    let tree = json(&run(&sim, &["mesh", "--tree"], password).await);
    assert_eq!(tree[0]["name"], "device-50");
    assert_eq!(tree[0]["children"][0]["name"], "device-51");
    assert_eq!(tree[0]["children"][0]["children"][0]["name"], "device-47");
    assert_eq!(tree[0]["children"][1]["uplinks"][0]["kind"], "WLAN");
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

/// Every command with JSON output prints the same value as YAML.
#[tokio::test(flavor = "multi_thread")]
async fn prints_the_same_values_as_yaml() {
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
    let commands: [&[&str]; 16] = [
        &["overview"],
        &["internet"],
        &["wlan"],
        &["ports"],
        &["telephony"],
        &["capabilities"],
        &["hosts"],
        &["hosts", "--active"],
        &["mesh"],
        &["mesh", "--tree"],
        &["smarthome"],
        &["settings"],
        &["calls"],
        &["phonebook"],
        &["log"],
        &["config", "list"],
    ];
    for command in commands {
        let mut args = vec!["--output", "json"];
        args.extend(command);
        let json: serde_json::Value =
            serde_json::from_str(&success(&run(&sim, &args, password).await)).unwrap();
        args[1] = "yaml";
        let yaml = success(&run(&sim, &args, password).await);
        let value: serde_json::Value =
            serde_saphyr::from_str(&yaml).unwrap_or_else(|e| panic!("{command:?}: {e}"));
        assert_eq!(value, json, "{command:?}");
    }
    // Set in the environment.
    let output = fritz_with(&["config", "list"], &[("FRITZ_OUTPUT", "yaml")]);
    assert_eq!(success(&output), "[]\n");
}

/// Errors in YAML are YAML on stderr, with the fields of errors in JSON.
#[tokio::test(flavor = "multi_thread")]
async fn reports_errors_as_yaml() {
    let fixture = fritz_sim::Fixture::load(FIXTURE).unwrap();
    let sim = fritz_sim::Sim::start(fixture).await.unwrap();
    sim.inject(Injection::fault("UserInterface1", "GetInfo", 867));
    let output = run(&sim, &["-o", "yaml", "overview"], Some(fritz_sim::PASSWORD)).await;
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(stdout(&output), "");
    let error: serde_json::Value = serde_saphyr::from_str(&stderr(&output)).unwrap();
    assert_eq!(error["status"], "blocked", "{error}");
    assert_eq!(error["retry_after"], 3600);
    let message = error["error"].as_str().unwrap();
    assert!(message.contains("(UPnP error 867)"), "{message}");

    let url = unreachable_url("http");
    let args = ["-o", "yaml", "overview", "--host", &url];
    let output = fritz_with(&args, &[("FRITZ_PASSWORD", "secret")]);
    assert_eq!(output.status.code(), Some(1));
    let error: serde_json::Value = serde_saphyr::from_str(&stderr(&output)).unwrap();
    let message = error["error"].as_str().unwrap();
    assert!(
        message.contains("cannot read the device description"),
        "{message}"
    );
    assert!(error.get("status").is_none(), "{error}");
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
    let output = run(&sim, &["--output", "text", "mesh", "--tree"], password).await;
    assert_eq!(
        success(&output),
        "device-50 (master, FRITZ!Box 7690)\n\
         ├── device-51 (slave, FRITZ!Repeater 1200 AX)  \
         WLAN 5 GHz ↓ 1201.0 Mbit/s  ↑ 1201.0 Mbit/s\n\
         │   └── device-47  LAN\n\
         └── device-2  WLAN 5 GHz ↓ 866.7 Mbit/s  ↑ 433.3 Mbit/s\n"
    );
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

/// Builds without the feature `write` have no command to change anything.
#[cfg(not(feature = "write"))]
#[test]
fn cannot_change_without_the_feature_write() {
    let error = failure(&fritz(&["change", "identity"]));
    assert!(
        error.contains("unrecognized subcommand 'change'"),
        "{error}"
    );
}

/// `fritz change`, in builds with the feature `write`.
#[cfg(feature = "write")]
mod change {
    use fritz_core::identity::Identity;

    use super::*;

    /// The identity of the device of the fixture.
    fn identity() -> String {
        Identity::of("uuid:00000000-0000-4000-8000-000000000001", "AABBCC000001").to_string()
    }

    struct Setup {
        sim: fritz_sim::Sim,
        dir: tempfile::TempDir,
    }

    impl Setup {
        /// A simulator, and a configuration that allows changes to it.
        async fn new() -> Self {
            let fixture = fritz_sim::Fixture::load(FIXTURE).unwrap();
            let setup = Self {
                sim: fritz_sim::Sim::start(fixture).await.unwrap(),
                dir: tempfile::tempdir().unwrap(),
            };
            setup.configure(&format!("[write]\nallow = [\"{}\"]\n", identity()));
            setup
        }

        fn configure(&self, text: &str) {
            std::fs::write(self.dir.path().join("config.toml"), text).unwrap();
        }

        /// Runs `fritz change` against the simulator with the password,
        /// the configuration, a state directory and the variables.
        async fn run(&self, args: &[&str], vars: &[(&str, &str)]) -> Output {
            let mut all = strings(&[
                "--host",
                self.sim.url().as_str(),
                "--user",
                fritz_sim::USERNAME,
                "change",
            ]);
            all.extend(strings(args));
            let dir = self.dir.path();
            let mut env = vec![
                ("FRITZ_PASSWORD".to_owned(), fritz_sim::PASSWORD.to_owned()),
                (
                    "FRITZ_CONFIG".to_owned(),
                    dir.join("config.toml").display().to_string(),
                ),
                (
                    "FRITZ_STATE_DIR".to_owned(),
                    dir.join("state").display().to_string(),
                ),
            ];
            env.extend(vars.iter().map(|(n, v)| ((*n).to_owned(), (*v).to_owned())));
            fritz_blocking(all, env, "").await
        }

        /// Plans `SetEnable` of the first network, and returns the token.
        async fn plan(&self, enable: &str) -> String {
            let argument = format!("NewEnable={enable}");
            let output = self
                .run(
                    &[
                        "--output",
                        "json",
                        "plan",
                        "WLANConfiguration1",
                        "SetEnable",
                        &argument,
                    ],
                    &[("FRITZ_ALLOW_WRITE", "1")],
                )
                .await;
            let plan: serde_json::Value = serde_json::from_str(&success(&output)).unwrap();
            plan["token"].as_str().unwrap().to_owned()
        }

        async fn apply(&self, enable: &str, token: &str, output: &str) -> Output {
            let argument = format!("NewEnable={enable}");
            self.run(
                &[
                    "--output",
                    output,
                    "apply",
                    "WLANConfiguration1",
                    "SetEnable",
                    &argument,
                    "--token",
                    token,
                ],
                &[("FRITZ_ALLOW_WRITE", "1")],
            )
            .await
        }

        fn enabled(&self) -> Option<String> {
            self.sim.state("WLANConfiguration1", "Enable")
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn prints_the_identity() {
        let setup = Setup::new().await;
        let output = success(&setup.run(&["identity"], &[]).await);
        assert_eq!(output, format!("{}\n", identity()));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn plans_changes_in_yaml() {
        let setup = Setup::new().await;
        let args = [
            "--output",
            "yaml",
            "plan",
            "WLANConfiguration1",
            "SetEnable",
            "NewEnable=0",
        ];
        let yaml = success(&setup.run(&args, &[("FRITZ_ALLOW_WRITE", "1")]).await);
        let plan: serde_json::Value = serde_saphyr::from_str(&yaml).unwrap();
        assert_eq!(plan["changes"][0]["argument"], "NewEnable", "{plan}");
        assert_eq!(plan["changes"][0]["after"], "0", "{plan}");
        assert_eq!(setup.enabled().as_deref(), Some("1"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn plans_and_applies_changes() {
        let setup = Setup::new().await;
        let args = [
            "--output",
            "text",
            "plan",
            "WLANConfiguration1",
            "SetEnable",
            "NewEnable=0",
        ];
        let plan = success(&setup.run(&args, &[("FRITZ_ALLOW_WRITE", "1")]).await);
        let expected = format!(
            "device  {}\naction  WLANConfiguration1 SetEnable (write)\nchange  NewEnable: 1 -> 0\n",
            identity()
        );
        assert!(plan.starts_with(&expected), "{plan}");
        let token = plan
            .lines()
            .find_map(|l| l.strip_prefix("token   "))
            .and_then(|t| t.strip_suffix(" (valid for 300 s)"))
            .unwrap();
        assert!(plan.contains("\nnote    shows only the inputs"), "{plan}");
        assert_eq!(setup.enabled().as_deref(), Some("1"));

        // Like a device, which reports a network it turns off as disabled.
        setup.sim.inject(Injection::side_effect(
            "WLANConfiguration1",
            "SetEnable",
            "Status",
            "Disabled",
        ));
        let applied = success(&setup.apply("0", token, "text").await);
        assert_eq!(
            applied,
            format!(
                "device        {}\napplied       WLANConfiguration1 SetEnable\nnow           NewEnable: 0 (sent 0)\nverified      yes\nalso changed  GetInfo NewStatus: Up -> Disabled\n",
                identity()
            )
        );
        assert_eq!(setup.enabled().as_deref(), Some("0"));

        let error = failure(&setup.apply("0", token, "text").await);
        assert!(error.contains("the token is unknown, used"), "{error}");
        let audit = std::fs::read_to_string(setup.dir.path().join("state/audit.jsonl")).unwrap();
        assert_eq!(audit.lines().count(), 4, "{audit}");
        assert!(audit.contains("\"source\":\"cli\""), "{audit}");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn prints_changes_as_json() {
        let setup = Setup::new().await;
        let token = setup.plan("0").await;
        let output = success(&setup.apply("0", &token, "json").await);
        let outcome: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert_eq!(outcome["verified"], true);
        assert_eq!(outcome["checks"][0]["actual"], "0");
        assert_eq!(outcome["side_effects"], serde_json::json!([]));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn fails_when_the_device_keeps_the_old_value() {
        let setup = Setup::new().await;
        setup
            .sim
            .inject(Injection::fault("WLANConfiguration1", "GetInfo", 820));
        let args = [
            "--output",
            "text",
            "plan",
            "WLANConfiguration1",
            "SetEnable",
            "NewEnable=0",
        ];
        let plan = success(&setup.run(&args, &[("FRITZ_ALLOW_WRITE", "1")]).await);
        assert!(plan.contains("change  NewEnable: ? -> 0\n"), "{plan}");
        let token = setup.plan("0").await;
        let output = success(&setup.apply("0", &token, "text").await);
        assert!(
            output.contains("now           NewEnable: ? (sent 0)\n"),
            "{output}"
        );
        assert!(output.contains("verified      unknown\n"), "{output}");

        setup.sim.clear_injections();
        let token = setup.plan("1").await;
        setup.sim.inject(Injection::DiscardWrites);
        let output = setup.apply("1", &token, "text").await;
        let error = failure(&output);
        assert!(
            error.contains("the device does not return the values it was given"),
            "{error}"
        );
        let text = stdout(&output);
        assert!(
            text.contains("now           NewEnable: 0 (sent 1)\n"),
            "{text}"
        );
        assert!(text.contains("verified      no\n"), "{text}");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn refuses_changes_that_are_not_allowed() {
        let setup = Setup::new().await;
        let plan = ["plan", "WLANConfiguration1", "SetEnable", "NewEnable=0"];
        let error = failure(&setup.run(&plan, &[]).await);
        assert!(
            error.contains("SetEnable is write; it needs FRITZ_ALLOW_WRITE=1"),
            "{error}"
        );
        let error = failure(
            &setup
                .run(&["plan", "WLANConfiguration1", "SetEnable", "x"], &[])
                .await,
        );
        assert!(error.contains("x is not NAME=VALUE"), "{error}");
        let error = failure(
            &setup
                .run(
                    &[
                        "apply",
                        "WLANConfiguration1",
                        "SetEnable",
                        "x",
                        "--token",
                        "t",
                    ],
                    &[],
                )
                .await,
        );
        assert!(error.contains("x is not NAME=VALUE"), "{error}");

        setup.configure(&format!("[write]\ndeny = [\"{}\"]\n", identity()));
        let error = failure(&setup.run(&plan, &[("FRITZ_ALLOW_WRITE", "1")]).await);
        assert!(error.contains("is on the deny list for changes"), "{error}");
        assert!(
            setup
                .sim
                .requests()
                .iter()
                .all(|r| r.action.as_deref() != Some("SetEnable"))
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn reads_devices_with_only_a_password() {
    // Devices with only a password, like FRITZ!Repeaters, take any user
    // name; without --user, fritz sends the historic default.
    for (fixture, model) in [
        ("fritzwlan-repeater-1750e/7.32", "FRITZ!WLAN Repeater 1750E"),
        ("fritz-repeater-2400/8.25", "FRITZ!Repeater 2400"),
    ] {
        let options = fritz_sim::Options {
            username: fritz_core::connection::PASSWORD_ONLY_USERNAME.to_owned(),
            ..fritz_sim::Options::default()
        };
        let fixture = fritz_sim::Fixture::bundled(fixture).unwrap();
        let sim = fritz_sim::Sim::start_with(fixture, options).await.unwrap();
        let read = |command: &str| {
            let args = strings(&["--host", sim.url().as_str(), "--output", "text", command]);
            fritz_async(args, vec![("FRITZ_PASSWORD", fritz_sim::PASSWORD)])
        };

        let overview = success(&read("overview").await);
        assert!(overview.contains(model), "{overview}");
        let wlan = success(&read("wlan").await);
        assert!(wlan.contains("wlan-1"), "{wlan}");
        let hosts = success(&read("hosts").await);
        assert!(hosts.contains("device-1"), "{hosts}");
    }
}

/// The values that must never be logged: the password, the session ID, the
/// nonce of the simulator, the WLAN keys of the fixture, and any part of an
/// `Authorization` header.
const SECRETS: &[&str] = &[
    fritz_sim::PASSWORD,
    SESSION,
    "F1A2B3C4D5E6F708",
    "fixture-wlan-key",
    "fixture-guest-key",
    "Authorization",
    "AVM-SID",
    "cnonce",
    "response=",
];

/// Runs `fritz` against `sim` once without and once with `FRITZ_LOG`, and
/// returns the log. stdout and the exit status must be the same, and the
/// log must not contain any of the [`SECRETS`].
async fn logged(sim: &fritz_sim::Sim, args: &[&str], password: &str, log: &str) -> String {
    let mut all = strings(&["--host", sim.url().as_str(), "--user", fritz_sim::USERNAME]);
    all.extend(strings(args));
    let password = ("FRITZ_PASSWORD".to_owned(), password.to_owned());
    let plain = fritz_blocking(all.clone(), vec![password.clone()], "").await;
    let env = vec![password, ("FRITZ_LOG".to_owned(), log.to_owned())];
    let output = fritz_blocking(all, env, "").await;
    assert_eq!(stdout(&output), stdout(&plain), "{args:?}");
    assert_eq!(output.status.code(), plain.status.code(), "{args:?}");
    let log = stderr(&output);
    for secret in SECRETS {
        assert!(
            !log.contains(secret),
            "{secret} in the log of {args:?}: {log}"
        );
    }
    log
}

#[tokio::test(flavor = "multi_thread")]
async fn logs_requests_without_secrets() {
    let fixture = fritz_sim::Fixture::load(FIXTURE).unwrap();
    let sim = fritz_sim::Sim::start(fixture).await.unwrap();
    let password = fritz_sim::PASSWORD;
    let authority = sim.url().authority().to_owned();

    let log = logged(&sim, &["hosts"], password, "trace").await;
    for expected in [
        &format!(
            "INFO tr064::client: request method=POST url=http://{authority}/upnp/control/hosts \
             action=\"urn:dslforum-org:service:Hosts:1#X_AVM-DE_GetHostListPath\" status=200"
        ),
        &format!("url=http://{authority}/devicehostlist.lua?sid=*** status=200"),
        r#"answer action="X_AVM-DE_GetHostListPath" arguments=NewX_AVM-DE_HostListPath="/devicehostlist.lua?sid=***""#,
        "the device asks for Digest authentication challenge=realm=",
        "the device accepted the Digest answer",
        "fritz::context: password found account=fritz-sim@127.0.0.1 source=the environment variable FRITZ_PASSWORD",
        "fritz::context: device url=http://",
        "TRACE tr064::client: document bytes=",
    ] {
        assert!(log.contains(expected), "{expected} in {log}");
    }

    // Secrets of the web interface, and the session.
    let log = logged(&sim, &["settings", "wlan"], password, "trace").await;
    assert!(
        log.contains("logged in to the web interface rights="),
        "{log}"
    );
    assert!(log.contains("/api/v0/generic/wlan status=200"), "{log}");
    logged(&sim, &["smarthome"], password, "trace").await;
    // Personal lists only with their size.
    let log = logged(&sim, &["calls"], password, "trace").await;
    assert!(log.contains("NewCallListURL=\"("), "{log}");
    assert!(
        log.contains("/calllist.lua?sid=***&max=50 status=200"),
        "{log}"
    );
    logged(&sim, &["phonebook"], password, "trace").await;
    let log = logged(&sim, &["overview"], password, "trace").await;
    assert!(log.contains(r#"NewDeviceLog="("#), "{log}");
    // A secret getter is refused before anything is sent.
    let call = ["tr064", "call", "WLANConfiguration1", "GetSecurityKeys"];
    let log = logged(&sim, &call, password, "trace").await;
    assert!(!log.contains("GetSecurityKeys\" status"), "{log}");

    // Failures.
    let log = logged(&sim, &["overview"], "wrong", "debug").await;
    assert!(
        log.contains("the device rejected the Digest answer"),
        "{log}"
    );
    let log = logged(&sim, &["smarthome"], "wrong", "debug").await;
    assert!(
        log.contains("the web interface rejected the login"),
        "{log}"
    );
    sim.inject(Injection::LockLogin);
    let log = logged(&sim, &["smarthome"], password, "debug").await;
    assert!(log.contains("the web interface blocks logins"), "{log}");
    sim.clear_injections();
    sim.inject(Injection::fault("DeviceInfo1", "GetInfo", 868));
    let call = ["tr064", "call", "DeviceInfo1", "GetInfo"];
    let log = logged(&sim, &call, password, "debug").await;
    assert!(
        log.contains("the device asks to wait status=\"busy\" retry_after_seconds=120"),
        "{log}"
    );
    sim.clear_injections();

    // -v is info, -vv debug, and trace only with FRITZ_LOG.
    let info = success(&run(&sim, &["-v", "wlan"], Some(password)).await);
    let output = run(&sim, &["-v", "wlan"], Some(password)).await;
    assert_eq!(stdout(&output), info);
    let log = stderr(&output);
    assert!(log.contains(" INFO tr064::client: request "), "{log}");
    assert!(!log.contains("DEBUG") && !log.contains("TRACE"), "{log}");
    let log = stderr(&run(&sim, &["-vv", "wlan"], Some(password)).await);
    assert!(log.contains("DEBUG") && !log.contains("TRACE"), "{log}");
    let log = stderr(&run(&sim, &["wlan"], Some(password)).await);
    assert_eq!(log, "");

    // Credentials in the host never reach the log.
    let mut host = sim.url().clone();
    host.set_username("intruder").unwrap();
    host.set_password(Some("host-secret")).unwrap();
    let args = ["--host", host.as_str(), "-vv", "tr064", "services"];
    let log = stderr(&fritz_with(&args, &[]));
    assert!(
        log.contains("fritz::context: device url=http://127.0.0.1:"),
        "{log}"
    );
    assert!(
        !log.contains("host-secret") && !log.contains("intruder"),
        "{log}"
    );

    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("missing/fritz.log");
    let env = [
        ("FRITZ_LOG", "debug"),
        ("FRITZ_LOG_FILE", missing.to_str().unwrap()),
    ];
    let error = failure(&fritz_with(&["config", "show"], &env));
    assert!(error.contains("cannot open the log file"), "{error}");

    let output = fritz_with(&["config", "show"], &[("FRITZ_LOG", "tr064=loud")]);
    assert!(
        failure(&output).contains("invalid FRITZ_LOG \"tr064=loud\""),
        "{}",
        stderr(&output)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn logs_the_check_of_the_certificate() {
    let fixture = fritz_sim::Fixture::load(FIXTURE).unwrap();
    let options = fritz_sim::Options {
        https: true,
        ..fritz_sim::Options::default()
    };
    let sim = fritz_sim::Sim::start_with(fixture, options).await.unwrap();
    let pinned = sim.fingerprint().unwrap().to_string();
    let host = sim.url().as_str();
    let debug = [("FRITZ_LOG", "debug")];

    let args = [
        "--host",
        host,
        "--fingerprint",
        &pinned,
        "tr064",
        "services",
    ];
    let output = fritz_with(&args, &debug);
    let log = stderr(&output);
    assert!(output.status.success(), "{log}");
    assert!(
        log.contains(&format!(
            "the certificate matches the pinned one fingerprint={pinned}"
        )),
        "{log}"
    );
    let args = [
        "--host",
        host,
        "--fingerprint",
        FINGERPRINT,
        "tr064",
        "services",
    ];
    let log = failure(&fritz_with(&args, &debug));
    assert!(
        log.contains("the certificate does not match the pinned one"),
        "{log}"
    );
    let args = ["--host", host, "tls", "fingerprint"];
    let output = fritz_with(&args, &debug);
    let log = stderr(&output);
    assert!(output.status.success(), "{log}");
    assert!(
        log.contains("no pinned certificate, reading its fingerprint"),
        "{log}"
    );
}

/// A password command that prints `text` without a shell.
#[cfg(unix)]
fn printf(text: &str) -> String {
    serde_json::json!(["printf", "%s", text]).to_string()
}

#[cfg(unix)]
#[test]
fn saves_and_runs_a_password_command() {
    use std::os::unix::fs::PermissionsExt as _;

    let config = Config::new();
    let command = printf("from the command");
    let output = config.run(&[
        "config",
        "init",
        "--host",
        "192.168.178.1",
        "--user",
        "agent",
        "--password-command",
        &command,
    ]);
    assert!(output.status.success(), "{}", stderr(&output));
    let saved = "password_command = [\"printf\", \"%s\", \"from the command\"]\n";
    assert!(config.text().ends_with(saved), "{}", config.text());

    // Another init keeps the command.
    let output = config.run(&["config", "init", "--user", "admin"]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(config.text().contains("username = \"admin\"\n"));
    assert!(config.text().ends_with(saved), "{}", config.text());

    let output = config.run(&["auth", "status"]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(
        stdout(&output),
        "the password of admin@192.168.178.1 comes from the command `printf`, \
         which ran successfully\n"
    );
    assert_eq!(stderr(&output), "");

    let output = config.run(&["config", "show"]);
    assert!(
        stdout(&output).ends_with("password     from the command `printf`\n"),
        "{}",
        stdout(&output)
    );

    let mode = std::fs::Permissions::from_mode(0o664);
    std::fs::set_permissions(&config.path, mode).unwrap();
    let output = config.run(&["auth", "status"]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(
        stderr(&output),
        format!(
            "warning: others may change {0}, which has the password command, \
             run `chmod go-w {0}`\n",
            config.path
        )
    );

    // The environment wins over the profile and is not checked.
    let output = fritz_with(
        &["auth", "status"],
        &[
            ("FRITZ_CONFIG", &config.path),
            ("FRITZ_PASSWORD_COMMAND", "echo from the environment"),
        ],
    );
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(stdout(&output).contains("the command `echo`"));
    assert_eq!(stderr(&output), "");
}

#[cfg(unix)]
#[test]
fn reports_password_command_errors_without_the_output() {
    let args = [
        "--host",
        "192.168.178.1",
        "--user",
        "agent",
        "auth",
        "status",
    ];
    for (command, error) in [
        (
            r#"["sh", "-c", "echo topsecret; exit 3"]"#,
            "the password command `sh` failed with exit status: 3",
        ),
        ("true", "the password command `true` printed no password"),
        (
            "fritz-test-no-such-program",
            "cannot run the password command `fritz-test-no-such-program`",
        ),
        ("[\"sh\"", "FRITZ_PASSWORD_COMMAND is invalid"),
    ] {
        let output = fritz_with(&args, &[("FRITZ_PASSWORD_COMMAND", command)]);
        assert!(!output.status.success());
        assert!(stderr(&output).contains(error), "{}", stderr(&output));
        assert!(
            !stderr(&output).contains("topsecret"),
            "{}",
            stderr(&output)
        );
        assert!(
            !stdout(&output).contains("topsecret"),
            "{}",
            stdout(&output)
        );
    }
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn logs_in_with_a_password_command() {
    let fixture = fritz_sim::Fixture::load(FIXTURE).unwrap();
    let sim = fritz_sim::Sim::start(fixture).await.unwrap();
    let args = strings(&[
        "--host",
        sim.url().as_str(),
        "--user",
        fritz_sim::USERNAME,
        "tr064",
        "call",
        "DeviceInfo1",
        "GetInfo",
    ]);
    let env = vec![
        (
            "FRITZ_PASSWORD_COMMAND".to_owned(),
            printf(&format!("{}\n", fritz_sim::PASSWORD)),
        ),
        ("FRITZ_LOG".to_owned(), "trace".to_owned()),
    ];
    let output = fritz_blocking(args, env, "").await;
    let info = success(&output);
    assert!(info.contains("NewModelName"), "{info}");
    assert!(!info.contains(fritz_sim::PASSWORD));
    let log = stderr(&output);
    assert!(!log.contains(fritz_sim::PASSWORD), "{log}");
    assert!(log.contains("password found"), "{log}");
}
