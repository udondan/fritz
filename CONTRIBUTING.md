# Contributing

## Development

```sh
make lint   # cargo fmt --check and clippy with warnings as errors
make test   # all workspace tests
make hooks  # install a pre-commit hook that runs the qlty checks of CI
```

The hook needs [qlty](https://docs.qlty.sh/cli/quickstart) and `jq`. Like CI,
it blocks a commit on lint findings and on code smells that are new compared to
`origin/main`.

All tests run against recorded fixtures and the simulator in `fritz-sim`. CI
never talks to a real device.

## Commits and pull requests

Pull request titles follow [Conventional Commits](https://www.conventionalcommits.org/).
Pull requests are squash-merged, and the title becomes the commit message.

## Fixtures from other models

Fixtures recorded from other FRITZ!Box models and FRITZ!OS versions are
welcome. They live in `crates/fritz-sim/fixtures/` and are published with
`fritz-sim`, so every project that tests against it gains the device.
Recording one only reads from the device, and nothing personal ends up in the
repository:

1. Set up a user on the device as described in [docs/box-setup.md](docs/box-setup.md).
2. Look at what a capture would read. This loads only the service
   descriptions, which need no login:

   ```sh
   fritz capture --plan
   ```

3. Record the raw capture. It contains personal data and is written outside
   the repository, to `~/.local/share/fritz/captures/<time>/`:

   ```sh
   FRITZ_PASSWORD=… fritz capture --user <user>
   ```

4. List what is personal to you and must not appear anywhere, one text per
   line, in `.private-patterns` at the root of the repository: your name,
   street, real WLAN names and so on. The file is ignored by git, and lines
   starting with `#` are comments.
5. Anonymize the capture into the fixtures, named by model and FRITZ!OS
   version:

   ```sh
   fritz fixture anonymize ~/.local/share/fritz/captures/<time> \
     --out crates/fritz-sim/fixtures/<model>/<fritzos>
   ```

   Addresses, UUIDs, serial numbers, host and WLAN names, user names, phone
   numbers and similar values become placeholders like `AA:BB:CC:00:00:01`,
   `192.0.2.1` or `device-1`; the same value always gets the same
   placeholder. If any replaced value or private pattern is still found
   afterwards, nothing is written and the affected files are listed.
6. Check all fixtures and read through the new one before opening a pull
   request:

   ```sh
   fritz fixture check crates/fritz-sim/fixtures
   ```

   It names the files and kinds of the personal values it finds, never the
   values. Delete the raw capture when you no longer need it.
