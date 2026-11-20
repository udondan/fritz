//! Replacing personal values with placeholders, the same value with the
//! same placeholder.

use std::collections::HashMap;
use std::net::{Ipv4Addr, Ipv6Addr};
use std::sync::LazyLock;

use regex::{Captures, Regex};

use super::Kind;
use super::addresses::{self, Ipv6Pool};
use super::leftovers::{self, MIN_LENGTH};
use super::rules;

fn regex(pattern: &str) -> Regex {
    Regex::new(pattern).expect("valid pattern")
}

static UUID: LazyLock<Regex> = LazyLock::new(|| {
    regex(r"[0-9A-Fa-f]{8}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{12}")
});
static MAC: LazyLock<Regex> =
    LazyLock::new(|| regex(r"[0-9A-Fa-f]{2}([:-])[0-9A-Fa-f]{2}(?:[:-][0-9A-Fa-f]{2}){4}"));
static EMAIL: LazyLock<Regex> =
    LazyLock::new(|| regex(r"[A-Za-z0-9._%+-]+@[A-Za-z0-9-]+(?:\.[A-Za-z0-9-]+)*\.[A-Za-z]{2,}"));
static MYFRITZ: LazyLock<Regex> =
    LazyLock::new(|| regex(r"(?i)[a-z0-9-]+(?:\.[a-z0-9-]+)*\.myfritz\.net"));
static IPV4: LazyLock<Regex> = LazyLock::new(|| regex(r"\d{1,3}(?:\.\d{1,3}){3}"));
static IPV6: LazyLock<Regex> =
    LazyLock::new(|| regex(r"[0-9A-Fa-f]{0,4}(?::[0-9A-Fa-f]{0,4}){2,7}"));
/// An XML element with text and no child elements.
static LEAF: LazyLock<Regex> =
    LazyLock::new(|| regex(r"<([A-Za-z_][\w.:-]*)([^<>]*)>([^<]+)</([A-Za-z_][\w.:-]*)>"));
