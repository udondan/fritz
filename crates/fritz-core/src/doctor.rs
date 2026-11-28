//! The checks of the setup: their results, and the rules that need no
//! device, like which rights the commands need.
//!
//! A check never carries a secret: no password, session ID or key, only
//! where the password came from.

use std::net::IpAddr;

use serde::Serialize;

use crate::status::Right;

/// The result of a check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Ok,
    /// Works, but not everything.
    Warn,
    Fail,
    /// Not checked, because an earlier check failed or it does not apply.
    Skip,
}

impl Status {
    /// The name, like `ok`.
    pub fn name(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Warn => "warn",
            Self::Fail => "fail",
            Self::Skip => "skip",
        }
    }
}

/// One check: its name, its result, what was found and, on a warning or a
/// failure, how to fix it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Check {
    /// Like `Password`.
    pub check: &'static str,
    pub status: Status,
    pub detail: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

impl Check {
    pub fn ok(check: &'static str, detail: impl Into<String>) -> Self {
        Self::new(check, Status::Ok, detail.into(), None)
    }

    pub fn skip(check: &'static str, detail: impl Into<String>) -> Self {
        Self::new(check, Status::Skip, detail.into(), None)
    }

    pub fn warn(check: &'static str, detail: impl Into<String>, hint: impl Into<String>) -> Self {
        Self::new(check, Status::Warn, detail.into(), Some(hint.into()))
    }

    pub fn fail(check: &'static str, detail: impl Into<String>, hint: impl Into<String>) -> Self {
        Self::new(check, Status::Fail, detail.into(), Some(hint.into()))
    }

    fn new(check: &'static str, status: Status, detail: String, hint: Option<String>) -> Self {
        Self {
            check,
            status,
            detail,
            hint,
        }
    }

    /// Whether the check failed.
    pub fn failed(&self) -> bool {
        self.status == Status::Fail
    }
}

/// Whether an address belongs to the local network: loopback, private,
/// link-local or, for IPv6, unique local.
pub fn is_local(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => ip.is_loopback() || ip.is_private() || ip.is_link_local(),
        IpAddr::V6(ip) => match ip.to_ipv4_mapped() {
            Some(ip) => is_local(IpAddr::V4(ip)),
            None => ip.is_loopback() || ip.is_unique_local() || ip.is_unicast_link_local(),
        },
    }
}

/// The check `Host`: `host` resolved to `addresses`. Fails without an
/// address, or if none is local, since TR-064 belongs in the home network.
/// Public addresses are counted, not shown.
pub fn host(name: &'static str, host: &str, addresses: &[IpAddr]) -> Check {
    let local: Vec<String> = addresses
        .iter()
        .filter(|ip| is_local(**ip))
        .map(ToString::to_string)
        .collect();
    let public = addresses.len() - local.len();
    let hint = "set --host to the local address of the device, like 192.168.178.1";
    if addresses.is_empty() {
        return Check::fail(name, format!("{host} does not resolve"), hint);
    }
    if local.is_empty() {
        let detail = format!("{host} resolves only to public addresses");
        return Check::fail(name, detail, hint);
    }
    let also = match public {
        0 => String::new(),
        n => format!(" and {n} public"),
    };
    Check::ok(name, format!("{host} is {}{also}", local.join(", ")))
}

/// The rights the commands need, what they are for, and whether a missing
/// one fails or only warns.
const NEEDS: [(&str, &str, Status); 3] = [
    (
        "BoxAdmin",
        "most commands, like overview, hosts, wlan and internet",
        Status::Fail,
    ),
    ("Phone", "telephony, calls and phonebook", Status::Warn),
    ("HomeAuto", "smarthome", Status::Warn),
];

/// The check `Rights`: whether the user has the rights the commands need.
/// The right for the settings includes the one for the phone.
pub fn rights(name: &'static str, rights: &[Right]) -> Check {
    if rights.is_empty() {
        return Check::skip(name, "the device reports no rights for the user");
    }
    let has = |area: &str| {
        rights
            .iter()
            .any(|r| r.area == area && !r.access.is_empty() && r.access != "none")
    };
    let covered = |area: &str| has(area) || (area == "Phone" && has("BoxAdmin"));
    let missing: Vec<_> = NEEDS.iter().filter(|(area, ..)| !covered(area)).collect();
    let all: Vec<String> = rights
        .iter()
        .map(|r| format!("{}: {}", r.area, r.access))
        .collect();
    if missing.is_empty() {
        return Check::ok(name, all.join(", "));
    }
    let detail = missing
        .iter()
        .map(|(area, commands, _)| format!("{area} missing, needed by {commands}"))
        .collect::<Vec<_>>()
        .join("; ");
    let hint = "give the user these rights in System > FRITZ!Box Users, see docs/box-setup.md";
    if missing.iter().any(|(.., status)| *status == Status::Fail) {
        Check::fail(name, detail, hint)
    } else {
        Check::warn(name, detail, hint)
    }
}

