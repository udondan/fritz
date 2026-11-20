# Contributing

## Development

```sh
make lint   # cargo fmt --check and clippy with warnings as errors
make test   # all workspace tests
```

All tests run against recorded fixtures and the simulator in `fritz-sim`. CI
never talks to a real device.

## Commits and pull requests

Pull request titles follow [Conventional Commits](https://www.conventionalcommits.org/).
Pull requests are squash-merged, and the title becomes the commit message.

## Fixtures from other models

Fixtures recorded from other FRITZ!Box models and FRITZ!OS versions are
welcome. `fritz capture` will record them read-only and anonymized; the steps
will be described here once the command exists.
