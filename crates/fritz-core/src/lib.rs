//! FRITZ!Box specific logic on top of the vendor-neutral `tr064` crate:
//! session login, access policy, redaction and fixture anonymization.
//!
//! Unofficial, not affiliated with FRITZ! GmbH.

pub mod anonymize;
pub mod capture;
pub mod config;
pub mod connection;
pub mod fault;
pub mod generic;
pub mod lists;
mod names;
pub mod policy;
pub mod read;
pub mod redact;
pub mod secrets;
pub mod session;
pub mod smarthome;
pub mod status;
