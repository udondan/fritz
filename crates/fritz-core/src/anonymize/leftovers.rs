//! Searching texts for values that must not be in a fixture.

use super::Kind;
use super::addresses;

/// Replaced values shorter than this are not searched for: they cannot
/// identify anyone, and would be found in every document, like an SSID `5`.
pub(super) const MIN_LENGTH: usize = 3;

/// A text to search for, in lower case.
#[derive(Debug)]
pub(super) struct Needle {
    kind: Kind,
    text: String,
    /// Whether the text only counts as a whole word.
    whole: bool,
}

/// The needles for replaced values and private patterns. A MAC address is
/// also searched for without separators and with dashes, and the text of a
/// private pattern anywhere, even inside words and however short.
pub(super) fn needles(replaced: &[(Kind, String)], private: &[String]) -> Vec<Needle> {
    let mut needles = Vec::new();
    for (kind, original) in replaced {
        let original = original.to_lowercase();
        if *kind == Kind::Mac
            && let Some(digits) = addresses::mac_digits(&original)
        {
            let bytes: Vec<&str> = (0..12).step_by(2).map(|i| &digits[i..i + 2]).collect();
            for text in [bytes.join(":"), bytes.join("-"), digits.clone()] {
                needles.push(Needle {
                    kind: *kind,
                    text,
                    whole: true,
                });
            }
        } else if original.chars().count() >= MIN_LENGTH {
            needles.push(Needle {
                kind: *kind,
                text: original,
                whole: true,
            });
        }
    }
    needles.extend(private.iter().map(|pattern| Needle {
        kind: Kind::Private,
        text: pattern.to_lowercase(),
        whole: false,
    }));
    needles
}

/// The kinds of the needles found in a text, each once.
pub(super) fn find(text: &str, needles: &[Needle]) -> Vec<Kind> {
    let text = text.to_lowercase();
    let mut kinds: Vec<Kind> = needles
        .iter()
        .filter(|needle| {
            text.match_indices(&needle.text)
                .any(|(start, found)| !needle.whole || whole_word(&text, start, found))
        })
        .map(|needle| needle.kind)
        .collect();
    kinds.sort_unstable();
    kinds.dedup();
    kinds
}

/// Whether `text`, in lower case, contains `word` as a whole word.
pub(super) fn contains_word(text: &str, word: &str) -> bool {
    let word = word.to_lowercase();
    text.match_indices(&word)
        .any(|(start, found)| whole_word(text, start, found))
}

/// Whether a match does not continue into the letters or digits around it,
/// as far as it starts and ends with one: `1.2.3.4` is in `1.2.3.4:80` but
/// not in `1.2.3.45`.
fn whole_word(text: &str, start: usize, found: &str) -> bool {
    let alphanumeric = |c: Option<char>| c.is_some_and(char::is_alphanumeric);
    let first = found.chars().next();
    let last = found.chars().next_back();
    let before = text[..start].chars().next_back();
    let after = text[start + found.len()..].chars().next();
    !(alphanumeric(first) && alphanumeric(before) || alphanumeric(last) && alphanumeric(after))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(text: &str, replaced: &[(Kind, &str)], private: &[&str]) -> Vec<Kind> {
        let replaced: Vec<(Kind, String)> = replaced
            .iter()
            .map(|(k, v)| (*k, (*v).to_owned()))
            .collect();
        let private: Vec<String> = private.iter().map(|p| (*p).to_owned()).collect();
        find(text, &needles(&replaced, &private))
    }

    #[test]
    fn finds_replaced_values_as_whole_words() {
        let replaced = [(Kind::Ipv4, "192.168.178.2"), (Kind::Device, "Laptop")];
        assert_eq!(kinds("at 192.168.178.2:80", &replaced, &[]), [Kind::Ipv4]);
        assert_eq!(kinds("at 192.168.178.20", &replaced, &[]), []);
        assert_eq!(
            kinds("LAPTOP 192.168.178.2", &replaced, &[]),
            [Kind::Ipv4, Kind::Device]
        );
        assert_eq!(kinds("laptops", &replaced, &[]), []);
    }

    #[test]
    fn finds_mac_addresses_in_any_spelling() {
        let replaced = [(Kind::Mac, "38:10:D5:01:02:0A")];
        for text in ["38:10:d5:01:02:0a", "38-10-D5-01-02-0A", "x 3810D501020A"] {
            assert_eq!(kinds(text, &replaced, &[]), [Kind::Mac], "{text}");
        }
        assert_eq!(kinds("3810D501020AB", &replaced, &[]), []);
    }

    #[test]
    fn finds_private_patterns_anywhere() {
        assert_eq!(kinds("MyStreet12", &[], &["street"]), [Kind::Private]);
        assert_eq!(kinds("road", &[], &["street"]), []);
    }

    #[test]
    fn ignores_short_originals() {
        let replaced = [(Kind::Log, ""), (Kind::Wlan, "5"), (Kind::Name, "TV")];
        assert_eq!(kinds("", &replaced, &[]), []);
        assert_eq!(kinds("5 TV", &replaced, &["v"]), [Kind::Private]);
    }

    #[test]
    fn matches_needles_that_start_with_symbols() {
        let replaced = [(Kind::Phone, "+49 30 1"), (Kind::Name, "(home)")];
        assert_eq!(
            kinds("a+49 30 1 b(home)c", &replaced, &[]),
            [Kind::Name, Kind::Phone]
        );
        assert_eq!(kinds("+49 30 12", &replaced, &[]), []);
    }
}
