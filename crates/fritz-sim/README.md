# fritz-sim

A fake FRITZ!Box for tests. `fritz-sim` serves recorded and anonymized
fixtures of real devices over TR-064 on the loopback interface, so a client
can be tested end to end without a router and without ever sending a change
to a real one.

Unofficial, not affiliated with FRITZ! GmbH.

- Serves the device description, the service descriptions (SCPD) and the
  action calls with HTTP Digest authentication, over HTTP or HTTPS with a
  self-signed certificate
- Checks every call against the service description: unknown actions fail
  with UPnP error 401, missing or unknown arguments with 402, values that
  don't fit the data type with 600
- Keeps state: a call like `SetEnable` changes the values that the matching
  getters return afterwards. Settings a real device changes along with them
  change only when a test injects that side effect
- Looks up one host by MAC or IP address in the recorded host entries
- Serves the list documents (hosts, mesh, calls, phonebook), the session
  login of the web interface (`login_sid.lua`, PBKDF2), the Smart Home REST
  API, the statistics of Smart Home devices (`getbasicdevicestats`) and pages
  of `data.lua`, like the system load of `ecoStat`
- Injects UPnP errors (like 866, 867 or 868 for the second factor), delays,
  discarded writes, side effects of writes and a locked login, and records
  every request

## Fixtures

The fixtures come with the crate:

| Fixture                         | Device                    | FRITZ!OS |
| ------------------------------- | ------------------------- | -------- |
| `fritz-repeater-2400/8.25`      | FRITZ!Repeater 2400       | 8.25     |
| `fritzbox-7690/8.25`            | FRITZ!Box 7690            | 8.25     |
| `fritzwlan-repeater-1750e/7.32` | FRITZ!WLAN Repeater 1750E | 7.32     |

The repeaters have only a password and take any user name; start the
simulator with `--username dslf-config` to log in without `--user`.

`fritz-sim --list` prints them. Every fixture is a directory with the files
of one device; see [`Fixture`](https://docs.rs/fritz-sim/latest/fritz_sim/struct.Fixture.html)
for the layout.

More devices make every project that tests against `fritz-sim` better. If
your FRITZ!Box model or FRITZ!OS version is missing, please contribute a
fixture. Recording it only reads from
the device, and the anonymizer replaces addresses, names and other personal
values before anything leaves your machine. The steps are in
[CONTRIBUTING.md](https://github.com/udondan/fritz/blob/main/CONTRIBUTING.md#fixtures-from-other-models).

## In Rust tests

```toml
[dev-dependencies]
fritz-sim = "0.1"
```

```rust,no_run
use fritz_sim::{Fixture, Injection, Sim};

#[tokio::test]
async fn turns_the_wlan_off() -> Result<(), Box<dyn std::error::Error>> {
    let sim = Sim::start(Fixture::bundled("fritzbox-7690/8.25")?).await?;
    // sim.url() is the base URL, like http://127.0.0.1:49000, and
    // fritz_sim::USERNAME and fritz_sim::PASSWORD are the credentials.
    let client = sim.client().build()?;
    let description = client.description().await?;
    let wlan = description.service("WLANConfiguration1")?;
    client.call(wlan, "SetEnable", &[("NewEnable", "0")]).await?;
    assert_eq!(sim.state("WLANConfiguration1", "Enable").as_deref(), Some("0"));

    sim.inject(Injection::fault("WLANConfiguration1", "SetEnable", 866));
    assert!(client.call(wlan, "SetEnable", &[("NewEnable", "1")]).await.is_err());
    Ok(())
}
```

`Fixture::bundled_names()` lists all fixtures, to run a test against each of
them.

## In other languages

The binary serves a fixture until it is interrupted, so tests in any language
can start it and point their client at it:

```sh
cargo install fritz-sim
fritz-sim --fixture fritzbox-7690/8.25 --port 49000
```

It prints `listening on http://127.0.0.1:49000` once it accepts calls. With
`--https` it serves a new self-signed certificate and prints its SHA-256
fingerprint on the next line. The credentials are `fritz-sim` and
`fritz-sim-password`, or those of `--username` and `--password`. `--fixture`
also takes the directory of a fixture that does not come with the crate.

## License

Apache-2.0
