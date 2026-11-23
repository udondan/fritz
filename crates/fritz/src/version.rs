//! `fritz version`: the build, and on request whether a newer release
//! exists.
//!
//! The check runs only when asked for with `--check`, never on its own and
//! never in `fritz mcp`. It asks the GitHub API for the releases of this
//! repository, sends nothing but a generic user agent, and downloads nothing
//! else. The release notes are untrusted text.

use std::io::Write;
use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context as _, bail};
use reqwest::Url;
use reqwest::header::{ACCEPT, USER_AGENT};
use rustls::ClientConfig;
use rustls_platform_verifier::BuilderVerifierExt;
use semver::Version;
use serde::{Deserialize, Serialize};

use crate::status::{self, Format};

/// The exit code of `fritz version --check` when a newer version exists.
pub(crate) const NEWER: i32 = 10;

/// Replaces the releases URL of the GitHub API, for tests. Plain HTTP is
/// only allowed to loopback addresses.
pub(crate) const ENV_RELEASES_URL: &str = "FRITZ_RELEASES_URL";

const RELEASES_URL: &str = "https://api.github.com/repos/udondan/fritz/releases?per_page=100";

/// The tags of the releases of the binary. The library crates of the
/// repository have their own tags, like `tr064-v0.1.1`.
const TAG_PREFIX: &str = "fritz-v";

const TIMEOUT: Duration = Duration::from_secs(10);

/// The most lines of the release notes to show.
const NOTES: usize = 5;

/// The details of this build.
#[derive(Debug, Serialize)]
struct Build {
    version: &'static str,
    commit: Option<&'static str>,
    built: &'static str,
    target: &'static str,
    features: Vec<&'static str>,
}

impl Build {
    fn current() -> Self {
        Self {
            version: env!("CARGO_PKG_VERSION"),
            commit: option_env!("FRITZ_BUILD_COMMIT"),
            built: env!("FRITZ_BUILD_DATE"),
            target: env!("FRITZ_BUILD_TARGET"),
            features: [("write", cfg!(feature = "write"))]
                .into_iter()
                .filter_map(|(name, on)| on.then_some(name))
                .collect(),
        }
    }
}

#[derive(Debug, Serialize)]
struct Report {
    #[serde(flatten)]
    build: Build,
    /// The latest release, with `--check`.
    #[serde(skip_serializing_if = "Option::is_none")]
    latest: Option<Latest>,
}

/// The latest release and how it compares with this build.
#[derive(Debug, Serialize)]
struct Latest {
    version: String,
    /// Whether it is newer than this build.
    newer: bool,
    url: String,
    /// The first lines of the release notes.
    notes: Vec<String>,
}

/// A release in the answer of the GitHub API.
#[derive(Debug, Deserialize)]
struct Release {
    tag_name: String,
    html_url: String,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
}

/// Prints the build, and with `check` the latest release. Returns whether
/// the latest release is newer than this build.
pub(crate) async fn run(check: bool, format: Format, out: &mut impl Write) -> anyhow::Result<bool> {
    let latest = if check {
        let url = releases_url(std::env::var(ENV_RELEASES_URL).ok().as_deref())?;
        Some(latest(&url, TIMEOUT).await?)
    } else {
        None
    };
    let report = Report {
        build: Build::current(),
        latest,
    };
    match format {
        Format::Json => status::json(&report, out)?,
        Format::Text => status::pairs(&pairs(&report), out)?,
    }
    Ok(report.latest.is_some_and(|l| l.newer))
}

fn pairs(report: &Report) -> Vec<(&'static str, Option<String>)> {
    let build = &report.build;
    let features = if build.features.is_empty() {
        "none".to_owned()
    } else {
        build.features.join(", ")
    };
    let mut pairs = vec![
        ("Version", Some(build.version.to_owned())),
        ("Commit", Some(build.commit.unwrap_or("unknown").to_owned())),
        ("Built", Some(build.built.to_owned())),
        ("Target", Some(build.target.to_owned())),
        ("Features", Some(features)),
    ];
    if let Some(latest) = &report.latest {
        let state = if latest.newer {
            "a newer version is available"
        } else {
            "this is the latest version"
        };
        pairs.extend([
            ("Latest", Some(format!("{} ({state})", latest.version))),
            ("Release", Some(latest.url.clone())),
        ]);
        for (index, line) in latest.notes.iter().enumerate() {
            pairs.push((if index == 0 { "Notes" } else { "" }, Some(line.clone())));
        }
    }
    pairs
}

/// The releases URL: the one of GitHub, or the one given. Plain HTTP only to
/// loopback addresses, so that the answer cannot be changed on the way.
fn releases_url(given: Option<&str>) -> anyhow::Result<Url> {
    let Some(given) = given else {
        return Ok(Url::parse(RELEASES_URL).expect("a valid URL"));
    };
    let url = Url::parse(given).with_context(|| format!("{ENV_RELEASES_URL} is not a URL"))?;
    match url.scheme() {
        "https" => Ok(url),
        "http" if is_loopback(&url) => Ok(url),
        _ => bail!("{ENV_RELEASES_URL} must be an https URL, or http to a loopback address"),
    }
}

