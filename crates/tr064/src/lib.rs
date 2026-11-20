//! Vendor-neutral client for the TR-064 device management protocol.
//!
//! TR-064 (Broadband Forum, "LAN-Side DSL CPE Configuration") describes
//! devices and their services with UPnP description and SCPD documents and
//! calls their actions over SOAP with HTTP digest authentication.
//!
//! This crate knows nothing about specific vendors. Vendor extensions are
//! plain services and actions like any other.
