.PHONY: build release run test lint

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