fn is_loopback(url: &Url) -> bool {
    let host = url.host_str().unwrap_or_default();
    host == "localhost"
        || host
            .trim_start_matches('[')
            .trim_end_matches(']')
            .parse::<IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

/// Asks for the releases and returns the latest of `fritz`, ignoring drafts,
/// pre-releases and the releases of the library crates.
async fn latest(url: &Url, timeout: Duration) -> anyhow::Result<Latest> {
    let releases: Vec<Release> = fetch(url, timeout).await?;
    let current = Version::parse(env!("CARGO_PKG_VERSION")).expect("a valid version");
    let (version, release) = releases
        .into_iter()
        .filter(|r| !r.draft && !r.prerelease)
        .filter_map(|r| {
            let version = Version::parse(r.tag_name.strip_prefix(TAG_PREFIX)?).ok()?;
            version.pre.is_empty().then_some((version, r))
        })
        .max_by(|(a, _), (b, _)| a.cmp(b))
        .context("the answer of GitHub lists no release of fritz")?;
    Ok(Latest {
        newer: version > current,
        version: version.to_string(),
        url: release.html_url,
        notes: notes(release.body.as_deref().unwrap_or_default()),
    })
}

/// The first lines of the release notes that are not empty.
fn notes(body: &str) -> Vec<String> {
    body.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .take(NOTES)
        .map(str::to_owned)
        .collect()
}

async fn fetch(url: &Url, timeout: Duration) -> anyhow::Result<Vec<Release>> {
    let response = client(url, timeout)?
        .get(url.clone())
        // GitHub needs a user agent. It names the program and nothing else.
        .header(USER_AGENT, "fritz")
        .header(ACCEPT, "application/vnd.github+json")
        .send()
        .await
        .map_err(|e| request_error(&e, timeout))?;
    let status = response.status();
    if !status.is_success() {
        bail!("GitHub answered with {status}");
    }
    let body = response
        .bytes()
        .await
        .map_err(|e| request_error(&e, timeout))?;
    serde_json::from_slice(&body).context("cannot understand the answer of GitHub")
}

fn request_error(error: &reqwest::Error, timeout: Duration) -> anyhow::Error {
    if error.is_timeout() {
        anyhow::anyhow!(
            "GitHub did not answer within {} seconds",
            timeout.as_secs_f32()
        )
    } else {
        anyhow::anyhow!("cannot reach GitHub, are you offline? ({error})")
    }
}

/// An HTTP client that trusts the roots of the system and takes the proxy
/// from `HTTPS_PROXY`, except for loopback addresses.
fn client(url: &Url, timeout: Duration) -> anyhow::Result<reqwest::Client> {
    let mut builder = reqwest::Client::builder()
        .timeout(timeout)
        .https_only(url.scheme() == "https");
    if is_loopback(url) {
        builder = builder.no_proxy();
    }
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .and_then(BuilderVerifierExt::with_platform_verifier)
        .map(|tls| builder.tls_backend_preconfigured(tls.with_no_client_auth()))
        .map_err(anyhow::Error::from)
        .and_then(|builder| builder.build().map_err(anyhow::Error::from))
}

#[cfg(test)]
mod tests {
    use axum::Router;
    use axum::http::StatusCode;
    use axum::routing::get;
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    use super::*;
    use crate::status::tests::text;

    /// A server that answers the releases URL with `status` and `body` after
    /// `delay`.
    async fn server(status: StatusCode, body: String, delay: Duration) -> Url {
        let app = Router::new().route(
            "/releases",
            get(move || async move {
                tokio::time::sleep(delay).await;
                (status, body)
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/releases", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        Url::parse(&url).unwrap()
    }

    fn release(tag: &str, prerelease: bool, draft: bool) -> serde_json::Value {
        serde_json::json!({
            "tag_name": tag,
            "html_url": format!("https://github.com/udondan/fritz/releases/tag/{tag}"),
            "body": format!("## {tag}\n\n### Features\n\n* one\n* two\n* three\n* four"),
            "prerelease": prerelease,
            "draft": draft,
        })
    }

    async fn latest_of(releases: &[serde_json::Value]) -> anyhow::Result<Latest> {
        let body = serde_json::to_string(releases).unwrap();
        let url = server(StatusCode::OK, body, Duration::ZERO).await;
        latest(&url, TIMEOUT).await
    }

    fn bump(minor: u64) -> String {
        let mut version = Version::parse(env!("CARGO_PKG_VERSION")).unwrap();
        version.minor += minor;
        format!("fritz-v{version}")
    }

    #[tokio::test]
    async fn finds_a_newer_release_of_fritz() {
        let newer = bump(1);
        let latest = latest_of(&[
            release("tr064-v99.0.0", false, false),
            release(&bump(3), true, false),
            release(&bump(2), false, true),
            release("fritz-v99.0.0-rc.1", false, false),
            release("fritz-vnext", false, false),
            release(&newer, false, false),
            release("fritz-v0.0.1", false, false),
        ])
        .await
        .unwrap();
        assert!(latest.newer);
        assert_eq!(format!("fritz-v{}", latest.version), newer);
        assert!(latest.url.ends_with(&newer));
        assert_eq!(latest.notes.len(), NOTES);
        assert_eq!(latest.notes[0], format!("## {newer}"));
    }

    #[tokio::test]
    async fn knows_when_this_is_the_latest_or_a_newer_version() {
        let same = format!("fritz-v{}", env!("CARGO_PKG_VERSION"));
        let latest = latest_of(&[release(&same, false, false)]).await.unwrap();
        assert!(!latest.newer);
        let latest = latest_of(&[release("fritz-v0.0.1", false, false)])
            .await
            .unwrap();
        assert!(!latest.newer);
        assert_eq!(latest.version, "0.0.1");
    }

    #[tokio::test]
    async fn needs_a_release_that_is_not_a_pre_release() {
        let error = latest_of(&[release(&bump(1), true, false)])
            .await
            .unwrap_err();
        assert!(error.to_string().contains("no release of fritz"), "{error}");
    }

    #[tokio::test]
    async fn refuses_malformed_answers() {
        let url = server(StatusCode::OK, "{\"message\": 1}".into(), Duration::ZERO).await;
        let error = latest(&url, TIMEOUT).await.unwrap_err();
        assert!(error.to_string().contains("cannot understand"), "{error}");
        let url = server(StatusCode::FORBIDDEN, String::new(), Duration::ZERO).await;
        let error = latest(&url, TIMEOUT).await.unwrap_err();
        assert!(error.to_string().contains("403"), "{error}");
    }

    #[tokio::test]
    async fn tells_when_the_answer_breaks_off() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/releases", listener.local_addr().unwrap());
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0; 1024];
            let _ = stream.read(&mut request).await;
            let head = "HTTP/1.1 200 OK\r\ncontent-length: 100\r\n\r\n[";
            stream.write_all(head.as_bytes()).await.unwrap();
        });
        let error = latest(&Url::parse(&url).unwrap(), TIMEOUT)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("cannot reach GitHub"), "{error}");
    }

    #[tokio::test]
    async fn gives_up_after_the_timeout() {
        let url = server(StatusCode::OK, "[]".into(), Duration::from_secs(5)).await;
        let error = latest(&url, Duration::from_millis(200)).await.unwrap_err();
        assert!(
            error.to_string().contains("did not answer within"),
            "{error}"
        );
    }

    #[tokio::test]
    async fn tells_when_github_cannot_be_reached() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/releases", listener.local_addr().unwrap());
        drop(listener);
        let error = latest(&Url::parse(&url).unwrap(), TIMEOUT)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("offline"), "{error}");
    }

    #[test]
    fn sets_up_https_clients_without_connecting() {
        let url = releases_url(None).unwrap();
        assert!(client(&url, TIMEOUT).is_ok());
    }

    #[test]
    fn allows_plain_http_only_to_loopback() {
        assert_eq!(releases_url(None).unwrap().as_str(), RELEASES_URL);
        for allowed in [
            "https://example.com/releases",
            "http://127.0.0.1:1/releases",
            "http://[::1]:1/releases",
            "http://localhost:1/releases",
        ] {
            assert!(releases_url(Some(allowed)).is_ok(), "{allowed}");
        }
        for refused in ["http://example.com/releases", "ftp://127.0.0.1/", "nothing"] {
            assert!(releases_url(Some(refused)).is_err(), "{refused}");
        }
        assert!(!is_loopback(&Url::parse("file:///tmp").unwrap()));
    }

    #[test]
    fn prints_the_build_and_the_latest_release_as_text() {
        let mut report = Report {
            build: Build {
                version: "0.1.1",
                commit: None,
                built: "2026-11-23",
                target: "aarch64-apple-darwin",
                features: Vec::new(),
            },
            latest: None,
        };
        let printed = text(|out| status::pairs(&pairs(&report), out));
        assert!(printed.contains("Commit    unknown\n"), "{printed}");
        assert!(printed.contains("Features  none\n"), "{printed}");
        report.build.features = vec!["write"];
        report.latest = Some(Latest {
            version: "0.2.0".into(),
            newer: true,
            url: "https://github.com/udondan/fritz/releases/tag/fritz-v0.2.0".into(),
            notes: vec!["## 0.2.0".into(), "* \u{1b}[31mred".into()],
        });
        let printed = text(|out| status::pairs(&pairs(&report), out));
        assert!(printed.contains("Features  write\n"), "{printed}");
        assert!(
            printed.contains("Latest    0.2.0 (a newer version is available)\n"),
            "{printed}"
        );
        assert!(printed.contains("Notes     ## 0.2.0\n"), "{printed}");
        assert!(
            printed.contains("          * \u{FFFD}[31mred\n"),
            "{printed}"
        );
        report.latest.as_mut().unwrap().newer = false;
        let printed = text(|out| status::pairs(&pairs(&report), out));
        assert!(
            printed.contains("(this is the latest version)"),
            "{printed}"
        );
    }
}
