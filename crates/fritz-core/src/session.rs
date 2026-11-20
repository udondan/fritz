//! The session of the web interface, which the Smart Home REST API needs:
//! a login with `login_sid.lua` and a session ID that stays in memory.
//!
//! The login answers the challenge of the device with PBKDF2, or with MD5
//! for devices without it. Like the TR-064 client, it never retries after a
//! rejected login, so that a wrong password does not lock the account. The
//! session ID is never shown: it is kept as a secret, and is sent in a header
//! instead of the URL.

use std::fmt::Write as _;

use md5::{Digest as _, Md5};
use secrecy::{ExposeSecret as _, SecretString};
use sha2::Sha256;
use tr064::Client;

use crate::status::Right;

/// The login page, in the version with PBKDF2.
pub const LOGIN: &str = "/login_sid.lua?version=2";

/// The session ID of an answer without a session.
const NO_SESSION: &str = "0000000000000000";

/// The login or a request of the session failed.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Tr064(#[from] tr064::Error),
    /// The device answered with something unexpected.
    #[error("invalid answer from {what}: {reason}")]
    Invalid { what: &'static str, reason: String },
    /// The device blocks logins after failed attempts.
    #[error("the device blocks logins for {0} more seconds after failed attempts")]
    Blocked(u64),
    /// The device rejected the user or the password.
    #[error("the device rejected the user or the password{}", blocked(*.0))]
    Rejected(u64),
}

fn blocked(seconds: u64) -> String {
    if seconds == 0 {
        String::new()
    } else {
        format!(", and blocks logins for {seconds} seconds")
    }
}

/// An open session.
#[derive(Debug)]
pub struct Session {
    client: Client,
    sid: SecretString,
    rights: Vec<Right>,
}

impl Session {
    /// Logs in with the user and the password. `client` is a client for the
    /// web interface, without credentials. Sends the password only as the
    /// answer to the challenge, and logs in at most once.
    ///
    /// # Errors
    ///
    /// [`Error::Blocked`] if the device blocks logins, [`Error::Rejected`]
    /// if it rejects the login, and the errors of the requests.
    pub async fn login(
        client: Client,
        username: &str,
        password: &SecretString,
    ) -> Result<Self, Error> {
        let info = SessionInfo::parse(&client.get(LOGIN).await?)?;
        if info.block_time > 0 {
            return Err(Error::Blocked(info.block_time));
        }
        let response = response(&info.challenge, password.expose_secret())?;
        let form = [("username", username), ("response", response.as_str())];
        let info = SessionInfo::parse(&client.post_form(LOGIN, &form).await?)?;
        if info.sid == NO_SESSION {
            return Err(Error::Rejected(info.block_time));
        }
        Ok(Self {
            client,
            sid: SecretString::from(info.sid),
            rights: info.rights,
        })
    }

    /// What the user may do in the web interface, like `HomeAuto: read`.
    pub fn rights(&self) -> &[Right] {
        &self.rights
    }

    /// Fetches a document of the REST API, like
    /// `/api/v0/smarthome/overview`, with the session.
    ///
    /// # Errors
    ///
    /// The errors of the request.
    pub async fn get(&self, path: &str) -> Result<String, Error> {
        let authorization = format!("AVM-SID {}", self.sid.expose_secret());
        let headers = [("Authorization", authorization.as_str())];
        Ok(self.client.get_with_headers(path, &headers).await?)
    }

    /// Ends the session. Without it, the device ends the session after a
    /// while without requests.
    ///
    /// # Errors
    ///
    /// The errors of the request.
    pub async fn logout(self) -> Result<(), Error> {
        let form = [("logout", "1"), ("sid", self.sid.expose_secret())];
        self.client.post_form(LOGIN, &form).await?;
        Ok(())
    }
}

/// The answer of the login page.
#[derive(Debug, PartialEq, Eq)]
struct SessionInfo {
    sid: String,
    challenge: String,
    /// Seconds until the device accepts the next login.
    block_time: u64,
    rights: Vec<Right>,
}

