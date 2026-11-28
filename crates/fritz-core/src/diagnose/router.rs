//! Diagnoses the device itself: its address is private, TR-064 answers over
//! HTTPS and HTTP, the web interface answers, and the certificate it
//! presents is the trusted one. Only fetches the device description and the
//! login challenge, which need no login.

use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

use tr064::{Client, Fingerprint, Url};

use super::{Check, Network, Report, Status, is_private};
use crate::connection::{HTTP_PORT, HTTPS_PORT, Target};
use crate::{redact, session};

/// What names the device itself instead of a host.
pub const ROUTER: &str = "router";

/// How long a request to the device may take.
const TIMEOUT: Duration = Duration::from_secs(5);

const PRIVATE: &str = "private address";
const HTTPS: &str = "TR-064 over HTTPS";
const HTTP: &str = "TR-064 over HTTP";
const WEB: &str = "web interface";
const CERTIFICATE: &str = "certificate";

/// Diagnoses the device of `target`. Sends no credentials.
pub async fn router(target: &Target, network: &impl Network) -> Report {
    // Credentials in the address are never sent and must not be shown.
    let mut target = target.clone();
    target
        .url
        .set_username("")
        .and_then(|()| target.url.set_password(None))
        .ok();
    let target = &target;
    let url = &target.url;
    let host = url.host_str().unwrap_or_default();
    let host = host.trim_start_matches('[').trim_end_matches(']');
    let (https, presented) = https(endpoint(url, "https").as_ref(), target.fingerprint).await;
    let checks = vec![
        private(host, network).await,
        https,
        http(endpoint(url, "http").as_ref()).await,
        web(target).await,
        certificate(target, presented),
    ];
    Report {
        target: url.to_string(),
        host: None,
        checks,
    }
}

/// The address of the device for `scheme`: the configured one, or for the
/// standard TR-064 port of the other scheme, the standard port of this one.
fn endpoint(url: &Url, scheme: &str) -> Option<Url> {
    if url.scheme() == scheme {
        return Some(url.clone());
    }
    let (from, to) = if scheme == "https" {
        (HTTP_PORT, HTTPS_PORT)
    } else {
        (HTTPS_PORT, HTTP_PORT)
    };
    let mut other = url.clone();
    (url.port() == Some(from)
        && other.set_scheme(scheme).is_ok()
        && other.set_port(Some(to)).is_ok())
    .then_some(other)
}

/// A client for `url` with a short timeout and the log redacted.
fn client(url: &Url, pin: Option<Fingerprint>) -> Result<Client, tr064::Error> {
    let builder = Client::builder(url.clone())
        .timeout(TIMEOUT)
        .redact(Arc::new(redact::Log));
    match pin {
        Some(fingerprint) => builder.pin(fingerprint),
        None => builder,
    }
    .build()
}

async fn private(host: &str, network: &impl Network) -> Check {
    let addresses = match host.parse::<IpAddr>() {
        Ok(address) => vec![address],
        Err(_) => network.resolve(host).await.unwrap_or_default(),
    };
    if addresses.is_empty() {
        return Check::fail(
            PRIVATE,
            Some("does not resolve".to_owned()),
            format!("{host} does not resolve on this machine; give the address of the device"),
        );
    }
    let list: Vec<String> = addresses.iter().map(ToString::to_string).collect();
    let detail = Some(format!("{host} → {}", list.join(", ")));
    match addresses.iter().find(|a| !is_private(**a)) {
        None => Check::new(PRIVATE, Status::Pass, detail),
        Some(public) => Check::fail(
            PRIVATE,
            detail,
            format!(
                "{host} resolves to the public address {public}, so the requests may leave the \
                 home network; check the DNS server or give the local address of the device"
            ),
        ),
    }
}

/// Whether TR-064 answers over HTTPS, and the certificate it presents.
async fn https(url: Option<&Url>, trusted: Option<Fingerprint>) -> (Check, Option<Fingerprint>) {
    let Some(url) = url else {
        let detail = Some("the device is configured on another port".to_owned());
        return (Check::new(HTTPS, Status::Skip, detail), None);
    };
    let presented = match tr064::fetch_fingerprint(url, TIMEOUT).await {
        Ok(presented) => presented,
        Err(error) => return (unanswered(HTTPS, url, &error), None),
    };
    // Only with the trusted certificate can more than the handshake be
    // checked.
    let check = if trusted == Some(presented) {
        description(HTTPS, url, trusted).await
    } else {
        Check::new(HTTPS, Status::Pass, Some(format!("{url} answers")))
    };
    (check, Some(presented))
}

async fn http(url: Option<&Url>) -> Check {
    match url {
        Some(url) => description(HTTP, url, None).await,
        None => Check::new(
            HTTP,
            Status::Skip,
            Some("the device is configured on another port".to_owned()),
        ),
    }
}

/// Whether the device description can be fetched from `url`.
async fn description(name: &str, url: &Url, pin: Option<Fingerprint>) -> Check {
    match fetch(url, pin, "/tr64desc.xml").await {
        Ok(()) => Check::new(name, Status::Pass, Some(format!("{url} answers"))),
        Err(error) => unanswered(name, url, &error),
    }
}

