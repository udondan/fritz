//! Rating argument and action names by how sensitive their values are.

/// Words in action or argument names that point to secrets.
const SECRET_WORDS: &[&str] = &[
    "credential",
    "credentials",
    "key",
    "keys",
    "passphrase",
    "passwd",
    "password",
    "pin",
    "psk",
    "secret",
    "sid",
    "token",
];

/// Words that turn a name into a limit, counter or flag about a secret rather
/// than the secret itself, like `NewMinCharsPSK` or `NewPINFailureCount`.
const QUALIFIER_WORDS: &[&str] = &[
    "allowed",
    "chars",
    "count",
    "enabled",
    "index",
    "is",
    "max",
    "min",
    "required",
    "selectable",
];

/// Word sequences of values that are not secrets but identify the owner or
/// the connection, like the login name at the internet provider. Getters
/// returning them are called, but the values are masked.
const MASKED_PHRASES: &[&[&str]] = &[
    &["username"],
    &["user", "name"],
    &["login"],
    &["parameter", "key"],
];

/// How sensitive the value of a name is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Sensitivity {
    /// Recorded as it is.
    Plain,
    /// Recorded as `***`.
    Masked,
    /// Never read.
    Secret,
}

/// How sensitive the value of a name is. Limits and flags like
/// `NewMaxCharsPassword` are plain. Names with one of the [`SECRET_WORDS`]
/// outside of [`MASKED_PHRASES`] are secret, other names with such a phrase
/// are masked.
pub(super) fn sensitivity(name: &str) -> Sensitivity {
    let words = words(name);
    if words.iter().any(|w| QUALIFIER_WORDS.contains(&w.as_str())) {
        return Sensitivity::Plain;
    }
    let mut in_phrase = vec![false; words.len()];
    for phrase in MASKED_PHRASES {
        for (start, window) in words.windows(phrase.len()).enumerate() {
            if window.iter().zip(*phrase).all(|(w, p)| w == p) {
                in_phrase[start..start + phrase.len()].fill(true);
            }
        }
    }
    let secret = words
        .iter()
        .zip(&in_phrase)
        .any(|(w, &phrase)| !phrase && SECRET_WORDS.contains(&w.as_str()));
    if secret {
        Sensitivity::Secret
    } else if in_phrase.contains(&true) {
        Sensitivity::Masked
    } else {
        Sensitivity::Plain
    }
}

/// The lower-case words of an identifier like `NewX_AVM-DE_WEPKey0`, without
/// trailing digits: `new`, `x`, `avm`, `de`, `wep`, `key`.
fn words(name: &str) -> Vec<String> {
    let chars: Vec<char> = name.chars().collect();
    let mut words = Vec::new();
    let mut current = String::new();
    for (i, &c) in chars.iter().enumerate() {
        let previous = i.checked_sub(1).map(|j| chars[j]);
        let next = chars.get(i + 1).copied();
        if !c.is_ascii_alphanumeric() || starts_word(previous, c, next) {
            finish_word(&mut words, &mut current);
        }
        if c.is_ascii_alphanumeric() {
            current.push(c.to_ascii_lowercase());
        }
    }
    finish_word(&mut words, &mut current);
    words
}

/// Whether `c` starts a new word in camel case: `Key` in `WEPKey` or
/// `PreSharedKey`, `Index` in `Entry0Index`.
fn starts_word(previous: Option<char>, c: char, next: Option<char>) -> bool {
    let Some(previous) = previous else {
        return false;
    };
    c.is_ascii_uppercase()
        && (previous.is_ascii_lowercase()
            || previous.is_ascii_digit()
            || (previous.is_ascii_uppercase() && next.is_some_and(|n| n.is_ascii_lowercase())))
}

fn finish_word(words: &mut Vec<String>, current: &mut String) {
    let word = current.trim_end_matches(|c: char| c.is_ascii_digit());
    if !word.is_empty() {
        words.push(word.to_owned());
    }
    current.clear();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rates_the_sensitivity_of_names() {
        for (name, expected) in [
            ("NewSSID", Sensitivity::Plain),
            ("NewX_AVM-DE_PINEnabled", Sensitivity::Plain),
            ("NewX_AVM-DE_IsDefaultPasswordActive", Sensitivity::Plain),
            ("NewPasswordUserSelectable", Sensitivity::Plain),
            ("NewUserName", Sensitivity::Masked),
            ("NewX_AVM-DE_CurrentUsername", Sensitivity::Masked),
            ("NewParameterKey", Sensitivity::Masked),
            ("NewKeyPassphrase", Sensitivity::Secret),
            ("NewLoginPassword", Sensitivity::Secret),
            ("NewUserNameKey", Sensitivity::Secret),
        ] {
            assert_eq!(sensitivity(name), expected, "{name}");
        }
    }

    #[test]
    fn splits_identifiers_into_words() {
        assert_eq!(
            words("NewX_AVM-DE_WEPKey0"),
            ["new", "x", "avm", "de", "wep", "key"]
        );
        assert_eq!(words("NewPreSharedKey"), ["new", "pre", "shared", "key"]);
        assert_eq!(words("Entry0Index"), ["entry", "index"]);
        assert_eq!(words("NewPortMappingNumberOfEntries")[2], "mapping");
        assert_eq!(words("__"), Vec::<String>::new());
    }
}
