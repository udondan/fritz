//! The client of the examples, configured from the environment:
//!
//! - `TR064_URL`: the device, `http://192.168.178.1:49000` by default
//! - `TR064_FINGERPRINT`: the certificate fingerprint for an `https` URL
//! - `TR064_USERNAME`, `TR064_PASSWORD`: the credentials, if needed

use std::env;

use tr064::{Client, SecretString, Url};

pub fn client() -> Result<Client, Box<dyn std::error::Error>> {
    let url = env::var("TR064_URL").unwrap_or_else(|_| "http://192.168.178.1:49000".to_owned());
    let mut builder = Client::builder(Url::parse(&url)?);
    if let Ok(fingerprint) = env::var("TR064_FINGERPRINT") {
        builder = builder.pin(fingerprint.parse()?);
    }
    if let (Ok(username), Ok(password)) = (env::var("TR064_USERNAME"), env::var("TR064_PASSWORD")) {
        builder = builder.credentials(username, SecretString::from(password));
    }
    Ok(builder.build()?)
}