impl SessionInfo {
    fn parse(xml: &str) -> Result<Self, Error> {
        let invalid = |reason: String| Error::Invalid {
            what: "the login page",
            reason,
        };
        let document = roxmltree::Document::parse(xml).map_err(|e| invalid(e.to_string()))?;
        let root = document.root_element();
        if !root.has_tag_name("SessionInfo") {
            return Err(invalid(format!("unexpected <{}>", root.tag_name().name())));
        }
        let text = |name: &str| {
            root.children()
                .find(|n| n.has_tag_name(name))
                .and_then(|n| n.text())
                .unwrap_or_default()
                .trim()
                .to_owned()
        };
        let sid = text("SID");
        if sid.len() != NO_SESSION.len() || !sid.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(invalid("no session ID".to_owned()));
        }
        let block_time = text("BlockTime");
        let block_time = block_time
            .parse()
            .map_err(|_| invalid(format!("block time {block_time:?}")))?;
        let rights = root
            .children()
            .filter(|n| n.has_tag_name("Rights"))
            .flat_map(|rights| rights.children().filter(roxmltree::Node::is_element))
            .collect::<Vec<_>>();
        let rights = rights
            .chunks(2)
            .filter_map(|pair| match pair {
                [name, access] if name.has_tag_name("Name") && access.has_tag_name("Access") => {
                    Some(Right {
                        area: name.text().unwrap_or_default().to_owned(),
                        access: match access.text() {
                            Some("1") => "read".to_owned(),
                            Some("2") => "readwrite".to_owned(),
                            other => other.unwrap_or_default().to_owned(),
                        },
                    })
                }
                _ => None,
            })
            .collect();
        Ok(Self {
            sid,
            challenge: text("Challenge"),
            block_time,
            rights,
        })
    }
}

/// The answer to a challenge: PBKDF2 for a challenge like
/// `2$60000$<salt1>$6000$<salt2>`, else MD5.
fn response(challenge: &str, password: &str) -> Result<String, Error> {
    if !challenge.starts_with("2$") {
        return Ok(md5_response(challenge, password));
    }
    let invalid = || Error::Invalid {
        what: "the login page",
        reason: format!("challenge {challenge:?}"),
    };
    let parts: Vec<&str> = challenge.split('$').collect();
    let [_, iterations1, salt1, iterations2, salt2] = parts[..] else {
        return Err(invalid());
    };
    let number = |text: &str| text.parse::<u32>().ok().filter(|n| *n > 0);
    let (Some(iterations1), Some(salt1), Some(iterations2), Some(salt2)) = (
        number(iterations1),
        from_hex(salt1),
        number(iterations2),
        from_hex(salt2),
    ) else {
        return Err(invalid());
    };
    let first = pbkdf2::pbkdf2_hmac_array::<Sha256, 32>(password.as_bytes(), &salt1, iterations1);
    let second = pbkdf2::pbkdf2_hmac_array::<Sha256, 32>(&first, &salt2, iterations2);
    Ok(format!("{}${}", to_hex(&salt2), to_hex(&second)))
}

/// The MD5 answer: the challenge, `-`, and the MD5 hash of the challenge,
/// `-` and the password in UTF-16LE. Characters beyond Latin-1 count as `.`.
fn md5_response(challenge: &str, password: &str) -> String {
    let text = format!("{challenge}-{password}");
    let bytes: Vec<u8> = text
        .chars()
        .map(|c| if u32::from(c) > 255 { '.' } else { c })
        .flat_map(|c| {
            let mut units = [0; 2];
            c.encode_utf16(&mut units);
            units[0].to_le_bytes()
        })
        .collect();
    format!("{challenge}-{}", to_hex(&Md5::digest(bytes)))
}

fn from_hex(text: &str) -> Option<Vec<u8>> {
    if text.is_empty() || !text.len().is_multiple_of(2) {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(text.get(i..i + 2)?, 16).ok())
        .collect()
}

