.PHONY: build release run test lint coverage hooks

build:
	@cargo build --workspace

release:
	@cargo build --release -p fritz

run:
	@cargo run -p fritz

test:
	@cargo test --workspace --all-features
	@cargo test -p fritz --test cli -- --exact cannot_change_without_the_feature_write

lint:
	@cargo fmt --all -- --check
	@cargo clippy --workspace --all-targets -- -D warnings
	@cargo clippy --workspace --all-targets --all-features -- -D warnings

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
