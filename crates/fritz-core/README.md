# fritz-core

FRITZ!Box specific logic on top of the vendor-neutral
[`tr064`](https://crates.io/crates/tr064) client. It is the library behind the
[`fritz`](https://crates.io/crates/fritz) CLI and MCP server, and can be used
on its own to read a FRITZ!Box from Rust.

Unofficial, not affiliated with FRITZ! GmbH.

- Reads the state of a device: an overview, hosts, mesh, wireless networks,
  the internet connection, port forwardings, telephony and settings. Parts
  the device lacks, or that the user may not read, are left out instead of
  failing the whole read
- Classifies every action by the names in its service description (read,
  denied getter, write, critical) before anything is sent, and only calls
  plain getters when reading
- Redacts passwords, keys and other secrets in every value the device
  returns
- Calls, phonebooks and the event log are personal and live in their own
  module
- Logs in to the web interface (`login_sid.lua`, PBKDF2 or MD5) for the Smart
  Home REST API of FRITZ!OS 8.20 and later, without ever retrying a rejected
  login
- Maps the faults of the second factor (UPnP errors 866, 867 and 868) and
  missing rights
- Profiles, passwords from the environment or the keychain of the operating
  system, and certificate pinning
- Records fixtures from a device (reads only) and anonymizes them, for
  [`fritz-sim`](https://crates.io/crates/fritz-sim)

Changes to devices need the feature `write` (`change::Changer`). It is off by
default, so builds without it can only read.

## Example

```rust,no_run
use fritz_core::read::Reader;
use fritz_core::status;
use tr064::{Client, SecretString, Url};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = Client::builder(Url::parse("http://192.168.178.1:49000")?)
        .credentials("agent", SecretString::from("password"))
        .build()?;
    let mut reader = Reader::new(client).await?;
    let overview = status::overview(&mut reader).await?;
    println!("{}", serde_json::to_string_pretty(&overview)?);
    Ok(())
}
```

Use a user of its own for this, with only the rights it needs; see
[the setup of the device](https://github.com/udondan/fritz/blob/main/docs/box-setup.md).

## License

Apache-2.0
