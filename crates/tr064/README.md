# tr064

Vendor-neutral async Rust client for TR-064, the UPnP/SOAP protocol many
routers, modems and gateways use for device management.

- Reads the device description and the service descriptions (SCPD), so every
  service and action of a device is available at runtime, vendor extensions
  included
- Calls actions over SOAP and reports UPnP errors with their codes
- HTTP Digest authentication (MD5 and SHA-256). Reuses the nonce and never
  retries rejected credentials, so a wrong password does not lock the account
- TLS with a pinned certificate fingerprint for devices with self-signed
  certificates, and a helper to fetch the fingerprint on first use
- Logs requests through `tracing`, with argument values and query
  parameters hidden unless a `Redact` hook shows them
- rustls only, no OpenSSL

## Example

```rust,no_run
use tr064::{Client, SecretString, Url};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = Client::builder(Url::parse("http://192.168.178.1:49000")?)
        .credentials("user", SecretString::from("password"))
        .build()?;

    let description = client.description().await?;
    let device_info = description.service("DeviceInfo1")?;
    let out = client.call(device_info, "GetInfo", &[]).await?;
    for (name, value) in out.iter() {
        println!("{name} = {value}");
    }
    Ok(())
}
```

For `https`, pin the device's certificate:

```rust,ignore
let url = Url::parse("https://192.168.178.1:49443")?;
let fingerprint = tr064::fetch_fingerprint(&url, Duration::from_secs(5)).await?;
// Confirm the fingerprint once, store it, and pin it from then on.
let client = Client::builder(url).pin(fingerprint).build()?;
```

The [examples](https://github.com/udondan/fritz/tree/main/crates/tr064/examples)
list the services of a device and call any action:

```sh
TR064_URL=http://192.168.178.1:49000 cargo run -p tr064 --example services
TR064_USERNAME=user TR064_PASSWORD=secret cargo run -p tr064 --example call -- DeviceInfo1 GetInfo
```

## Scope

The crate knows the protocol, not any particular device. Vendor-specific
meaning, such as what a vendor's error codes stand for, belongs to the
application. Values are passed and returned as strings. The SCPD names
each argument's UPnP data type.

## License

Apache-2.0
