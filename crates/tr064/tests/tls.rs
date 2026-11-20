//! The client against a local HTTPS server with a self-signed certificate.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use rustls::{ServerConfig, SupportedProtocolVersion};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;
use tr064::{Client, Error, Fingerprint, SecretString, Url, fetch_fingerprint};

const BODY: &str = "hello over TLS";

/// How the server ends its answer.
#[derive(Clone, Copy)]
enum Ending {
    /// With a `Content-Length` and `close_notify`.
    Clean,
    /// Without a length, by closing the connection without `close_notify`.
    Abrupt,
    /// With a `Content-Length` larger than the body, by closing the
    /// connection without `close_notify`.
    Truncated,
}

/// A server that answers every request with [`BODY`], and the fingerprint of
/// its certificate.
async fn server(version: &'static SupportedProtocolVersion) -> (SocketAddr, Fingerprint) {
    server_ending(version, Ending::Clean).await
}

async fn server_ending(
    version: &'static SupportedProtocolVersion,
    ending: Ending,
) -> (SocketAddr, Fingerprint) {
    let certified = rcgen::generate_simple_self_signed(["localhost".to_owned()]).unwrap();
    let der: CertificateDer<'static> = certified.cert.der().clone();
    let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(
        certified.signing_key.serialize_der(),
    ));
    let fingerprint = Fingerprint::of(&der);
    let config =
        ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_protocol_versions(&[version])
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(vec![der], key)
            .unwrap();
    let acceptor = TlsAcceptor::from(Arc::new(config));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let (tcp, _) = listener.accept().await.unwrap();
            let acceptor = acceptor.clone();
            tokio::spawn(async move {
                // A client that rejects the certificate aborts the handshake.
                let Ok(mut tls) = acceptor.accept(tcp).await else {
                    return;
                };
                let mut request = Vec::new();
                let mut buffer = [0; 1024];
                while !request.ends_with(b"\r\n\r\n") {
                    let n = tls.read(&mut buffer).await.unwrap();
                    request.extend_from_slice(&buffer[..n]);
                }
                let length = match ending {
                    Ending::Clean => format!("Content-Length: {}\r\n", BODY.len()),
                    Ending::Abrupt => String::new(),
                    Ending::Truncated => format!("Content-Length: {}\r\n", BODY.len() + 10),
                };
                let response =
                    format!("HTTP/1.1 200 OK\r\n{length}Connection: close\r\n\r\n{BODY}");
                tls.write_all(response.as_bytes()).await.unwrap();
                tls.flush().await.unwrap();
                match ending {
                    Ending::Clean => tls.shutdown().await.unwrap(),
                    // Closes the TCP connection without close_notify.
                    Ending::Abrupt | Ending::Truncated => drop(tls.into_inner().0),
                }
            });
        }
    });
    (addr, fingerprint)
}

fn url(addr: SocketAddr) -> Url {
    Url::parse(&format!("https://{addr}")).unwrap()
}

fn pinned(addr: SocketAddr, pin: Fingerprint) -> Client {
    Client::builder(url(addr))
        .credentials("user", SecretString::from("secret"))
        .pin(pin)
        .build()
        .unwrap()
}

#[tokio::test]
async fn accepts_a_body_that_ends_with_the_connection() {
    let (addr, fingerprint) = server_ending(&rustls::version::TLS13, Ending::Abrupt).await;
    assert_eq!(pinned(addr, fingerprint).get("/").await.unwrap(), BODY);
}

#[tokio::test]
async fn rejects_a_body_shorter_than_its_length() {
    let (addr, fingerprint) = server_ending(&rustls::version::TLS13, Ending::Truncated).await;
    let error = pinned(addr, fingerprint).get("/").await.unwrap_err();
    assert!(matches!(error, Error::Http(_)), "{error:?}");
}

#[tokio::test]
async fn accepts_the_pinned_certificate_with_tls13() {
    let (addr, fingerprint) = server(&rustls::version::TLS13).await;
    assert_eq!(pinned(addr, fingerprint).get("/").await.unwrap(), BODY);
}

#[tokio::test]
async fn accepts_the_pinned_certificate_with_tls12() {
    let (addr, fingerprint) = server(&rustls::version::TLS12).await;
    assert_eq!(pinned(addr, fingerprint).get("/").await.unwrap(), BODY);
}

#[tokio::test]
async fn rejects_another_certificate() {
    let (addr, found) = server(&rustls::version::TLS13).await;
    let expected = Fingerprint::of(b"another certificate");
    let error = pinned(addr, expected).get("/").await.unwrap_err();
    assert!(
        matches!(error, Error::CertificateMismatch { expected: e, found: f } if e == expected && f == found),
        "{error:?}"
    );
    assert!(error.to_string().contains(&found.to_string()));
}

#[tokio::test]
async fn reports_other_failures_of_a_pinned_client_as_http_errors() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);
    let error = pinned(addr, Fingerprint::of(b"any"))
        .get("/")
        .await
        .unwrap_err();
    assert!(matches!(error, Error::Http(_)), "{error:?}");
}

#[tokio::test]
async fn fetches_the_fingerprint_on_first_use() {
    let (addr, fingerprint) = server(&rustls::version::TLS13).await;
    let fetched = fetch_fingerprint(&url(addr), Duration::from_secs(5))
        .await
        .unwrap();
    assert_eq!(fetched, fingerprint);
}

#[tokio::test]
async fn fetching_the_fingerprint_needs_https_and_a_server() {
    let http = Url::parse("http://127.0.0.1:1").unwrap();
    let error = fetch_fingerprint(&http, Duration::from_secs(5))
        .await
        .unwrap_err();
    assert!(matches!(error, Error::InvalidUrl(_)), "{error:?}");

    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);
    let error = fetch_fingerprint(&url(addr), Duration::from_secs(5))
        .await
        .unwrap_err();
    assert!(matches!(error, Error::Http(_)), "{error:?}");
}

#[test]
fn requires_a_pin_exactly_for_https() {
    let pin = Fingerprint::of(b"any");
    let https = Url::parse("https://192.0.2.1:49443").unwrap();
    let http = Url::parse("http://192.0.2.1:49000").unwrap();
    let ftp = Url::parse("ftp://192.0.2.1").unwrap();

    assert!(Client::builder(https.clone()).pin(pin).build().is_ok());
    assert!(Client::builder(http.clone()).build().is_ok());
    for builder in [
        Client::builder(https),
        Client::builder(http).pin(pin),
        Client::builder(ftp),
    ] {
        let error = builder.build().unwrap_err();
        assert!(matches!(error, Error::InvalidUrl(_)), "{error:?}");
    }
}
