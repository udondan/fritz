# Security

## Reporting a vulnerability

Please report vulnerabilities privately through
[GitHub security advisories](https://github.com/udondan/fritz/security/advisories/new),
not in a public issue.

## Security model

`fritz` talks to a device that controls a home network, so it is careful by
default:

- **Read-only by default.** Release builds can't send any action that changes
  the device. Write support will be a separate compile-time feature, and will
  also need a runtime opt-in plus an allowlist of target devices.
- **Every action has a class**, taken from its name and arguments in the
  service description before anything is sent: `read`, `personal` (call
  lists, phone books, voice messages, the event log), `secret` (getters that
  could return passwords or keys), `write`, or `critical` (changes that can
  lock you out, cost money, expose or reset the device, and any action of an
  unknown kind). `fritz tr64 call` only runs `read` and `personal` actions.
  The class of every action of the recorded fixtures is kept in
  [`policy.txt`](crates/fritz-core/tests/data/policy.txt), so that every
  change of the policy shows up in review.
- **No secrets in output.** Passwords, keys, session IDs and similar values are
  redacted from every output, error and log line.
- **Credentials** come from the environment or the OS keychain, never from
  config files or command line arguments.
- **TLS** to the device is pinned to the certificate fingerprint you confirmed
  on first use. There is no option to turn verification off.
- **Two-factor confirmation** that the device asks for is reported back, never
  bypassed.
- **Data from the device** (host names, network names, phone book entries) is
  treated as untrusted input when it is handed to an AI agent.