/// Bytes as lowercase hex digits.
pub(crate) fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().fold(String::new(), |mut text, byte| {
        let _ = write!(text, "{byte:02x}");
        text
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn answers_pbkdf2_challenges() {
        // The example of the session ID documentation, checked with Python's
        // hashlib.pbkdf2_hmac.
        assert_eq!(
            response("2$10000$5A1711$2000$5A1722", "1example!").unwrap(),
            EXAMPLE_PBKDF2
        );
    }

    #[test]
    fn answers_md5_challenges() {
        // The example of the session ID documentation.
        assert_eq!(
            response("1234567z", "äbc").unwrap(),
            "1234567z-9e224a41eeefa284df7bb0f26c2913e2"
        );
        // Characters beyond Latin-1 count as `.`.
        assert_eq!(
            response("1234567z", "€").unwrap(),
            md5_response("1234567z", ".")
        );
    }

    #[test]
    fn rejects_invalid_challenges() {
        for challenge in [
            "2$10000$5A1711$2000",
            "2$0$5A1711$2000$5A1722",
            "2$x$5A1711$2000$5A1722",
            "2$10000$5A171$2000$5A1722",
            "2$10000$5A1711$2000$",
            "2$10000$5A1711$2000$zz",
        ] {
            let error = response(challenge, "secret").unwrap_err();
            assert_eq!(
                error.to_string(),
                format!("invalid answer from the login page: challenge {challenge:?}")
            );
        }
    }

    #[test]
    fn converts_hex() {
        assert_eq!(from_hex("00ff7A"), Some(vec![0, 255, 122]));
        assert_eq!(from_hex("ä0"), None);
        assert_eq!(to_hex(&[0, 255, 122]), "00ff7a");
    }

    #[test]
    fn parses_session_infos() {
        let info = SessionInfo::parse(
            "<SessionInfo><SID>0123456789abcdef</SID><Challenge>2$1$aa$1$bb</Challenge>\
             <BlockTime>0</BlockTime><Rights><Name>Dial</Name><Access>2</Access>\
             <Name>HomeAuto</Name><Access>1</Access><Name>App</Name><Access>3</Access>\
             <Name>Odd</Name></Rights><Users><User last=\"1\">agent</User></Users>\
             </SessionInfo>",
        )
        .unwrap();
        assert_eq!(info.sid, "0123456789abcdef");
        assert_eq!(info.challenge, "2$1$aa$1$bb");
        assert_eq!(info.block_time, 0);
        let rights: Vec<_> = info
            .rights
            .iter()
            .map(|r| format!("{}: {}", r.area, r.access))
            .collect();
        assert_eq!(rights, ["Dial: readwrite", "HomeAuto: read", "App: 3"]);
    }

    #[test]
    fn rejects_invalid_session_infos() {
        for (xml, reason) in [
            ("<SessionInfo>", "never closed"),
            ("<Other/>", "unexpected <Other>"),
            ("<SessionInfo><SID>0</SID></SessionInfo>", "no session ID"),
            (
                "<SessionInfo><SID>000000000000000g</SID></SessionInfo>",
                "no session ID",
            ),
            (
                "<SessionInfo><SID>0000000000000000</SID><BlockTime>x</BlockTime></SessionInfo>",
                "block time \"x\"",
            ),
        ] {
            let error = SessionInfo::parse(xml).unwrap_err().to_string();
            assert!(
                error.starts_with("invalid answer from the login page: ") && error.contains(reason),
                "{xml}: {error}"
            );
        }
    }

    #[test]
    fn explains_rejections() {
        assert_eq!(
            Error::Rejected(0).to_string(),
            "the device rejected the user or the password"
        );
        assert_eq!(
            Error::Rejected(8).to_string(),
            "the device rejected the user or the password, and blocks logins for 8 seconds"
        );
    }

    const EXAMPLE_PBKDF2: &str =
        "5a1722$1798a1672bca7c6463d6b245f82b53703b0f50813401b03e4045a5861e689adb";
}
