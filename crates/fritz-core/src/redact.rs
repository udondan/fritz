//! Hiding secrets in the values a device returns, before they are shown or
//! stored.

use crate::names::{Sensitivity, sensitivity};

/// What a hidden value is replaced with.
pub const HIDDEN: &str = "***";

/// The value of an argument as it may be shown: [`HIDDEN`] if the name
/// points to a secret or a login name, otherwise the value with session IDs
/// in URLs like `/hostlist.lua?sid=…` hidden.
pub fn value(name: &str, value: &str) -> String {
    if sensitivity(name) != Sensitivity::Plain {
        return HIDDEN.to_owned();
    }
    let mut redacted = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(position) = rest.find("sid=") {
        let start = position + "sid=".len();
        let parameter = rest[..position].ends_with(['?', '&']);
        redacted.push_str(&rest[..start]);
        rest = &rest[start..];
        if parameter {
            redacted.push_str(HIDDEN);
            rest = &rest[rest.find(['&', '#']).unwrap_or(rest.len())..];
        }
    }
    redacted.push_str(rest);
    redacted
}

/// An error as it may be shown: without the session ID of a requested URL
/// like `/devicehostlist.lua?sid=…`.
pub fn error(error: tr064::Error) -> tr064::Error {
    match error {
        tr064::Error::Http(error) => tr064::Error::Http(error.without_url()),
        tr064::Error::Status { status, url } => tr064::Error::Status {
            status,
            url: value("", &url),
        },
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use tr064::{Client, Error, Url};

    use super::*;

    #[tokio::test]
    async fn hides_session_ids_in_errors() {
        let status = Error::Status {
            status: 403,
            url: "http://box/devicehostlist.lua?sid=0123abcd".to_owned(),
        };
        assert_eq!(
            error(status).to_string(),
            "HTTP status 403 from http://box/devicehostlist.lua?sid=***"
        );
        let client = Client::builder(Url::parse("http://127.0.0.1:1").unwrap())
            .build()
            .unwrap();
        let http = client
            .get("/devicehostlist.lua?sid=0123abcd")
            .await
            .unwrap_err();
        assert!(http.to_string().contains("0123abcd"));
        assert!(!error(http).to_string().contains("0123abcd"));
        assert!(matches!(
            error(Error::AuthenticationFailed),
            Error::AuthenticationFailed
        ));
    }

    #[test]
    fn hides_secrets_and_session_ids() {
        assert_eq!(value("NewX_AVM-DE_WPSClientPIN", "1234"), "***");
        assert_eq!(value("NewSSID", "home"), "home");
        assert_eq!(value("NewUserName", "provider-login@isp"), "***");
        assert_eq!(value("NewParameterKey", "abc"), "***");
        assert_eq!(value("NewMinCharsPSK", "8"), "8");
        assert_eq!(
            value("NewX_AVM-DE_HostListPath", "/hostlist.lua?sid=0123abcd"),
            "/hostlist.lua?sid=***"
        );
        assert_eq!(
            value(
                "NewCallListURL",
                "http://box/calllist.lua?a=1&sid=ab&max=9#x"
            ),
            "http://box/calllist.lua?a=1&sid=***&max=9#x"
        );
        assert_eq!(value("NewName", "outsid=1, sid=2"), "outsid=1, sid=2");
    }
}
