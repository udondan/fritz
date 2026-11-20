//! Vendor-neutral client for the TR-064 device management protocol.
//!
//! TR-064 (Broadband Forum) describes a device and its services in UPnP
//! description and SCPD documents, and controls it with SOAP calls. This crate
//! reads those documents and makes the calls. It knows nothing about specific
//! vendors: vendor extensions are services and actions like any other.

mod client;
mod description;
mod digest;
mod error;
mod scpd;
mod soap;
mod tls;
mod xml;

pub use client::{Client, ClientBuilder};
pub use description::{Device, DeviceDescription, Service};
pub use error::Error;
pub use reqwest::Url;
pub use scpd::{Action, AllowedRange, Argument, Direction, Scpd, StateVariable};
pub use secrecy::SecretString;
pub use soap::Arguments;
pub use tls::{Fingerprint, fetch_fingerprint};
