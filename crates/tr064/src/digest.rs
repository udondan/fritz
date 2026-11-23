//! HTTP Digest access authentication (RFC 7616), as far as TR-064 devices use
//! it: the algorithms MD5 and SHA-256, with or without `qop=auth`.

use std::fmt::{self, Write as _};
use std::iter;

use md5::Md5;
use sha2::{Digest as _, Sha256};

use crate::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Algorithm {
    Md5,
    Sha256,
}

impl Algorithm {
    fn hash(self, data: &str) -> String {
        match self {
            Self::Md5 => hex(&Md5::digest(data.as_bytes())),
            Self::Sha256 => hex(&Sha256::digest(data.as_bytes())),
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Md5 => "MD5",
            Self::Sha256 => "SHA-256",
        }
    }
}

/// A `WWW-Authenticate: Digest …` challenge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Challenge {
    realm: String,
    nonce: String,
    opaque: Option<String>,
    algorithm: Algorithm,
    qop: bool,
}

impl Challenge {
    pub(crate) fn parse(header: &str) -> Result<Self, Error> {
        let unsupported = || Error::UnsupportedAuth(header.to_owned());
        let (scheme, rest) = header.trim().split_once(' ').ok_or_else(unsupported)?;
        if !scheme.eq_ignore_ascii_case("digest") {
            return Err(unsupported());
        }
        let params = parse_params(rest);
        let get = |key: &str| {
            params
                .iter()
                .find(|(name, _)| name.eq_ignore_ascii_case(key))
                .map(|(_, value)| value.as_str())
        };
        let algorithm = match get("algorithm") {
            None => Algorithm::Md5,
            Some(a) if a.eq_ignore_ascii_case("MD5") => Algorithm::Md5,
            Some(a) if a.eq_ignore_ascii_case("SHA-256") => Algorithm::Sha256,
            Some(_) => return Err(unsupported()),
        };
        let qop = match get("qop") {
            None => false,
            Some(q) if q.split(',').any(|o| o.trim().eq_ignore_ascii_case("auth")) => true,
            Some(_) => return Err(unsupported()),
        };
        Ok(Self {
            realm: get("realm").ok_or_else(unsupported)?.to_owned(),
            nonce: get("nonce").ok_or_else(unsupported)?.to_owned(),
            opaque: get("opaque").map(str::to_owned),
            algorithm,
            qop,
        })
    }
}

/// The challenge as it may be logged: without the nonce and the opaque
/// value, which belong to the answer.
impl fmt::Display for Challenge {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "realm={:?}, algorithm={}, qop={}",
            self.realm,
            self.algorithm.name(),
            if self.qop { "auth" } else { "none" }
        )
    }
}

/// The state to answer a challenge, reused for further requests until the
/// device sends a new one.
#[derive(Debug)]
pub(crate) struct DigestAuth {
    challenge: Challenge,
    nc: u32,
}

impl DigestAuth {
    pub(crate) fn new(challenge: Challenge) -> Self {
        Self { challenge, nc: 0 }
    }

    /// The `Authorization` header for the next request.
    pub(crate) fn authorization(
        &mut self,
        username: &str,
        password: &str,
        method: &str,
        uri: &str,
    ) -> String {
        let mut bytes = [0u8; 16];
        getrandom::fill(&mut bytes).expect("the OS random number generator works");
        self.authorization_with_cnonce(username, password, method, uri, &hex(&bytes))
    }

    fn authorization_with_cnonce(
        &mut self,
        username: &str,
        password: &str,
        method: &str,
        uri: &str,
        cnonce: &str,
    ) -> String {
        self.nc += 1;
        let c = &self.challenge;
        let ha1 = c
            .algorithm
            .hash(&format!("{username}:{}:{password}", c.realm));
        let ha2 = c.algorithm.hash(&format!("{method}:{uri}"));
        let nc = format!("{:08x}", self.nc);
        let mut header = format!(
            "Digest username=\"{}\", realm=\"{}\", nonce=\"{}\", uri=\"{}\", algorithm={}",
            quote(username),
            quote(&c.realm),
            quote(&c.nonce),
            quote(uri),
            c.algorithm.name(),
        );
        let response = if c.qop {
            let _ = write!(header, ", qop=auth, nc={nc}, cnonce=\"{cnonce}\"");
            c.algorithm
                .hash(&format!("{ha1}:{}:{nc}:{cnonce}:auth:{ha2}", c.nonce))
        } else {
            c.algorithm.hash(&format!("{ha1}:{}:{ha2}", c.nonce))
        };
        let _ = write!(header, ", response=\"{response}\"");
        if let Some(opaque) = &c.opaque {
            let _ = write!(header, ", opaque=\"{}\"", quote(opaque));
        }
        header
    }
}

/// Parses `key=value, key="quoted value", …`.
fn parse_params(input: &str) -> Vec<(String, String)> {
    let mut params = Vec::new();
    let mut chars = input.chars().peekable();
    loop {
        while chars.next_if(|c| c.is_whitespace() || *c == ',').is_some() {}
        let key: String = iter::from_fn(|| chars.next_if(|c| *c != '=')).collect();
        if chars.next().is_none() {
            return params;
        }
        let value = if chars.next_if_eq(&'"').is_some() {
            let mut value = String::new();
            while let Some(c) = chars.next() {
                match c {
                    '"' => break,
                    '\\' => value.extend(chars.next()),
                    _ => value.push(c),
                }
            }
            value
        } else {
            let value: String = iter::from_fn(|| chars.next_if(|c| *c != ',')).collect();
            value.trim().to_owned()
        };
        params.push((key.trim().to_owned(), value));
    }
}

