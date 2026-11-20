//! HTTPS with a fresh self-signed certificate, like the devices use.

use std::io;
use std::net::SocketAddr;
use std::sync::Arc;

use rustls::ServerConfig;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::TlsAcceptor;
use tokio_rustls::server::TlsStream;
use tr064::Fingerprint;

/// A listener that completes the TLS handshake of each connection. Failed
/// handshakes, like those of clients that reject the certificate, are
/// skipped.
pub(crate) struct TlsListener {
    tcp: TcpListener,
    acceptor: TlsAcceptor,
}

impl TlsListener {
    /// Serves a new self-signed certificate on `tcp`, and returns its
    /// fingerprint.
    pub(crate) fn new(tcp: TcpListener) -> (Self, Fingerprint) {
        let certified = rcgen::generate_simple_self_signed(["fritz-sim".to_owned()])
            .expect("a key pair can be generated");
        let der: CertificateDer<'static> = certified.cert.der().clone();
        let fingerprint = Fingerprint::of(&der);
        let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(
            certified.signing_key.serialize_der(),
        ));
        let config =
            ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
                .with_safe_default_protocol_versions()
                .expect("ring supports the default protocol versions")
                .with_no_client_auth()
                .with_single_cert(vec![der], key)
                .expect("the generated certificate is valid");
        let acceptor = TlsAcceptor::from(Arc::new(config));
        (Self { tcp, acceptor }, fingerprint)
    }
}

impl axum::serve::Listener for TlsListener {
    type Io = TlsStream<TcpStream>;
    type Addr = SocketAddr;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        loop {
            let (tcp, addr) = axum::serve::Listener::accept(&mut self.tcp).await;
            if let Ok(tls) = self.acceptor.accept(tcp).await {
                return (tls, addr);
            }
        }
    }

    fn local_addr(&self) -> io::Result<Self::Addr> {
        self.tcp.local_addr()
    }
}