static ATTRIBUTE: LazyLock<Regex> = LazyLock::new(|| regex(r#"([A-Za-z_][\w.:-]*)="([^"]*)""#));

/// The domains of e-mail placeholders and other examples, which stay.
const EXAMPLE_DOMAINS: &[&str] = &["example.com", "example.net", "example.org"];

/// Replaces personal values and remembers which ones it replaced.
#[derive(Debug, Default)]
pub(super) struct Mapper {
    /// The service descriptions in lower case, whose words are protocol
    /// words, not personal values.
    protocol: String,
    placeholders: HashMap<(Kind, String), String>,
    counters: HashMap<&'static str, u32>,
    /// The kinds and original texts of all replaced values, each once.
    pub(super) replaced: Vec<(Kind, String)>,
}

impl Mapper {
    /// A mapper for a capture with the given service descriptions.
    pub(super) fn new<'a>(descriptions: impl IntoIterator<Item = &'a str>) -> Self {
        let protocol = descriptions.into_iter().collect::<Vec<_>>().join("\n");
        Self {
            protocol: protocol.to_lowercase(),
            ..Self::default()
        }
    }

    /// A mapper with the same service descriptions that has not replaced
    /// anything yet.
    pub(super) fn fresh(&self) -> Self {
        Self {
            protocol: self.protocol.clone(),
            ..Self::default()
        }
    }

    /// The placeholder for a value. `key` identifies the value, so that
    /// different spellings get the same placeholder; `pool` names the
    /// counter the placeholder is numbered with.
    fn map(
        &mut self,
        kind: Kind,
        pool: &'static str,
        key: String,
        original: &str,
        make: impl FnOnce(u32) -> String,
    ) -> String {
        if let Some(placeholder) = self.placeholders.get(&(kind, key.clone())) {
            return placeholder.clone();
        }
        let counter = self.counters.entry(pool).or_insert(0);
        *counter += 1;
        let placeholder = make(*counter);
        self.placeholders.insert((kind, key), placeholder.clone());
        self.replaced.push((kind, original.to_owned()));
        placeholder
    }

    /// Replaces addresses, e-mail addresses, MyFRITZ! names and UUIDs
    /// anywhere in a text.
    pub(super) fn text(&mut self, text: &str) -> String {
        let text = substitute(&UUID, text, "-", |m| self.uuid(m));
        let text = substitute(&MAC, &text, ":-", |m| self.mac(m));
        let text = substitute(&EMAIL, &text, ".-_+", |m| self.email(m));
        let text = substitute(&MYFRITZ, &text, ".-@", |m| self.myfritz(m));
        let text = substitute(&IPV4, &text, ".", |m| self.ipv4(m));
        substitute(&IPV6, &text, ":.", |m| self.ipv6(m))
    }

    /// Replaces the personal values in an XML document: those of elements
    /// and attributes with personal names, then those [anywhere](Self::text).
    pub(super) fn xml(&mut self, xml: &str) -> String {
        let xml = ATTRIBUTE.replace_all(xml, |c: &Captures| match rules::element(&c[1]) {
            Some(kind) => format!(r#"{}="{}""#, &c[1], self.named(kind, &c[2])),
            None => c[0].to_owned(),
        });
        let xml = LEAF.replace_all(&xml, |c: &Captures| match rules::element(&c[1]) {
            Some(kind) if c[1] == c[4] => {
                format!(
                    "<{}{}>{}</{}>",
                    &c[1],
                    &c[2],
                    self.named(kind, &c[3]),
                    &c[4]
                )
            }
            _ => c[0].to_owned(),
        });
        self.text(&xml)
    }

    /// Replaces the value of an output argument like `NewSSID`, depending on
    /// its name, and the personal values [anywhere](Self::text) in it.
    pub(super) fn argument(&mut self, name: &str, value: &str) -> String {
        if value.starts_with('<') {
            return self.xml(value);
        }
        let value = match rules::argument(name).or_else(|| rules::field(name)) {
            Some(kind) => self.named(kind, value),
            None => value.to_owned(),
        };
        self.text(&value)
    }

    /// Replaces a value known to be of a kind because of its name, unless
    /// it is a placeholder or a protocol word like the default filter
    /// profile `Standard`.
    fn named(&mut self, kind: Kind, value: &str) -> String {
        let protocol_word =
            value.chars().count() >= MIN_LENGTH && leftovers::contains_word(&self.protocol, value);
        if value.is_empty() || value == "***" || protocol_word || rules::is_placeholder(kind, value)
        {
            return value.to_owned();
        }
        match kind {
            Kind::Log if value == rules::LOG => value.to_owned(),
            Kind::Log => {
                self.replaced.push((Kind::Log, String::new()));
                rules::LOG.to_owned()
            }
            Kind::Serial if addresses::mac_digits(value).is_some() => {
                self.mac(value).unwrap_or_else(|| value.to_owned())
            }
            Kind::Url if !value.contains("://") => value.to_owned(),
            Kind::Email => self.email(value).unwrap_or_else(|| value.to_owned()),
            _ => self.map(kind, kind.label(), value.to_owned(), value, |n| {
                rules::placeholder(kind, n)
            }),
        }
    }

    fn uuid(&mut self, value: &str) -> Option<String> {
        if addresses::keeps_uuid(value) {
            return None;
        }
        Some(self.map(
            Kind::Uuid,
            "uuid",
            value.to_ascii_lowercase(),
            value,
            addresses::uuid,
        ))
    }

    /// Replaces a MAC address, keeping its separator, or none for a serial
    /// number made of the 12 digits.
    fn mac(&mut self, value: &str) -> Option<String> {
        let digits = addresses::mac_digits(value)?;
        if addresses::keeps_mac(&digits) {
            return None;
        }
        let separator = if value.len() == 12 { "" } else { &value[2..3] };
        let placeholder = self.map(Kind::Mac, "mac", digits, value, |n| addresses::mac(n, ":"));
        Some(placeholder.replace(':', separator))
    }

    fn email(&mut self, value: &str) -> Option<String> {
        let (_, domain) = value.rsplit_once('@')?;
        if EXAMPLE_DOMAINS.contains(&domain.to_ascii_lowercase().as_str()) {
            return None;
        }
        Some(self.map(
            Kind::Email,
            "email",
            value.to_ascii_lowercase(),
            value,
            |n| rules::placeholder(Kind::Email, n),
        ))
    }

    fn myfritz(&mut self, value: &str) -> Option<String> {
        if rules::is_placeholder(Kind::MyFritz, &value.to_ascii_lowercase()) {
            return None;
        }
        Some(self.map(
            Kind::MyFritz,
            "myfritz",
            value.to_ascii_lowercase(),
            value,
            |n| rules::placeholder(Kind::MyFritz, n),
        ))
    }

    fn ipv4(&mut self, value: &str) -> Option<String> {
        let address: Ipv4Addr = value.parse().ok()?;
        if addresses::keeps_ipv4(address) {
            return None;
        }
        let internal = addresses::is_internal(address);
        let pool = if internal { "ipv4-internal" } else { "ipv4" };
        Some(self.map(Kind::Ipv4, pool, address.to_string(), value, |n| {
            addresses::ipv4(internal, n).to_string()
        }))
    }

    fn ipv6(&mut self, value: &str) -> Option<String> {
        let address: Ipv6Addr = value.parse().ok()?;
        let pool = addresses::ipv6_pool(address)?;
        let counter = match pool {
            Ipv6Pool::LinkLocal => "ipv6-link-local",
            Ipv6Pool::Prefix => "ipv6-prefix",
            Ipv6Pool::Host => "ipv6",
        };
        Some(
            self.map(Kind::Ipv6, counter, address.to_string(), value, |n| {
                addresses::ipv6(pool, n).to_string()
            }),
        )
    }
}

/// Replaces the matches of `pattern` in `text` for which `replace` returns a
/// replacement. Matches that continue into a letter, a digit or one of
/// `joiners` are parts of something longer and stay.
fn substitute(
    pattern: &Regex,
    text: &str,
    joiners: &str,
    mut replace: impl FnMut(&str) -> Option<String>,
) -> String {
    let free = |c: Option<char>| c.is_none_or(|c| !c.is_alphanumeric() && !joiners.contains(c));
    let mut result = String::with_capacity(text.len());
    let mut last = 0;
    for m in pattern.find_iter(text) {
        let before = text[..m.start()].chars().next_back();
        let after = text[m.end()..].chars().next();
        if !(free(before) && free(after)) {
            continue;
        }
        if let Some(replacement) = replace(m.as_str()) {
            result.push_str(&text[last..m.start()]);
            result.push_str(&replacement);
            last = m.end();
        }
    }
    result.push_str(&text[last..]);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(mapper: &Mapper) -> Vec<Kind> {
        mapper.replaced.iter().map(|(kind, _)| *kind).collect()
    }

    #[test]
    fn replaces_addresses_anywhere_and_consistently() {
        let mut mapper = Mapper::default();
        assert_eq!(
            mapper.text("http://192.168.178.1:49000/x and 84.1.2.3, 192.168.178.1"),
            "http://192.0.2.1:49000/x and 198.51.100.1, 192.0.2.1"
        );
        assert_eq!(
            mapper.text("38:10:D5:01:02:0A 38-10-d5-01-02-0b 38:10:d5:01:02:0a"),
            "AA:BB:CC:00:00:01 AA-BB-CC-00-00-02 AA:BB:CC:00:00:01"
        );
        assert_eq!(
            mapper.text("2a00:1:2:3::/64 2a00:1:2:3::4 fe80::3a10:d5ff:fe01:20a"),
            "2001:db8:0:1::/64 2001:db8::1 fe80::1"
        );
        assert_eq!(
            mapper.text("uuid:75802409-BCCB-40e7-8e6c-3810d501020a"),
            "uuid:00000000-0000-4000-8000-000000000001"
        );
        assert_eq!(
            mapper.text("Max.Muster@mail.example.de at abc123.myfritz.net"),
            "user-1@example.com at example-1.myfritz.net"
        );
        assert_eq!(
            kinds(&mapper),
            [
                Kind::Ipv4,
                Kind::Ipv4,
                Kind::Mac,
                Kind::Mac,
                Kind::Ipv6,
                Kind::Ipv6,
                Kind::Ipv6,
                Kind::Uuid,
                Kind::Email,
                Kind::MyFritz
            ]
        );
    }

    #[test]
    fn keeps_what_identifies_no_one() {
        let mut mapper = Mapper::default();
        let text = "0.0.0.0 255.255.255.0 127.0.0.1 :: fe80::1 ff02::1 12:30:00 \
                    00:00:00:00:00:00 user-1@example.com example-1.myfritz.net \
                    version 1.2.3.4.5 7.31 ::ffff:192.0.2.1 a:b";
        assert_eq!(mapper.text(text), text);
        assert_eq!(kinds(&mapper), []);
    }

    #[test]
    fn keeps_parts_of_longer_tokens() {
        let mut mapper = Mapper::default();
        let text = "x192.168.1.1 192.168.1.1.2 a38:10:d5:01:02:0a 1:2:3:4:5:6:7:8:9";
        assert_eq!(mapper.text(text), text);
    }

    #[test]
    fn replaces_arguments_by_name() {
        let mut mapper = Mapper::default();
        let log = rules::LOG;
        for (name, value, expected) in [
            ("NewHostName", "laptop", "device-1"),
            ("NewX_AVM-DE_FriendlyName", "laptop", "device-1"),
            ("NewSSID", "Home", "wlan-1"),
            ("NewSSID", "", ""),
            ("NewUserName", "***", "***"),
            ("NewName", "device-1", "name-1"),
            ("NewSerialNumber", "3810D501020A", "AABBCC000001"),
            ("NewSerialNumber", "AABBCC000001", "AABBCC000001"),
            ("NewSerialNumber", "35123", "serial-1"),
            ("NewDeviceLog", "laptop joined", log),
            ("NewDeviceLog", log, log),
            ("NewURL", "/tr064", "/tr064"),
            (
                "NewURL",
                "https://acs.isp.de/x",
                "https://url-1.example.net/",
            ),
            ("NewEmail", "a@isp.de", "user-1@example.com"),
            ("NewEmail", "nobody", "nobody"),
            ("NewEnable", "1", "1"),
            ("NewIPAddress", "10.0.0.2", "192.0.2.1"),
            ("NewX_AVM-DE_Provider", "Telco", "provider-1"),
            ("NewEndpointID", "acs-7", "id-1"),
        ] {
            assert_eq!(mapper.argument(name, value), expected, "{name}");
        }
        assert_eq!(
            kinds(&mapper),
            [
                Kind::Device,
                Kind::Wlan,
                Kind::Name,
                Kind::Mac,
                Kind::Serial,
                Kind::Log,
                Kind::Url,
                Kind::Email,
                Kind::Ipv4,
                Kind::Provider,
                Kind::Id
            ]
        );
    }

    #[test]
    fn keeps_protocol_words() {
        let mut mapper = Mapper::new(["<allowedValue>Standard</allowedValue>", "<name>Up</name>"]);
        assert_eq!(mapper.argument("NewName", "standard"), "standard");
        assert_eq!(mapper.argument("NewName", "Standards"), "name-1");
        assert_eq!(mapper.argument("NewName", "up"), "name-2");
    }

    #[test]
    fn replaces_xml_by_element_name() {
        let mut mapper = Mapper::default();
        let xml = concat!(
            "<List><Item><Name>Home</Name><Number>0301</Number><Index>0</Index></Item>",
            r#"<Item last_user="1"><Username a="b">admin</Username><Name>Home</Name></Item>"#,
            "<Name>x</Other></List>"
        );
        let expected = concat!(
            "<List><Item><Name>name-1</Name><Number>phone-1</Number><Index>0</Index></Item>",
            r#"<Item last_user="1"><Username a="b">user-1</Username><Name>name-1</Name></Item>"#,
            "<Name>x</Other></List>"
        );
        assert_eq!(mapper.argument("NewNumberList", xml), expected);
        assert_eq!(
            mapper.xml("<r><UDN>uuid:75802409-bccb-40e7-8e6c-3810d501020a</UDN></r>"),
            "<r><UDN>uuid:00000000-0000-4000-8000-000000000001</UDN></r>"
        );
        assert_eq!(
            mapper.xml(r#"<Item Name="Home" Type="eVoIP"/>"#),
            r#"<Item Name="name-1" Type="eVoIP"/>"#
        );
    }
}
