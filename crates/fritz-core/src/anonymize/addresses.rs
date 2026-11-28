//! Which addresses identify a household, and the placeholders for them.
//!
//! Placeholders come from ranges that never identify anyone: the
//! documentation networks of RFC 5737 and RFC 3849, the benchmarking network
//! of RFC 2544 when those run out, and a locally administered MAC prefix.

use std::net::{Ipv4Addr, Ipv6Addr};

/// The first bytes of every placeholder MAC address.
const MAC_PREFIX: [u8; 4] = [0xAA, 0xBB, 0xCC, 0x00];

/// The 12 hex digits of a MAC address in lower case, from forms like
/// `38:10:D5:01:02:03`, `38-10-d5-01-02-03` or `3810D5010203`.
pub(super) fn mac_digits(value: &str) -> Option<String> {
    let digits: String = value
        .chars()
        .filter(|c| !matches!(c, ':' | '-'))
        .map(|c| c.to_ascii_lowercase())
        .collect();
    (digits.len() == 12 && digits.bytes().all(|b| b.is_ascii_hexdigit())).then_some(digits)
}

/// Whether a MAC address, as [`mac_digits`], stays as it is: a placeholder,
/// or the all-zero and broadcast addresses.
pub(super) fn keeps_mac(digits: &str) -> bool {
    digits.starts_with("aabbcc00") || digits == "000000000000" || digits == "ffffffffffff"
}

/// The `n`th placeholder MAC address, with `separator` between the bytes.
pub(super) fn mac(n: u32, separator: &str) -> String {
    let [.., high, low] = n.to_be_bytes();
    MAC_PREFIX
        .iter()
        .chain(&[high, low])
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join(separator)
}

/// Whether an IPv4 address stays as it is because it identifies no one:
/// special addresses, network masks, and placeholders.
pub(super) fn keeps_ipv4(address: Ipv4Addr) -> bool {
    [
        address.is_unspecified(),
        address.is_loopback(),
        address.is_broadcast(),
        address.is_multicast(),
        address.is_documentation(),
        is_mask(address),
        is_benchmarking(address),
    ]
    .contains(&true)
}

/// Whether an address is a network mask of at least 8 bits.
fn is_mask(address: Ipv4Addr) -> bool {
    let bits = u32::from(address);
    bits.leading_ones() >= 8 && bits.leading_ones() + bits.trailing_zeros() == 32
}

/// Whether an address is in 198.18.0.0/15, where placeholders go once the
/// documentation networks are used up.
fn is_benchmarking(address: Ipv4Addr) -> bool {
    let [a, b, ..] = address.octets();
    a == 198 && (b & 0xFE) == 18
}

/// Whether an IPv4 address is only used inside a home or provider network.
pub(super) fn is_internal(address: Ipv4Addr) -> bool {
    let [a, b, ..] = address.octets();
    address.is_private() || address.is_link_local() || (a == 100 && (b & 0xC0) == 64)
}

/// The `n`th placeholder IPv4 address, starting at 1: 192.0.2.n for internal
/// addresses and 198.51.100.n for others, then 198.18.x.y or 198.19.x.y.
pub(super) fn ipv4(internal: bool, n: u32) -> Ipv4Addr {
    if let Ok(host @ 1..=254) = u8::try_from(n) {
        return if internal {
            Ipv4Addr::new(192, 0, 2, host)
        } else {
            Ipv4Addr::new(198, 51, 100, host)
        };
    }
    let [_, _, x, y] = (n - 255).to_be_bytes();
    Ipv4Addr::new(198, if internal { 18 } else { 19 }, x, y)
}

/// How an IPv6 address is replaced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Ipv6Pool {
    /// A link-local address, which contains the MAC address in many cases.
    LinkLocal,
    /// A prefix like `2a00:1:2:3::`, without an interface part.
    Prefix,
    /// Any other address.
    Host,
}

/// The pool of an IPv6 address, or `None` when it stays as it is: special
/// addresses, short link-local addresses like `fe80::1`, and placeholders.
pub(super) fn ipv6_pool(address: Ipv6Addr) -> Option<Ipv6Pool> {
    let segments = address.segments();
    let bits = u128::from(address);
    if address.is_unspecified()
        || address.is_loopback()
        || address.is_multicast()
        || segments[..2] == [0x2001, 0x0db8]
    {
        return None;
    }
    if segments[0] & 0xFFC0 == 0xFE80 {
        let short = segments[0] == 0xFE80 && segments[1..6].iter().all(|&s| s == 0);
        return (!short).then_some(Ipv6Pool::LinkLocal);
    }
    if bits & u128::from(u64::MAX) == 0 {
        return Some(Ipv6Pool::Prefix);
    }
    Some(Ipv6Pool::Host)
}

