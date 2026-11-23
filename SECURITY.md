# Security

## Reporting a vulnerability

Please report vulnerabilities privately through
[GitHub security advisories](https://github.com/udondan/fritz/security/advisories/new),
not in a public issue.

## Security model

`fritz` talks to a device that controls a home network, so it is careful by
default:

- **Read-only by default.** Release builds can't send any action that changes
  the device. Changes need a build with the feature `write`, and then pass
  every check below before anything is sent.
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

### Changes

In builds with the feature `write`, `fritz change` plans and applies changes,
and so do the MCP tools `fritz_plan_change` and `fritz_apply_change`. `fritz
mcp` only offers these tools with `FRITZ_ALLOW_WRITE=1`. A change is only sent
if all of these hold:

1. The action is `write` and `FRITZ_ALLOW_WRITE=1` is set, or it is `critical`
   and `FRITZ_ALLOW_CRITICAL=1` is set as well. Getters are never changes.
2. The arguments are exactly the inputs of the action, each once.
3. The identity of the device, a SHA-256 hash of its unique device name and
   serial number, is in `allow` of the section `[write]` of the configuration,
   and not in `deny`. `deny` always wins, so a device can be locked against
   changes for good. `fritz change identity` prints the identity.
4. The change was planned less than five minutes before, for the same device,
   action and arguments, and its token was not used yet. A plan reads the
   current values and shows what would change, without changing anything.

After a change, the values are read back and compared to the ones that were
sent.

A plan shows only the inputs of the action. A device may change other
settings along with them, like the options of a feature it turns off, and
the service descriptions don't say which. So a change reads every getter of
the service twice before and once after it, and reports the other values
that changed as side effects. Values that already differ between the two
reads before, like counters, are left out; slower ones may still show up.
Changes in other services are not detected. The simulator that the tests of
changes run against changes nothing along with a write unless a test
injects it, so these tests cannot show what a real device does. Every plan, change and refusal is appended to `audit.jsonl` in the state
directory, with secrets hidden; a change that cannot be logged is not sent.
The token file holds only hashes of the tokens and of the changes, and is
locked while it changes, so processes that share it cannot use a token twice.
