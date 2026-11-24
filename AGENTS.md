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
- Library crates (`tr064`, `fritz-core`, `fritz-sim`) use regular version
  requirements; the `fritz` binary and dev-dependencies use exact versions
  (`=x.y.z`).
- Conventional commits: `feat:`/`fix:` only for changes to shipped code,
  `chore:` for tooling and CI, `build(deps):` for runtime dependency updates.
  A change that belongs to one crate names it as scope, like
  `feat(fritz-sim): add a fixture`: it then appears in the release notes of
  that crate only, even when it touches others. Without a scope, a change is
  listed under every crate whose files outside `tests/` it changes.

## Commands

- `make lint`: `cargo fmt --check` and clippy with `-D warnings`, once
  without features and once with `--all-features`. The workspace enables
  `clippy::pedantic` and forbids `unsafe_code`.
- `make test`: `cargo test --workspace --all-features`, plus
  `cannot_change_without_the_feature_write` run without features, which shows
  that default builds cannot change anything.
- One test: `cargo test -p <crate> --test <file> -- <name>`. Add
  `--all-features` for code behind the feature `write`.
- `make hooks`: installs a pre-commit hook with the qlty checks of CI (needs
  `qlty` and `jq`) and a pre-push hook that runs `make coverage`: Codecov
  needs every changed line covered, and `scripts/patch-coverage.py` checks
  that locally. `make coverage` measures in a Linux container (needs Docker),
  because CI measures on Linux, which counts lines that macOS does not, like
  error paths. `FRITZ_COVERAGE=native` uses the local `cargo-llvm-cov`
  instead. Run it before every push. When a commit changes `.github/` or
  `mise.toml`, the pre-commit hook also runs actionlint (with shellcheck) and
  zizmor in the versions of `mise.toml` (needs `mise`), like CI.
- Where rustup is installed, `make` and the hooks put its stable toolchain
  first in PATH, so clippy and rustfmt are the versions of CI, not older ones
  of Homebrew. Without that, new lints only fail in CI.
- The MCP tool list is checked against
  `crates/fritz/tests/data/mcp-tools.json`. After changing tools, review the
  difference and rewrite it with
  `FRITZ_UPDATE_GOLDEN=1 cargo test -p fritz --test mcp`.
- Docker: `mise run docker-build` builds the image, `mise run docker` also
  adds it to the Docker MCP Toolkit.

## Architecture

- `tr064` speaks SOAP/UPnP: device and service descriptions (SCPD), Digest
  authentication, pinned TLS certificates. `fritz-core` adds everything
  FRITZ!-specific: reads, policy, redaction, configuration, secrets, the
  web interface session for the Smart Home REST API, capture and
  anonymization. `fritz` is a thin CLI and MCP server on top;
  `crates/fritz/src/context.rs` resolves the profile, flags, environment and
  password source of every command.
- `fritz-core/src/policy.rs` classifies every action by the names in its SCPD
  alone (read, denied getter, write, critical), before anything is sent. Read
  paths only run plain getters. Calls of arbitrary actions (`fritz tr064 call`,
  `fritz_call_action`) go through `generic::call`.
- Values from the device pass `redact` before they are shown or stored. Calls,
  phonebooks and the event log are personal and only served with `--personal`
  (`FRITZ_MCP_PERSONAL=true`). Strings from the device, like host names and
  SSIDs, are untrusted; the MCP instructions tell the model to treat them as
  data.
- Changes need the cargo feature `write` (`fritz/write` enables
  `fritz-core/write` and `fritz_core::change`), `FRITZ_ALLOW_WRITE=1` at run
  time, and a device on the allow list and not on the deny list. A plan shows
  the current and new values and returns a token, which apply requires; every
  change is audited (`change/audit.rs`). SECURITY.md lists all checks.
- `fritz-sim` serves the fixtures in
  `crates/fritz-sim/fixtures/<model>/<fritzos>/` on loopback; they are
  published with the crate (`Fixture::bundled`). It checks calls against the SCPDs, keeps the state that writes
  change, serves list documents, the Smart Home REST API and pages of
  `data.lua`, can inject
  faults, delays and a locked login (`Sim::inject`), and records requests.
  The CLI and MCP tests remove inherited `FRITZ_*` variables.
- Fixtures come from `fritz capture` (reads only, writes outside the
  repository), `fritz fixture anonymize` and `fritz fixture check`; see
  CONTRIBUTING.md. Raw captures are personal and never go into the
  repository.
