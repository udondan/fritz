//! FRITZ!Box specific logic on top of the vendor-neutral `tr064` crate:
//! session login, access policy, redaction and fixture anonymization.
//!
//! Unofficial, not affiliated with FRITZ! GmbH.

pub mod anonymize;
pub mod capabilities;
pub mod capture;
#[cfg(feature = "write")]
pub mod change;
pub mod config;
pub mod connection;
pub mod fault;
pub mod generic;
pub mod identity;
pub mod lists;
mod names;
pub mod personal;
pub mod policy;
pub mod ports;
pub mod read;
pub mod redact;
pub mod secrets;
pub mod session;
pub mod settings;
pub mod smarthome;
pub mod status;
pub mod telephony;

// Compiles the example of the README.
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
struct ReadmeDoctests;
