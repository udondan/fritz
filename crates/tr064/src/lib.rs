//! Vendor-neutral client for the TR-064 device management protocol.
//!
//! TR-064 (Broadband Forum) describes a device and its services in UPnP
//! description and SCPD documents, and controls it with SOAP calls. This crate
//! reads those documents and makes the calls. It knows nothing about specific
//! vendors: vendor extensions are services and actions like any other.

mod description;
mod error;
mod scpd;
mod xml;

pub use description::{Device, DeviceDescription, Service};
pub use error::Error;
pub use scpd::{Action, AllowedRange, Argument, Direction, Scpd, StateVariable};
