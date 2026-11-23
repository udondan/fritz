//! FRITZ!Box specific logic on top of the vendor-neutral `tr064` crate:
//! session login, access policy, redaction and fixture anonymization.
//!
//! Unofficial, not affiliated with FRITZ! GmbH.

pub mod anonymize;
pub mod cache;
pub mod callmonitor;
pub mod capabilities;
pub mod capture;
#[cfg(feature = "write")]
pub mod change;
pub mod config;
pub mod connection;
pub mod diagnose;
pub mod dirs;
pub mod discover;
pub mod doctor;
pub mod dsl;
pub mod fault;
pub mod firmware;
pub mod generic;
pub mod host;
pub mod identity;
pub mod interfaces;
pub mod lists;
mod names;
pub mod personal;
pub mod policy;
pub mod ports;
pub mod powerline;
pub mod read;
pub mod redact;
pub mod secrets;
pub mod session;
pub mod settings;
pub mod smarthome;
pub mod status;
pub mod system;
pub mod telephony;
pub mod traffic;
pub mod users;
pub mod wlan;

// Compiles the example of the README.
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
struct ReadmeDoctests;