/// The check `Two-factor`: whether changes need a confirmation on the
/// device. That only matters to builds that can change the device.
pub fn two_factor(name: &'static str, enabled: Option<bool>, changes: bool) -> Check {
    match (enabled, changes) {
        (None, _) => Check::skip(name, "the device does not report it"),
        (Some(false), _) => Check::ok(name, "off: changes need no confirmation on the device"),
        (Some(true), false) => Check::ok(
            name,
            "on: some changes need a confirmation on the device; reads are not affected",
        ),
        (Some(true), true) => Check::warn(
            name,
            "on: some changes need a confirmation on the device, which fritz never gives",
            "such changes fail with confirmation_required; make them in the web interface",
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn right(area: &str, access: &str) -> Right {
        Right {
            area: area.to_owned(),
            access: access.to_owned(),
        }
    }

    #[test]
    fn tells_local_from_public_addresses() {
        for ip in [
            "127.0.0.1",
            "192.168.178.1",
            "10.0.0.1",
            "169.254.1.1",
            "::1",
            "fd00::1",
            "fe80::1",
            "::ffff:192.168.178.1",
        ] {
            assert!(is_local(ip.parse().unwrap()), "{ip}");
        }
        for ip in ["8.8.8.8", "2001:db8::1", "::ffff:8.8.8.8"] {
            assert!(!is_local(ip.parse().unwrap()), "{ip}");
        }
    }

    #[test]
    fn checks_the_host() {
        let local = "192.168.178.1".parse().unwrap();
        let public = "203.0.113.1".parse().unwrap();
        let check = host("Host", "fritz.box", &[local, public]);
        assert_eq!(check.status, Status::Ok);
        assert_eq!(check.detail, "fritz.box is 192.168.178.1 and 1 public");
        assert_eq!(host("Host", "box", &[local]).detail, "box is 192.168.178.1");
        let check = host("Host", "box.example", &[public]);
        assert!(check.failed());
        assert!(!check.detail.contains("203.0.113.1"), "{}", check.detail);
        let check = host("Host", "box.example", &[]);
        assert_eq!(check.detail, "box.example does not resolve");
        assert!(check.hint.unwrap().contains("--host"));
    }

    #[test]
    fn checks_the_rights() {
        let all = [right("BoxAdmin", "readwrite"), right("HomeAuto", "read")];
        let check = rights("Rights", &all);
        assert_eq!(check.status, Status::Ok);
        assert_eq!(check.detail, "BoxAdmin: readwrite, HomeAuto: read");

        let check = rights("Rights", &[right("BoxAdmin", "readwrite")]);
        assert_eq!(check.status, Status::Warn);
        assert_eq!(check.detail, "HomeAuto missing, needed by smarthome");

        let check = rights("Rights", &[right("HomeAuto", "readwrite")]);
        assert!(check.failed());
        assert_eq!(
            check.detail,
            "BoxAdmin missing, needed by most commands, like overview, hosts, wlan and \
             internet; Phone missing, needed by telephony, calls and phonebook"
        );
        let check = rights("Rights", &[right("BoxAdmin", "none")]);
        assert!(check.failed());
        assert_eq!(rights("Rights", &[]).status, Status::Skip);
    }

    #[test]
    fn checks_the_second_factor() {
        assert_eq!(two_factor("2FA", None, true).status, Status::Skip);
        assert_eq!(two_factor("2FA", Some(false), true).status, Status::Ok);
        assert_eq!(two_factor("2FA", Some(true), false).status, Status::Ok);
        let check = two_factor("2FA", Some(true), true);
        assert_eq!(check.status, Status::Warn);
        assert!(check.hint.is_some());
    }

    #[test]
    fn names_the_statuses() {
        let names: Vec<_> = [Status::Ok, Status::Warn, Status::Fail, Status::Skip]
            .iter()
            .map(|s| s.name())
            .collect();
        assert_eq!(names, ["ok", "warn", "fail", "skip"]);
        let json = serde_json::to_value(Check::warn("Host", "detail", "hint")).unwrap();
        assert_eq!(
            json,
            serde_json::json!({"check": "Host", "status": "warn", "detail": "detail", "hint": "hint"})
        );
    }
}
