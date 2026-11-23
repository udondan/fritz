//! TLS with a pinned certificate.
//!
//! Devices like home routers use self-signed certificates, so the usual
//! checks against certificate authorities and host names don't apply. Instead
//! the client trusts exactly one certificate, identified by the SHA-256 hash
//! of its DER encoding. The handshake signatures are still verified, so only
//! the holder of that certificate's private key can answer.

use std::fmt;
use std::str::FromStr;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use reqwest::Url;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{WebPkiSupportedAlgorithms, ring};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{CertificateError, ClientConfig, DigitallySignedStruct, SignatureScheme};
use sha2::{Digest as _, Sha256};

use crate::Error;

/// The SHA-256 fingerprint of a certificate.
///
/// Parsed from 64 hex digits, optionally separated by colons. Displayed as
/// upper-case hex pairs separated by colons, the way browsers show it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Fingerprint([u8; 32]);

impl Fingerprint {
    /// The fingerprint of a DER-encoded certificate.
    pub fn of(der: &[u8]) -> Self {
        Self(Sha256::digest(der).into())
    }

    /// The raw hash.
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl FromStr for Fingerprint {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Error> {
        let invalid = || Error::InvalidFingerprint(s.to_owned());
        let hex: Vec<u8> = s.bytes().filter(|b| *b != b':').collect();
        if hex.len() != 64 {
            return Err(invalid());
        }
        let digit = |b: u8| char::from(b).to_digit(16).ok_or_else(invalid);
        let mut bytes = [0; 32];
        for (byte, pair) in bytes.iter_mut().zip(hex.chunks(2)) {
            let value = digit(pair[0])? << 4 | digit(pair[1])?;
            *byte = u8::try_from(value).expect("two hex digits fit into a byte");
        }
        Ok(Self(bytes))
    }
}

impl fmt::Display for Fingerprint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, byte) in self.0.iter().enumerate() {
            if i > 0 {
                f.write_str(":")?;
            }
            write!(f, "{byte:02X}")?;
        }
        Ok(())
    }
}

/// Accepts the certificate with the expected fingerprint, or any certificate
/// if none is expected. Remembers the last certificate it saw.
#[derive(Debug)]
pub(crate) struct PinnedVerifier {
    expected: Option<Fingerprint>,
    seen: Mutex<Option<Fingerprint>>,
    algorithms: WebPkiSupportedAlgorithms,
}

impl PinnedVerifier {
    pub(crate) fn new(expected: Option<Fingerprint>) -> Arc<Self> {
        Arc::new(Self {
            expected,
            seen: Mutex::new(None),
            algorithms: ring::default_provider().signature_verification_algorithms,
        })
    }

    /// The fingerprint of the last certificate the device presented.
    pub(crate) fn seen(&self) -> Option<Fingerprint> {
        *self.seen.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The error for a failed request: a certificate mismatch if the device
    /// presented an unexpected certificate, otherwise the HTTP error.
    pub(crate) fn explain(&self, error: reqwest::Error) -> Error {
        match (self.expected, self.seen()) {
            (Some(expected), Some(found)) if expected != found => {
                Error::CertificateMismatch { expected, found }
            }
            _ => Error::Http(error),
        }
    }

    /// The TLS configuration for a reqwest client.
    pub(crate) fn config(self: &Arc<Self>) -> ClientConfig {
        ClientConfig::builder_with_provider(Arc::new(ring::default_provider()))
            .with_safe_default_protocol_versions()
            .expect("ring supports the default protocol versions")
            .dangerous()
            .with_custom_certificate_verifier(Arc::clone(self) as Arc<dyn ServerCertVerifier>)
            .with_no_client_auth()
    }
}

impl ServerCertVerifier for PinnedVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        let found = Fingerprint::of(end_entity);
        *self.seen.lock().unwrap_or_else(PoisonError::into_inner) = Some(found);
        match self.expected {
            Some(expected) if expected != found => {
                tracing::debug!(%expected, %found, "the certificate does not match the pinned one");
                Err(rustls::Error::InvalidCertificate(
                    CertificateError::ApplicationVerificationFailure,
                ))
            }
            Some(_) => {
                tracing::debug!(fingerprint = %found, "the certificate matches the pinned one");
                Ok(ServerCertVerified::assertion())
            }
            None => {
                tracing::debug!(fingerprint = %found, "no pinned certificate, reading its fingerprint");
                Ok(ServerCertVerified::assertion())
            }
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.algorithms.supported_schemes()
    }
}

/// Connects to `url` and returns the fingerprint of the certificate the
/// device presents, to pin it on first use. Sends no credentials.
///
/// # Errors
///
/// [`Error::InvalidUrl`] if `url` is not an `https` URL, and [`Error::Http`]
/// if no TLS connection can be made.
pub async fn fetch_fingerprint(url: &Url, timeout: Duration) -> Result<Fingerprint, Error> {
    if url.scheme() != "https" {
        return Err(Error::InvalidUrl(format!("{url} is not an https URL")));
    }
    let verifier = PinnedVerifier::new(None);
    let http = reqwest::Client::builder()
        .tls_backend_preconfigured(verifier.config())
        .timeout(timeout)
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .build()?;
    // The response does not matter, only the handshake before it.
    let result = http.head(url.clone()).send().await;
    match verifier.seen() {
        Some(fingerprint) => Ok(fingerprint),
        None => Err(Error::Http(
            result.expect_err("an https response comes after a certificate"),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEX: &str = "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f";

    #[test]
    fn parses_plain_and_colon_separated_hex() {
        let plain: Fingerprint = HEX.parse().unwrap();
        let colons: Fingerprint = plain.to_string().parse().unwrap();
        assert_eq!(plain, colons);
        assert_eq!(plain.as_bytes()[31], 0x1f);
        assert!(plain.to_string().starts_with("00:01:02:"));
        assert!(plain.to_string().ends_with(":1E:1F"));
        assert_eq!(HEX.to_uppercase().parse::<Fingerprint>().unwrap(), plain);
    }

    #[test]
    fn rejects_wrong_lengths_and_non_hex() {
        let long = format!("{HEX}00");
        let non_hex = HEX.replace('a', "g");
        for input in ["", &HEX[2..], &long, &non_hex] {
            let error = input.parse::<Fingerprint>().unwrap_err();
            assert_eq!(
                error.to_string(),
                format!("invalid certificate fingerprint {input:?}")
            );
        }
    }

    #[test]
    fn hashes_der() {
        assert_eq!(
            Fingerprint::of(b"abc").to_string().replace(':', ""),
            "BA7816BF8F01CFEA414140DE5DAE2223B00361A396177A9CB410FF61F20015AD"
        );
    }
}
