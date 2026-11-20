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

## MCP server

`fritz mcp` serves the state of the device to an MCP client on stdin and
stdout. It only reads. Store the password first with `fritz auth login`, then
add the server to the client, for example to Claude Code:

```sh
claude mcp add fritz -- fritz --host 192.168.178.1 --user agent mcp
```

| Tool                    | Returns                                                     |
| ----------------------- | ----------------------------------------------------------- |
| `fritz_overview`        | Model, firmware, uptime, update, connection, rights         |
| `fritz_internet`        | Internet connection, addresses, DNS, rates, DSL line        |
| `fritz_wlan`            | Wireless networks                                           |
| `fritz_hosts`           | Known hosts, filtered by `active` and `search`, paginated   |
| `fritz_mesh`            | Mesh nodes and links                                        |
| `fritz_smarthome`       | Smart Home devices (FRITZ!OS 8.20 and later)                |
| `fritz_ports`           | Port forwardings and whether UPnP is on                     |
| `fritz_telephony`       | Numbers, accounts, deflections, answering machines, DECT    |
| `fritz_capabilities`    | Features of the device, rights of the user, two-factor      |
| `fritz_settings`        | Settings like DNS, IPv6, MTU, DHCP and time, secrets hidden |
| `fritz_list_services`   | TR-064 services, or the actions of one with their class     |
| `fritz_describe_action` | The arguments of an action and their types                  |
| `fritz_call_action`     | Calls an action that only reads, with secrets hidden        |

Calls, contacts and the event log are personal, so `fritz mcp` only serves
them with `--personal` (or `FRITZ_MCP_PERSONAL=true`). Without it,
`fritz_call_action` refuses the actions behind them and hides the event log
that `DeviceInfo:GetInfo` returns as well. The option adds three tools:

| Tool              | Returns                                                     |
| ----------------- | ----------------------------------------------------------- |
| `fritz_calls`     | Latest calls, filtered by `kind`                            |
| `fritz_phonebook` | Contacts of all phonebooks, filtered by `search`, paginated |
| `fritz_log`       | Event log, filtered by `search`                             |

Errors from the device come back as JSON with a `status` (`denied`,
`confirmation_required`, `blocked` or `busy`) and, where it helps,
`retry_after` in seconds.

Agents that use the CLI instead can load the skill in
[skills/fritz/SKILL.md](skills/fritz/SKILL.md).

### Docker

The image runs `fritz`; without a command it starts the MCP server. A
container has no keychain, so the device and the password come from the
environment. Get the fingerprint once with `fritz tls fingerprint`.

```sh
mise run docker-build    # builds udondan/fritz
docker run --rm -i \
  -e FRITZ_HOST=192.168.178.1 -e FRITZ_USERNAME=agent \
  -e FRITZ_PASSWORD -e FRITZ_TLS_FINGERPRINT \
  udondan/fritz
```

For the Docker MCP Toolkit, `mise run docker` builds the image and adds
[docker-mcp.yml](docker-mcp.yml) to the profile `fritz`. Set the secrets
`fritz.host`, `fritz.username`, `fritz.password` and `fritz.fingerprint`
with `docker mcp secret set`. The container must be able to reach the box.

## Changes

Release builds only read. Builds with the feature `write` add `fritz change`,
which plans a change, shows the current and the new values, and applies it
with the token of the plan:

```sh
cargo build -p fritz --features write
fritz change identity        # add it to [write] allow in the configuration
FRITZ_ALLOW_WRITE=1 fritz change plan WLANConfiguration3 SetEnable NewEnable=0
FRITZ_ALLOW_WRITE=1 fritz change apply WLANConfiguration3 SetEnable NewEnable=0 --token <token>
```

With `FRITZ_ALLOW_WRITE=1`, `fritz mcp` of such a build also offers
`fritz_plan_change` and `fritz_apply_change`, which work the same way.

Changes are only made to devices on the allow list, and never to those on the
deny list. See [SECURITY.md](SECURITY.md#changes) for all checks.

## Security

`fritz` is read-only by default. See [SECURITY.md](SECURITY.md) for the security
model and how to report a vulnerability.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md).

## License

[Apache-2.0](LICENSE)
