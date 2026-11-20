# Notes for coding agents

## Layout

Cargo workspace:

- `crates/tr064`: vendor-neutral TR-064 client. Must not contain anything
  FRITZ!-specific (no AVM/FRITZ! fault codes, services or URLs).
- `crates/fritz-core`: FRITZ!-specific logic on top of `tr064`.
- `crates/fritz-sim`: fake TR-064 server for tests, not published.
- `crates/fritz`: the binary (CLI and `fritz mcp`).

## Rules

- **Never send a write action to a real device.** Against a real device, only
  read actions (`Get*` without side effects) and discovery are allowed. Every
  write path is tested against `fritz-sim` only.
- Tests must not reach the network except loopback.
- Never log or print secrets (passwords, keys, session IDs).
- `make lint test` must pass.
- Library crates (`tr064`, `fritz-core`) use regular version requirements; the
  `fritz` binary and dev-dependencies use exact versions (`=x.y.z`).
- Conventional commits: `feat:`/`fix:` only for changes to shipped code,
  `chore:` for tooling and CI, `build(deps):` for runtime dependency updates.
