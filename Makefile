.PHONY: build release run test lint hooks

build:
	@cargo build --workspace

release:
	@cargo build --release -p fritz

run:
	@cargo run -p fritz

test:
	@cargo test --workspace

lint:
	@cargo fmt --all -- --check
	@cargo clippy --workspace --all-targets -- -D warnings

# Links the pre-commit hook into the repository's hooks directory, which all
# worktrees share. The link points to the main checkout's copy of the script.
hooks:
	@hooks="$$(git rev-parse --path-format=absolute --git-common-dir)/hooks" && \
		mkdir -p "$$hooks" && \
		ln -sf ../../.githooks/pre-commit "$$hooks/pre-commit" && \
		echo "installed $$hooks/pre-commit"
