# ============================================================================
# VUMA — justfile: convenient developer commands
# ============================================================================
# Usage:
#   just              — list all recipes
#   just build        — compile the workspace
#   just test         — run all tests
#   just lint         — fmt check + clippy
# ============================================================================

# -- Variables ---------------------------------------------------------------

prefix := "/usr/local"
crate  := ""

# -- Default: list available recipes -----------------------------------------

default:
    @just --list

# ============================================================================
# Build
# ============================================================================

# Build the entire workspace (debug)
build:
    cargo build --workspace

# Build in release mode
release:
    cargo build --workspace --release

# Type-check without producing artifacts
check:
    cargo check --workspace

# Quick check: core crates only
check-fast:
    cargo check -p vuma -p vuma-scg -p vuma-ive -p vuma-bd

# ============================================================================
# Test
# ============================================================================

# Run all workspace tests
test:
    cargo test --workspace

# Run tests with full output
test-verbose:
    cargo test --workspace -- --nocapture

# Run tests for a single crate: just test-crate crate=vuma-bd
test-crate crate=crate:
    cargo test -p {{crate }}

# Run doc tests
test-doc:
    cargo test --workspace --doc

# Run tests matching a pattern: just test-filter filter=uart
test-filter filter:
    cargo test --workspace {{filter }}

# ============================================================================
# Benchmark
# ============================================================================

# Run all benchmarks
bench:
    cargo bench --workspace

# Run benchmarks for a single crate: just bench-crate crate=vuma-bd
bench-crate crate=crate:
    cargo bench -p {{crate }}

# ============================================================================
# Documentation
# ============================================================================

# Build documentation
doc:
    cargo doc --workspace --no-deps

# Build docs and open in browser
doc-open:
    cargo doc --workspace --no-deps --open

# Build docs including private items
doc-private:
    cargo doc --workspace --no-deps --document-private-items

# ============================================================================
# Code Quality
# ============================================================================

# Auto-format all Rust source
fmt:
    cargo fmt --all

# Check formatting without changes (CI-friendly)
fmt-check:
    cargo fmt --all -- --check

# Run Clippy with deny-warnings
clippy:
    cargo clippy --workspace -- -D warnings

# Auto-fix Clippy warnings
clippy-fix:
    cargo clippy --workspace --fix --allow-dirty

# Run all lints: fmt check + clippy
lint: fmt-check clippy

# ============================================================================
# x86_64 and RISC-V QEMU targets
# ============================================================================

# Run x86_64 target in QEMU
x86-64-run:
    qemu-system-x86_64 -drive format=raw,file=target/x86_64-unknown-none/release/vuma-x86_64.bin -serial stdio

# Run RISC-V 64 target in QEMU (virt machine)
riscv64-run:
    qemu-system-riscv64 -machine virt -nographic -bios default -kernel target/riscv64gc-unknown-none-elf/release/vuma-riscv64

# ============================================================================
# Cross-Compilation (aarch64 Linux)
# ============================================================================

# Cross-compile for aarch64 Linux (user-space)
cross-aarch64:
    cargo build --target aarch64-unknown-linux-gnu --workspace

# Cross-compile for aarch64 Linux (release)
cross-aarch64-release:
    cargo build --target aarch64-unknown-linux-gnu --workspace --release

# ============================================================================
# Setup & Toolchain
# ============================================================================

# Install the pinned nightly toolchain
toolchain:
    rustup toolchain install nightly-2026-03-01

# Install required components and targets
setup: toolchain
    rustup component add rustfmt clippy
    rustup target add aarch64-unknown-linux-gnu
    rustup target add aarch64-unknown-none

# Update Rust toolchain to latest nightly
update-toolchain:
    rustup update nightly

# Show current toolchain info
toolchain-info:
    rustup show
    @echo "---"
    rustup target list --installed

# ============================================================================
# Clean
# ============================================================================

# Remove all build artifacts
clean:
    cargo clean

# Remove generated documentation
clean-doc:
    rm -rf target/doc

# ============================================================================
# Install
# ============================================================================

# Build release and install to prefix: just install prefix=/usr/local
install prefix=prefix: release
    cargo install --path . --root {{prefix }} --locked

# ============================================================================
# Miscellaneous
# ============================================================================

# Verify example programs exist
verify-examples:
    @echo "Verifying example programs..."
    @for f in examples/*.vuma; do echo "  $$f"; done

# Show workspace members
members:
    cargo metadata --format-version 1 --no-deps | jq -r '.workspace_members[]'

# Show dependency tree
tree:
    cargo tree --workspace

# Watch for changes and auto-test
watch:
    cargo watch -x "test --workspace"

# Watch for changes and auto-check
watch-check:
    cargo watch -x "check --workspace"