/// The `n`th placeholder IPv6 address of a pool: `fe80::n`, `2001:db8:0:n::`
/// or `2001:db8::n`.
pub(super) fn ipv6(pool: Ipv6Pool, n: u32) -> Ipv6Addr {
    let [b0, b1, b2, b3] = n.to_be_bytes();
    let (high, low) = (u16::from_be_bytes([b0, b1]), u16::from_be_bytes([b2, b3]));
    match pool {
        Ipv6Pool::LinkLocal => Ipv6Addr::new(0xFE80, 0, 0, 0, 0, 0, high, low),
        Ipv6Pool::Prefix => Ipv6Addr::new(0x2001, 0xDB8, high, low, 0, 0, 0, 0),
        Ipv6Pool::Host => Ipv6Addr::new(0x2001, 0xDB8, 0, 0, 0, 0, high, low),
    }
}

/// Whether a UUID stays as it is: the nil UUID or a placeholder.
pub(super) fn keeps_uuid(value: &str) -> bool {
    value.starts_with("00000000-0000-")
}

/// The `n`th placeholder UUID.
pub(super) fn uuid(n: u32) -> String {
    format!("00000000-0000-4000-8000-{n:012x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v4(text: &str) -> Ipv4Addr {
        text.parse().unwrap()
    }

    fn v6(text: &str) -> Ipv6Addr {
        text.parse().unwrap()
    }

    #[test]
    fn handles_mac_addresses() {
        assert_eq!(
            mac_digits("38:10:D5:01:02:0A").as_deref(),
            Some("3810d501020a")
        );
        assert_eq!(mac_digits("3810D501020A").as_deref(), Some("3810d501020a"));
        assert_eq!(mac_digits("38:10:D5"), None);
        assert_eq!(mac_digits("38:10:D5:01:02:0G"), None);
        assert!(keeps_mac("aabbcc000001"));
        assert!(keeps_mac("000000000000"));
        assert!(keeps_mac("ffffffffffff"));
        assert!(!keeps_mac("3810d501020a"));
        assert_eq!(mac(258, ":"), "AA:BB:CC:00:01:02");
        assert_eq!(mac(1, ""), "AABBCC000001");
    }

    #[test]
    fn keeps_ipv4_addresses_that_identify_no_one() {
        for kept in [
            "0.0.0.0",
            "127.0.0.1",
            "255.255.255.255",
            "239.255.255.250",
            "255.255.255.0",
            "255.255.0.0",
            "192.0.2.7",
            "198.51.100.7",
            "203.0.113.7",
            "198.19.1.1",
        ] {
            assert!(keeps_ipv4(v4(kept)), "{kept}");
        }
        for replaced in ["192.168.178.1", "84.1.2.3", "128.0.0.0", "198.20.0.1"] {
            assert!(!keeps_ipv4(v4(replaced)), "{replaced}");
        }
    }

    #[test]
    fn tells_internal_ipv4_addresses() {
        assert!(is_internal(v4("192.168.178.1")));
        assert!(is_internal(v4("10.1.2.3")));
        assert!(is_internal(v4("169.254.1.1")));
        assert!(is_internal(v4("100.64.0.1")));
        assert!(!is_internal(v4("100.128.0.1")));
        assert!(!is_internal(v4("84.1.2.3")));
    }

    #[test]
    fn numbers_ipv4_placeholders() {
        assert_eq!(ipv4(true, 1), v4("192.0.2.1"));
        assert_eq!(ipv4(false, 254), v4("198.51.100.254"));
        assert_eq!(ipv4(true, 255), v4("198.18.0.0"));
        assert_eq!(ipv4(false, 256 + 255), v4("198.19.1.0"));
        assert!(keeps_ipv4(ipv4(true, 1000)));
    }

    #[test]
    fn sorts_ipv6_addresses_into_pools() {
        for kept in [
            "::",
            "::1",
            "ff02::1",
            "2001:db8::5",
            "fe80::1",
            "fe80::1:2",
        ] {
            assert_eq!(ipv6_pool(v6(kept)), None, "{kept}");
        }
        assert_eq!(
            ipv6_pool(v6("fe80::3a10:d5ff:fe01:203")),
            Some(Ipv6Pool::LinkLocal)
        );
        assert_eq!(ipv6_pool(v6("febf::1")), Some(Ipv6Pool::LinkLocal));
        assert_eq!(ipv6_pool(v6("2a00:1:2:3::")), Some(Ipv6Pool::Prefix));
        assert_eq!(ipv6_pool(v6("2a00:1:2:3::4")), Some(Ipv6Pool::Host));
    }

    #[test]
    fn numbers_ipv6_placeholders() {
        assert_eq!(ipv6(Ipv6Pool::LinkLocal, 1), v6("fe80::1"));
        assert_eq!(ipv6(Ipv6Pool::Prefix, 0x1_0002), v6("2001:db8:1:2::"));
        assert_eq!(ipv6(Ipv6Pool::Host, 3), v6("2001:db8::3"));
        for pool in [Ipv6Pool::LinkLocal, Ipv6Pool::Prefix, Ipv6Pool::Host] {
            assert_eq!(ipv6_pool(ipv6(pool, 70_000)), None);
        }
    }

    #[test]
    fn handles_uuids() {
        assert_eq!(uuid(26), "00000000-0000-4000-8000-00000000001a");
        assert!(keeps_uuid(&uuid(26)));
        assert!(keeps_uuid("00000000-0000-0000-0000-000000000000"));
        assert!(!keeps_uuid("75802409-bccb-40e7-8e6c-3810d501020a"));
    }
}
