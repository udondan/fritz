//! HTTP Digest authentication with MD5 and `qop=auth`, as FRITZ!Box
//! devices ask for it.

use std::fmt::Write as _;

use md5::{Digest as _, Md5};

/// The realm of FRITZ!Box devices.
pub(crate) const REALM: &str = "F!Box SOAP-Auth";

/// The `WWW-Authenticate` header of a 401 response.
pub(crate) fn challenge(nonce: &str) -> String {
    format!(r#"Digest realm="{REALM}", nonce="{nonce}", algorithm=MD5, qop="auth""#)
}

/// What a request must answer to.
#[derive(Debug, Clone)]
pub(crate) struct Expected<'a> {
    pub(crate) username: &'a str,
    pub(crate) password: &'a str,
    pub(crate) nonce: &'a str,
    pub(crate) method: &'a str,
    /// The path and query of the request.
    pub(crate) uri: &'a str,
}

/// Whether an `Authorization` header answers the challenge with the right
/// credentials.
pub(crate) fn verify(header: Option<&str>, expected: &Expected<'_>) -> bool {
    let Some(params) = header.and_then(|h| h.strip_prefix("Digest ")) else {
        return false;
    };
    let params = parse_params(params);
    let get = |name: &str| {
        params
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map_or("", |(_, v)| v.as_str())
    };
    let Expected {
        username,
        password,
        nonce,
        method,
        uri,
    } = *expected;
    let ha1 = md5_hex(&format!("{username}:{REALM}:{password}"));
    let ha2 = md5_hex(&format!("{method}:{uri}"));
    let response = md5_hex(&format!(
        "{ha1}:{nonce}:{}:{}:auth:{ha2}",
        get("nc"),
        get("cnonce")
    ));
    let given = (
        get("username"),
        get("nonce"),
        get("uri"),
        get("qop"),
        get("response"),
    );
    given == (username, nonce, uri, "auth", response.as_str())
}

/// The parameters of a header like `a="x, y", b=z`.
fn parse_params(mut rest: &str) -> Vec<(String, String)> {
    let mut params = Vec::new();
    loop {
        rest = rest.trim_start_matches([' ', ',']);
        let Some((name, after)) = rest.split_once('=') else {
            return params;
        };
        let (value, after) = if let Some(quoted) = after.strip_prefix('"') {
            unquote(quoted)
        } else {
            let end = after.find(',').unwrap_or(after.len());
            (after[..end].trim().to_owned(), &after[end..])
        };
        params.push((name.trim().to_owned(), value));
        rest = after;
    }
}

/// The text up to the closing quote, with escapes resolved, and the rest
/// after it.
fn unquote(text: &str) -> (String, &str) {
    let mut value = String::new();
    let mut chars = text.char_indices();
    while let Some((i, c)) = chars.next() {
        match c {
            '"' => return (value, &text[i + 1..]),
            '\\' => value.extend(chars.next().map(|(_, c)| c)),
            _ => value.push(c),
        }
    }
    (value, "")
}

fn md5_hex(data: &str) -> String {
    Md5::digest(data.as_bytes())
        .iter()
        .fold(String::new(), |mut hex, byte| {
            let _ = write!(hex, "{byte:02x}");
            hex
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_parameters() {
        assert_eq!(
            parse_params(r#"username="a \"b\", c", nc=00000001 , qop=auth, cut="x\"#),
            [
                ("username".to_owned(), r#"a "b", c"#.to_owned()),
                ("nc".to_owned(), "00000001".to_owned()),
                ("qop".to_owned(), "auth".to_owned()),
                ("cut".to_owned(), "x".to_owned()),
            ]
        );
        assert_eq!(parse_params("broken"), []);
    }

    #[test]
    fn rejects_other_schemes() {
        let expected = Expected {
            username: "u",
            password: "p",
            nonce: "n",
            method: "POST",
            uri: "/",
        };
        assert!(!verify(None, &expected));
        assert!(!verify(Some("Basic dTpw"), &expected));
    }
}
