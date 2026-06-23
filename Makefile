# ============================================================================
# VUMA — Verified-Unsafe Memory Access: AI-Native Programming Language
# ============================================================================
# Top-level Makefile for building, testing, benchmarking, documenting, and
# cross-compiling the VUMA framework.
# ============================================================================

# ---------------------------------------------------------------------------
# Toolchain
# ---------------------------------------------------------------------------
CARGO         := cargo
RUSTUP        := rustup

# ---------------------------------------------------------------------------
# Install prefix (override with make install PREFIX=/usr/local)
# ---------------------------------------------------------------------------
PREFIX        ?= /usr/local

# ---------------------------------------------------------------------------
# Feature flags (add or override on the command line)
# ---------------------------------------------------------------------------
FEATURES      ?=

# ============================================================================
# Phony targets
# ============================================================================
.PHONY: all build check test bench doc fmt clippy \
        x86-64-run riscv64-run \
        clean install verify-examples \
        setup toolchain

# ============================================================================
# Default: build + test
# ============================================================================
all: build test

# ============================================================================
# Core build targets
# ============================================================================

## build: Compile the entire workspace (debug mode)
build:
        $(CARGO) build --workspace

## check: Type-check the workspace without producing artifacts
check:
        $(CARGO) check --workspace

## check-fast: Type-check only the core crates (skip slow ones)
check-fast:
        $(CARGO) check -p vuma -p vuma-scg -p vuma-ive -p vuma-bd

# ============================================================================
# Testing
# ============================================================================

## test: Run all workspace tests
test:
        $(CARGO) test --workspace

## test-verbose: Run all workspace tests with full output
test-verbose:
        $(CARGO) test --workspace -- --nocapture

## test-single CRATE=<crate>: Run tests for a single crate
test-single:
        $(CARGO) test -p $(CRATE)

## test-doc: Run doc tests across the workspace
test-doc:
        $(CARGO) test --workspace --doc

# ============================================================================
# Benchmarking
# ============================================================================

## bench: Run all benchmarks
bench:
        $(CARGO) bench --workspace

## bench-single CRATE=<crate>: Run benchmarks for a single crate
bench-single:
        $(CARGO) bench -p $(CRATE)

# ============================================================================
# Documentation
# ============================================================================

## doc: Build workspace documentation (no dependencies)
doc:
        $(CARGO) doc --workspace --no-deps

## doc-open: Build documentation and open in browser
doc-open:
        $(CARGO) doc --workspace --no-deps --open

## doc-private: Build documentation including private items
doc-private:
        $(CARGO) doc --workspace --no-deps --document-private-items

# ============================================================================
# Code quality
# ============================================================================

## fmt: Auto-format all Rust source files
fmt:
        $(CARGO) fmt --all

## fmt-check: Check formatting without making changes (CI-friendly)
fmt-check:
        $(CARGO) fmt --all -- --check

## clippy: Run Clippy lints with deny-warnings
clippy:
        $(CARGO) clippy --workspace -- -D warnings

## clippy-fix: Auto-fix Clippy warnings where possible
clippy-fix:
        $(CARGO) clippy --workspace --fix --allow-dirty

## lint: Run all code-quality checks (fmt + clippy)
lint: fmt-check clippy

# ============================================================================
# Clean
# ============================================================================

## clean: Remove all build artifacts
clean:
        $(CARGO) clean

## clean-doc: Remove generated documentation
clean-doc:
        rm -rf target/doc

# ============================================================================
# Install
# ============================================================================

## install: Build in release mode and install to PREFIX
install: build-release
        $(CARGO) install --path . --root $(PREFIX) --locked

## build-release: Compile the workspace in release mode
build-release:
        $(CARGO) build --workspace --release

# ============================================================================
# Setup / toolchain
# ============================================================================

## setup: Install required toolchain, components, and targets
setup: toolchain
        $(RUSTUP) component add rustfmt clippy
        $(RUSTUP) target add aarch64-unknown-linux-gnu
        $(RUSTUP) target add aarch64-unknown-none

## toolchain: Install the pinned nightly toolchain
toolchain:
        $(RUSTUP) toolchain install nightly

# ============================================================================
# Miscellaneous
# ============================================================================

# ============================================================================
# x86_64 and RISC-V QEMU targets
# ============================================================================

## x86-64-run: Run x86_64 target in QEMU
x86-64-run:
        qemu-system-x86_64 -drive format=raw,file=target/x86_64-unknown-none/release/vuma-x86_64.bin -serial stdio

## riscv64-run: Run RISC-V 64 target in QEMU (virt machine)
riscv64-run:
        qemu-system-riscv64 -machine virt -nographic -bios default -kernel target/riscv64gc-unknown-none-elf/release/vuma-riscv64

## verify-examples: List all example programs
verify-examples:
        @echo "Verifying example programs..."
        @for f in examples/*.vuma; do echo "  $$f"; done

## help: Show this help message
help:
        @echo "VUMA Build System"
        @echo "================="
        @echo ""
        @echo "Usage: make <target>"
        @echo ""
        @echo "Targets:"
        @grep -E '^## ' $(MAKEFILE_LIST) | sort | \
                awk 'BEGIN {FS = ": "}; {printf "  %-18s %s\n", $$1, $$2}' | \
                sed 's/^## //'
