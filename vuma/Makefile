.PHONY: all build test doc clean check fmt clippy pi5

all: build test

build:
	cargo build --workspace

test:
	cargo test --workspace

doc:
	cargo doc --workspace --no-deps

clean:
	cargo clean

check:
	cargo check --workspace

fmt:
	cargo fmt --all

clippy:
	cargo clippy --workspace -- -D warnings

pi5:
	cargo build --target aarch64-unknown-linux-gnu --release

pi5-bare:
	cargo build --target aarch64-unknown-none --release

bench:
	cargo bench --workspace

verify-examples:
	@echo "Verifying example programs..."
	@for f in examples/*.vuma; do echo "  $$f"; done
