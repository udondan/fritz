//! Which names hold personal values, and what replaces those values.

use super::Kind;

/// The text that replaces an event log, which names devices, numbers and
/// addresses in free text.
pub(super) const LOG: &str = "(removed by fritz fixture anonymize)";

/// The kind of personal value an output argument like `NewSSID` holds, if any.
pub(super) fn argument(name: &str) -> Option<Kind> {
    let name = name.strip_prefix("New")?;
    let name = name.strip_prefix("X_AVM-DE_").unwrap_or(name);
    Some(match key(name).as_str() {
        "hostname" | "friendlyname" => Kind::Device,
        "ssid" => Kind::Wlan,
        "name" | "alarmclockname" | "alarmclockphonename" | "mountpointname" => Kind::Name,
        "username" => Kind::User,
        "number" | "phonenumber" => Kind::Phone,
        "serialnumber" => Kind::Serial,
        "devicelog" => Kind::Log,
        "email" => Kind::Email,
        "dyndnsname" | "myfritzdyndnsname" | "ntpserver1" | "ntpserver2" => Kind::Server,
        "url" | "connectionrequesturl" | "hosturl" => Kind::Url,
        "path" | "endpointid" => Kind::Id,
        "provider" | "pppoeacname" => Kind::Provider,
        _ => return None,
    })
}

/// The kind of personal value an XML element or attribute holds, if any,
/// for lists like `<List><Item><Name>…</Name></Item></List>` and for the
/// device description. Its `friendlyName` is the model name, not a name
/// given by the owner.
pub(super) fn element(name: &str) -> Option<Kind> {
    let name = name.rsplit(':').next().unwrap_or(name);
    Some(match key(name).as_str() {
        "name" => Kind::Name,
        "username" | "voipusername" => Kind::User,
        "number" | "voipnumber" => Kind::Phone,
        "serialnumber" => Kind::Serial,
        "voipregistrar" | "voipoutboundproxy" | "voipstunserver" => Kind::Server,
        _ => return None,
    })
}

/// A name in lower case without separators, so that `NewHostName`,
/// `NewHostname` and `last_user` compare as `hostname` and `lastuser`.
fn key(name: &str) -> String {
    name.chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// The placeholder pattern of a kind, with `{}` for the number. Kinds
/// without a pattern have their own placeholders.
pub(super) fn template(kind: Kind) -> Option<&'static str> {
    Some(match kind {
        Kind::Device => "device-{}",
        Kind::Wlan => "wlan-{}",
        Kind::Name => "name-{}",
        Kind::User => "user-{}",
        Kind::Phone => "phone-{}",
        Kind::Provider => "provider-{}",
        Kind::Id => "id-{}",
        Kind::Serial => "serial-{}",
        Kind::Server => "server-{}.example.net",
        Kind::Url => "https://url-{}.example.net/",
        Kind::Email => "user-{}@example.com",
        Kind::MyFritz => "example-{}.myfritz.net",
        _ => return None,
    })
}

/// The `n`th placeholder of a kind with a [template].
pub(super) fn placeholder(kind: Kind, n: u32) -> String {
    template(kind)
        .expect("only kinds with a template are numbered")
        .replace("{}", &n.to_string())
}

/// Whether a value is a placeholder made from the [template] of its kind.
pub(super) fn is_placeholder(kind: Kind, value: &str) -> bool {
    let Some((prefix, suffix)) = template(kind).and_then(|t| t.split_once("{}")) else {
        return false;
    };
    value
        .strip_prefix(prefix)
        .and_then(|v| v.strip_suffix(suffix))
        .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_personal_arguments() {
        assert_eq!(argument("NewHostName"), Some(Kind::Device));
        assert_eq!(argument("NewX_AVM-DE_FriendlyName"), Some(Kind::Device));
        assert_eq!(argument("NewSSID"), Some(Kind::Wlan));
        assert_eq!(argument("NewDeviceLog"), Some(Kind::Log));
        assert_eq!(argument("NewX_AVM-DE_PPPoEACName"), Some(Kind::Provider));
        assert_eq!(argument("NewX_AVM-DE_AlarmClockName"), Some(Kind::Name));
        assert_eq!(argument("NewX_AVM-DE_PhoneNumber"), Some(Kind::Phone));
        assert_eq!(argument("NewNumber"), Some(Kind::Phone));
        assert_eq!(argument("NewDynDNSName"), Some(Kind::Server));
        assert_eq!(argument("NewNTPServer1"), Some(Kind::Server));
        assert_eq!(argument("NewNTPServer2"), Some(Kind::Server));
        assert_eq!(
            argument("NewX_AVM-DE_MyFritzDynDNSName"),
            Some(Kind::Server)
        );
        assert_eq!(argument("NewConnectionRequestURL"), Some(Kind::Url));
        assert_eq!(argument("NewPath"), Some(Kind::Id));
        assert_eq!(argument("NewEnable"), None);
        assert_eq!(argument("SSID"), None);
    }

    #[test]
    fn finds_personal_elements() {
        assert_eq!(element("Username"), Some(Kind::User));
        assert_eq!(element("last_user"), None);
        assert_eq!(element("VoIPNumber"), Some(Kind::Phone));
        assert_eq!(element("s:Name"), Some(Kind::Name));
        assert_eq!(element("friendlyName"), None);
        assert_eq!(element("serialNumber"), Some(Kind::Serial));
        assert_eq!(element("VoIPSTUNServer"), Some(Kind::Server));
        assert_eq!(element("path"), None);
    }

    #[test]
    fn recognizes_placeholders() {
        assert_eq!(placeholder(Kind::Server, 3), "server-3.example.net");
        assert!(is_placeholder(Kind::Server, "server-3.example.net"));
        assert!(is_placeholder(Kind::Device, "device-12"));
        assert!(!is_placeholder(Kind::Device, "device-"));
        assert!(!is_placeholder(Kind::Device, "device-x"));
        assert!(!is_placeholder(Kind::Device, "laptop"));
        assert!(!is_placeholder(Kind::Mac, "AA:BB:CC:00:00:01"));
    }
}
