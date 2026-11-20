# fritz

A command line tool and MCP server that lets you and your AI agents read the
status and settings of a FRITZ!Box router.

> **Unofficial.** This project is not affiliated with, endorsed by or supported
> by FRITZ! GmbH (formerly AVM). FRITZ! and FRITZ!Box are trademarks of
> FRITZ! GmbH.

> **Work in progress.** Nothing is released yet. The first goal is read-only
> access over TR-064. Changing settings will come later, behind several
> explicit opt-ins.

## Crates

| Crate                              | Purpose                                                                |
| ---------------------------------- | ---------------------------------------------------------------------- |
| [`tr064`](crates/tr064)            | Vendor-neutral TR-064 (UPnP/SOAP) client library                       |
| [`fritz-core`](crates/fritz-core)  | FRITZ!-specific logic: login, Smart Home API, policy, redaction        |
| [`fritz-sim`](crates/fritz-sim)    | Stateful fake TR-064 server for tests, built from recorded fixtures    |
| [`fritz`](crates/fritz)            | The `fritz` binary: CLI and `fritz mcp`                                |

## Security

`fritz` is read-only by default. See [SECURITY.md](SECURITY.md) for the security
model and how to report a vulnerability.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md).

## License

[Apache-2.0](LICENSE)
