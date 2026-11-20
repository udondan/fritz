//! The faults of a FRITZ!Box that are about the user, not the action: a
//! missing right, and the second factor some actions need.
//!
//! The device asks for a second factor (UPnP error 866) before it runs some
//! actions that change settings. It is confirmed on the device, by a button
//! or a phone code, never by this crate. While a confirmation runs, others
//! are busy (868), and after failed ones the device blocks them for up to an
//! hour (867).

use std::fmt;
use std::time::Duration;

use serde::Serialize;

/// A fault of the device about the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Fault {
    /// The user lacks the right for the action (606).
    Denied,
    /// The action needs a second factor (866).
    ConfirmationRequired,
    /// Confirmations are blocked after failed ones (867).
    Blocked,
    /// Another confirmation is running (868).
    Busy,
}

impl Fault {
    /// The fault a UPnP error code stands for, if any.
    pub fn from_code(code: u16) -> Option<Self> {
        match code {
            606 => Some(Self::Denied),
            866 => Some(Self::ConfirmationRequired),
            867 => Some(Self::Blocked),
            868 => Some(Self::Busy),
            _ => None,
        }
    }

    /// The fault of an error, if the device answered with one.
    pub fn of(error: &tr064::Error) -> Option<Self> {
        error.upnp_code().and_then(Self::from_code)
    }

    /// The UPnP error code.
    pub fn code(self) -> u16 {
        match self {
            Self::Denied => 606,
            Self::ConfirmationRequired => 866,
            Self::Blocked => 867,
            Self::Busy => 868,
        }
    }

    /// A name for machines, like `confirmation_required`.
    pub fn status(self) -> &'static str {
        match self {
            Self::Denied => "denied",
            Self::ConfirmationRequired => "confirmation_required",
            Self::Blocked => "blocked",
            Self::Busy => "busy",
        }
    }

    /// The longest time to wait before trying again, if waiting helps.
    pub fn retry_after(self) -> Option<Duration> {
        match self {
            Self::Blocked => Some(Duration::from_hours(1)),
            Self::Busy => Some(Duration::from_mins(2)),
            Self::Denied | Self::ConfirmationRequired => None,
        }
    }

    /// Whether the device holds back all confirmations for a while, so that
    /// the next calls fail the same way.
    pub fn is_temporary(self) -> bool {
        self.retry_after().is_some()
    }
}

impl fmt::Display for Fault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = match self {
            Self::Denied => "the user lacks the right for this action",
            Self::ConfirmationRequired => {
                "the device wants this action confirmed with a second factor, \
                 which fritz never does"
            }
            Self::Blocked => {
                "the device blocks confirmations after failed ones; \
                 try again in up to an hour"
            }
            Self::Busy => {
                "the device is busy with another confirmation; \
                 try again in a few minutes"
            }
        };
        write!(f, "{text} (UPnP error {})", self.code())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [Fault; 4] = [
        Fault::Denied,
        Fault::ConfirmationRequired,
        Fault::Blocked,
        Fault::Busy,
    ];

    #[test]
    fn maps_the_codes() {
        for fault in ALL {
            assert_eq!(Fault::from_code(fault.code()), Some(fault));
        }
        assert_eq!(Fault::from_code(713), None);
        assert_eq!(Fault::from_code(401), None);
    }

    #[test]
    fn finds_the_fault_of_an_error() {
        let error = tr064::Error::Upnp {
            code: 866,
            description: "second factor authentication required".to_owned(),
        };
        assert_eq!(Fault::of(&error), Some(Fault::ConfirmationRequired));
        assert_eq!(Fault::of(&tr064::Error::AuthenticationFailed), None);
    }

    #[test]
    fn describes_the_faults() {
        let names: Vec<_> = ALL.iter().map(|f| f.status()).collect();
        assert_eq!(
            names,
            ["denied", "confirmation_required", "blocked", "busy"]
        );
        for fault in ALL {
            assert_eq!(
                serde_json::to_value(fault).unwrap(),
                serde_json::json!(fault.status())
            );
            let text = fault.to_string();
            assert!(text.ends_with(&format!("(UPnP error {})", fault.code())));
        }
        assert!(Fault::ConfirmationRequired.to_string().contains("never"));
    }

    #[test]
    fn says_when_to_try_again() {
        let waits: Vec<_> = ALL.iter().map(|f| f.retry_after()).collect();
        assert_eq!(
            waits,
            [
                None,
                None,
                Some(Duration::from_hours(1)),
                Some(Duration::from_mins(2))
            ]
        );
        assert!(Fault::Blocked.is_temporary());
        assert!(!Fault::Denied.is_temporary());
    }
}
