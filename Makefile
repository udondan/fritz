# The latest stable Rust of rustup, like CI, where there is rustup. A
# packaged toolchain, like Homebrew's, can be older and miss new lints.
CARGO ?= $(if $(shell command -v rustup),rustup run stable cargo,cargo)

.PHONY: build release run test lint coverage hooks

build:
	@$(CARGO) build --workspace

release:
	@$(CARGO) build --release -p fritz

run:
	@$(CARGO) run -p fritz

test:
	@$(CARGO) test --workspace --all-features
	@$(CARGO) test -p fritz --test cli -- --exact cannot_change_without_the_feature_write

lint:
	@$(CARGO) fmt --all -- --check
	@$(CARGO) clippy --workspace --all-targets -- -D warnings
	@$(CARGO) clippy --workspace --all-targets --all-features -- -D warnings

# Checks that the tests cover every line changed against origin/main, like
# the patch status of Codecov.
coverage:
	@.githooks/pre-push

# Links the hooks into the repository's hooks directory, which all worktrees
# share. The links point to the main checkout's copies of the scripts.
hooks:
	@hooks="$$(git rev-parse --path-format=absolute --git-common-dir)/hooks" && \
		mkdir -p "$$hooks" && \
		for hook in pre-commit pre-push; do \
			ln -sf "../../.githooks/$$hook" "$$hooks/$$hook" && \
			echo "installed $$hooks/$$hook"; \
		done
