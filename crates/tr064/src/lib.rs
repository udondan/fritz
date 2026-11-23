//! Vendor-neutral client for the TR-064 device management protocol.
//!
//! TR-064 (Broadband Forum) describes a device and its services in UPnP
//! description and SCPD documents, and controls it with SOAP calls. This crate
//! reads those documents and makes the calls. It knows nothing about specific
//! vendors: vendor extensions are services and actions like any other.
//!
//! # Example
//!
//! ```no_run
//! use tr064::{Client, SecretString, Url};
//!
//! # async fn run() -> Result<(), Box<dyn std::error::Error>> {
//! let client = Client::builder(Url::parse("http://192.168.178.1:49000")?)
//!     .credentials("user", SecretString::from("password"))
//!     .build()?;
//!
//! // The device description lists the services; SCPDs list their actions.
//! let description = client.description().await?;
//! let device_info = description.service("DeviceInfo1")?;
//! let scpd = client.scpd(device_info).await?;
//! for action in &scpd.actions {
//!     println!("{}", action.name);
//! }
//!
//! let out = client.call(device_info, "GetInfo", &[]).await?;
//! println!("{:?}", out.get("NewSoftwareVersion"));
//! # Ok(())
//! # }
//! ```
//!
//! # TLS
//!
//! Devices usually present a self-signed certificate, so the client does not
//! check it against certificate authorities. Instead it accepts exactly one
//! certificate, given by its SHA-256 [`Fingerprint`]. Fetch it once with
//! [`fetch_fingerprint`], have someone confirm it, store it, and
//! [pin](ClientBuilder::pin) it from then on:
//!
//! ```no_run
//! use std::time::Duration;
//! use tr064::{Client, Url};
//!
//! # async fn run() -> Result<(), Box<dyn std::error::Error>> {
//! let url = Url::parse("https://192.168.178.1:49443")?;
//! let fingerprint = tr064::fetch_fingerprint(&url, Duration::from_secs(5)).await?;
//! println!("the device presents {fingerprint}");
//! let client = Client::builder(url).pin(fingerprint).build()?;
//! # Ok(())
//! # }
//! ```
//!
//! Plain `http` works too, but sends everything except the password readable
//! on the network.
//!
//! # Errors
//!
//! The device reports failed actions as UPnP errors with a numeric code. The
//! standard codes are below 800, vendors define their own above:
//!
//! ```no_run
//! # async fn run(client: tr064::Client, service: tr064::Service) {
//! match client.call(&service, "GetGenericHostEntry", &[("NewIndex", "99")]).await {
//!     Ok(out) => println!("{out:?}"),
//!     Err(error) if error.upnp_code() == Some(713) => println!("no such entry"),
//!     Err(error) => eprintln!("{error}"),
//! }
//! # }
//! ```
//!
//! # Logging
//!
//! The client logs through [`tracing`](https://docs.rs/tracing): one event
//! per request at `info`, the steps of the Digest authentication and the
//! check of the certificate at `debug`, and the arguments of action calls at
//! `trace`. Passwords, headers, nonces and Digest answers are never logged,
//! and of other documents only the size. Values of arguments and query
//! parameters pass a [`Redact`] hook, which hides all of them unless the
//! client is given [another one](ClientBuilder::redact).
//!
//! The client answers HTTP Digest challenges and reuses the nonce while the
//! device accepts it. It never retries with credentials the device rejected
//! ([`Error::AuthenticationFailed`]), so that a wrong password does not lock
//! the account.

#![warn(missing_docs)]

mod client;
mod description;
mod digest;
mod error;
mod logging;
mod scpd;
mod soap;
mod tls;
mod xml;

pub use client::{Client, ClientBuilder};
pub use description::{Device, DeviceDescription, Service};
pub use error::Error;
pub use logging::{HIDDEN, HideValues, Redact};
pub use reqwest::Url;
pub use scpd::{Action, AllowedRange, Argument, Direction, Scpd, StateVariable};
pub use secrecy::SecretString;
pub use soap::Arguments;
pub use tls::{Fingerprint, fetch_fingerprint};