/// Fetches `path` of the device at `url`, without credentials.
async fn fetch(url: &Url, pin: Option<Fingerprint>, path: &str) -> Result<(), tr064::Error> {
    client(url, pin)?.get(path).await.map(drop)
}

/// A failed check of an address that does not answer.
fn unanswered(name: &str, url: &Url, error: &tr064::Error) -> Check {
    Check::fail(
        name,
        Some(error.to_string()),
        format!(
            "{url} does not answer; check that the device allows access for applications \
             (TR-064) and that nothing blocks the port"
        ),
    )
}

/// Whether the login page of the web interface answers.
async fn web(target: &Target) -> Check {
    let url = target.web().url;
    let https = url.scheme() == "https";
    if https && target.fingerprint.is_none() {
        let detail = Some("no certificate is trusted".to_owned());
        return Check::new(WEB, Status::Unknown, detail);
    }
    let pin = target.fingerprint.filter(|_| https);
    match fetch(&url, pin, session::LOGIN).await {
        Ok(()) => Check::new(WEB, Status::Pass, Some(format!("{url} answers"))),
        Err(error) => Check::fail(
            WEB,
            Some(error.to_string()),
            format!("the web interface does not answer on {url}"),
        ),
    }
}

fn certificate(target: &Target, presented: Option<Fingerprint>) -> Check {
    let https = target.url.scheme() == "https";
    match (presented, target.fingerprint) {
        (None, _) => Check::new(
            CERTIFICATE,
            Status::Skip,
            Some("no certificate presented".to_owned()),
        ),
        (Some(_), None) if !https => Check::new(
            CERTIFICATE,
            Status::Skip,
            Some("the profile uses http".to_owned()),
        ),
        (Some(presented), None) => Check::fail(
            CERTIFICATE,
            Some(format!("{presented}, not trusted")),
            "no certificate is trusted; compare it with the web interface and run `fritz tls \
             trust`"
                .to_owned(),
        ),
        (Some(presented), Some(trusted)) if presented == trusted => Check::new(
            CERTIFICATE,
            Status::Pass,
            Some("matches the trusted fingerprint".to_owned()),
        ),
        (Some(presented), Some(trusted)) => Check::fail(
            CERTIFICATE,
            Some(presented.to_string()),
            format!(
                "the device presents {presented}, but {trusted} is trusted; if the device got a \
                 new certificate, compare it with the web interface and run `fritz tls trust`"
            ),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_endpoints() {
        let url = |text: &str| Url::parse(text).unwrap();
        let https = url("https://fritz.box:49443/");
        let http = url("http://fritz.box:49000/");
        assert_eq!(endpoint(&https, "https"), Some(https.clone()));
        assert_eq!(endpoint(&https, "http"), Some(http.clone()));
        assert_eq!(endpoint(&http, "https"), Some(https));
        assert_eq!(endpoint(&url("http://127.0.0.1:8080/"), "https"), None);
        assert_eq!(endpoint(&url("https://fritz.box/"), "http"), None);
    }

    #[tokio::test]
    async fn fails_where_nothing_answers() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let closed = format!("http://{}/", listener.local_addr().unwrap());
        drop(listener);
        let check = http(Some(&Url::parse(&closed).unwrap())).await;
        assert_eq!(check.status, Status::Fail);
        assert!(check.hint.unwrap().contains("does not answer"));
    }

    #[tokio::test]
    async fn checks_that_the_device_is_private() {
        let network = super::super::Fixed {
            names: vec![
                (
                    "fritz.box".to_owned(),
                    vec!["192.168.178.1".parse().unwrap()],
                ),
                (
                    "public.example".to_owned(),
                    vec!["192.0.2.1".parse().unwrap()],
                ),
            ],
            ..Default::default()
        };
        let pass = private("fritz.box", &network).await;
        assert_eq!(pass.status, Status::Pass);
        assert_eq!(pass.detail.as_deref(), Some("fritz.box → 192.168.178.1"));
        assert_eq!(private("10.0.0.1", &network).await.status, Status::Pass);
        let public = private("public.example", &network).await;
        assert!(public.hint.unwrap().contains("public address 192.0.2.1"));
        let missing = private("nowhere", &network).await;
        assert_eq!(missing.detail.as_deref(), Some("does not resolve"));
    }

    #[test]
    fn compares_the_certificate() {
        let one = Fingerprint::of(b"one");
        let two = Fingerprint::of(b"two");
        let target = |scheme: &str, fingerprint| Target {
            url: Url::parse(&format!("{scheme}://fritz.box/")).unwrap(),
            username: None,
            fingerprint,
        };
        let https = target("https", Some(one));
        assert_eq!(certificate(&https, None).status, Status::Skip);
        assert_eq!(certificate(&https, Some(one)).status, Status::Pass);
        let other = certificate(&https, Some(two));
        assert!(other.hint.unwrap().contains("got a new certificate"));
        let untrusted = certificate(&target("https", None), Some(one));
        assert!(untrusted.hint.unwrap().contains("fritz tls trust"));
        let plain = certificate(&target("http", None), Some(one));
        assert_eq!(plain.detail.as_deref(), Some("the profile uses http"));
    }
}
