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

#[cfg(test)]
mod tests {
    use super::*;

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
