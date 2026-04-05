.PHONY: fmt lint test run

fmt:
	@cargo fmt --all

lint:
	@cargo clippy --workspace --all-targets --all-features -- -D warnings

test:
	@cargo test --workspace

run:
	@cargo run -p codex-bridge-cli -- run