fn quote(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

fn hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut s, b| {
            let _ = write!(s, "{b:02x}");
            s
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    // RFC 7616, section 3.9.1.
    const RFC7616: &str = r#"Digest realm="http-auth@example.org", qop="auth, auth-int", algorithm=ALGO, nonce="7ypf/xlj9XXwfDPEoM4URrv/xwf94BcCAzFZH4GiTo0v", opaque="FQhe/qaU925kfnzjCev0ciny7QMkPqMAFRtzCUYo5tdS""#;
    const CNONCE: &str = "f2/wE4q74E6zIJEtWaHKaf5wv/H5QzzpXusqGemxURZJ";

    fn response_of(header: &str) -> &str {
        let start = header.find("response=\"").unwrap() + 10;
        &header[start..start + header[start..].find('"').unwrap()]
    }

    #[test]
    fn shows_challenges_without_the_nonce() {
        let challenge = Challenge::parse(&RFC7616.replace("ALGO", "SHA-256")).unwrap();
        assert_eq!(
            challenge.to_string(),
            r#"realm="http-auth@example.org", algorithm=SHA-256, qop=auth"#
        );
        let challenge = Challenge::parse(r#"Digest realm="r", nonce="n""#).unwrap();
        assert_eq!(
            challenge.to_string(),
            r#"realm="r", algorithm=MD5, qop=none"#
        );
    }

    #[test]
    fn answers_rfc7616_md5_example() {
        let challenge = Challenge::parse(&RFC7616.replace("ALGO", "MD5")).unwrap();
        let mut auth = DigestAuth::new(challenge);
        let header = auth.authorization_with_cnonce(
            "Mufasa",
            "Circle of Life",
            "GET",
            "/dir/index.html",
            CNONCE,
        );
        assert_eq!(response_of(&header), "8ca523f5e9506fed4657c9700eebdbec");
        assert!(header.contains("qop=auth, nc=00000001, cnonce=\""));
        assert!(header.contains("algorithm=MD5"));
        assert!(header.ends_with("opaque=\"FQhe/qaU925kfnzjCev0ciny7QMkPqMAFRtzCUYo5tdS\""));
    }

    #[test]
    fn answers_rfc7616_sha256_example() {
        let challenge = Challenge::parse(&RFC7616.replace("ALGO", "SHA-256")).unwrap();
        let mut auth = DigestAuth::new(challenge);
        let header = auth.authorization_with_cnonce(
            "Mufasa",
            "Circle of Life",
            "GET",
            "/dir/index.html",
            CNONCE,
        );
        assert_eq!(
            response_of(&header),
            "753927fa0e85d155564e2e272a28d1802ca10daf4496794697cf8db5856cb6c1"
        );
        assert!(header.contains("algorithm=SHA-256"));
    }

    #[test]
    fn counts_requests_per_nonce() {
        let challenge = Challenge::parse(r#"Digest realm="r", nonce="n", qop="auth""#).unwrap();
        let mut auth = DigestAuth::new(challenge);
        auth.authorization("u", "p", "GET", "/");
        let second = auth.authorization("u", "p", "GET", "/");
        assert!(second.contains("nc=00000002"));
    }

    #[test]
    fn answers_challenge_without_qop() {
        // RFC 2069 style: response = H(HA1:nonce:HA2).
        let challenge = Challenge::parse(r#"Digest realm="r", nonce="n""#).unwrap();
        let mut auth = DigestAuth::new(challenge);
        let header = auth.authorization("u", "p", "POST", "/c");
        let ha1 = Algorithm::Md5.hash("u:r:p");
        let ha2 = Algorithm::Md5.hash("POST:/c");
        assert_eq!(
            response_of(&header),
            Algorithm::Md5.hash(&format!("{ha1}:n:{ha2}"))
        );
        assert!(!header.contains("qop"));
        assert!(!header.contains("opaque"));
    }

    #[test]
    fn quotes_values() {
        let challenge =
            Challenge::parse(r#"Digest realm="a \"quoted\" realm", nonce=plain , stale=TRUE"#)
                .unwrap();
        assert_eq!(challenge.realm, "a \"quoted\" realm");
        assert_eq!(challenge.nonce, "plain");
        let header = DigestAuth::new(challenge).authorization("we\\ird\"", "p", "GET", "/");
        assert!(header.starts_with(r#"Digest username="we\\ird\"", realm="a \"quoted\" realm""#));
    }

    #[test]
    fn reads_unterminated_values() {
        let challenge = Challenge::parse(r#"Digest nonce="n", realm="open\"#).unwrap();
        assert_eq!(challenge.realm, "open");
        let challenge = Challenge::parse("Digest nonce=n, realm=r, key").unwrap();
        assert_eq!(challenge.nonce, "n");
    }

    #[test]
    fn rejects_unsupported_challenges() {
        for header in [
            "Basic realm=\"r\"",
            "Digest",
            "Digest realm=\"r\", nonce=\"n\", algorithm=MD5-sess",
            "Digest realm=\"r\", nonce=\"n\", qop=\"auth-int\"",
            "Digest nonce=\"n\"",
            "Digest realm=\"r\"",
        ] {
            let err = Challenge::parse(header).unwrap_err();
            assert_eq!(
                err.to_string(),
                format!("unsupported authentication challenge: {header}")
            );
        }
    }
}
