# ============================================================================
# VUMA — Verified-Unsafe Memory Access: AI-Native Programming Language
# ============================================================================
# Top-level Makefile for building, testing, benchmarking, documenting, and
# cross-compiling the VUMA framework including the Pi 5 bare-metal target.
# ============================================================================

# ---------------------------------------------------------------------------
# Toolchain
# ---------------------------------------------------------------------------
CARGO         := cargo
RUSTUP        := rustup

# ---------------------------------------------------------------------------
# Raspberry Pi 5 bare-metal toolchain
# ---------------------------------------------------------------------------
PI5_TARGET    := aarch64-unknown-none
PI5_CROSS     := aarch64-none-elf-
PI5_OBJCOPY   := $(PI5_CROSS)objcopy
PI5_DIR       := src/pi5
PI5_OUT       := target/$(PI5_TARGET)/release
PI5_ELF       := $(PI5_OUT)/vuma-pi5
PI5_IMG       := $(PI5_DIR)/kernel8.img
PI5_MAP       := $(PI5_DIR)/kernel8.map

# SD card boot partition (override with make pi5-flash SD=/dev/sdX1)
SD            ?= /mnt/sd-boot

# QEMU binary for aarch64 bare-metal
QEMU          := qemu-system-aarch64

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
        pi5 pi5-image pi5-flash pi5-debug pi5-bare \
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
# Pi 5: bare-metal aarch64 targets
# ============================================================================

## pi5: Cross-compile vuma-pi5 for bare-metal aarch64 (release)
pi5:
	$(CARGO) build --target $(PI5_TARGET) --release -p vuma-pi5

## pi5-image: Build kernel8.img from the ELF binary
pi5-image: pi5
	$(PI5_OBJCOPY) -O binary $(PI5_ELF) $(PI5_IMG)
	@echo "Built $(PI5_IMG) ($$(stat -c %s $(PI5_IMG) 2>/dev/null || echo ?) bytes)"

## pi5-flash: Flash kernel8.img to SD card boot partition
pi5-flash: pi5-image
	cp $(PI5_IMG) $(SD)/kernel8.img
	@echo "Flashed kernel8.img to $(SD)"

## pi5-debug: Launch QEMU debug session (GDB stub on :1234)
pi5-debug: pi5-image
	$(QEMU) -M raspi3b -serial stdio -kernel $(PI5_IMG) -s -S

## pi5-bare: Legacy hosted cross-compile (Linux user-space)
pi5-bare:
	$(CARGO) build --target aarch64-unknown-none --release -p vuma-pi5

## pi5-run: Run the Pi 5 image in QEMU (no debug stub)
pi5-run: pi5-image
	$(QEMU) -M raspi3b -serial stdio -kernel $(PI5_IMG)

# ============================================================================
# Clean
# ============================================================================

## clean: Remove all build artifacts
clean:
	$(CARGO) clean

## clean-pi5: Remove only Pi 5 (aarch64-unknown-none) build artifacts
clean-pi5:
	$(CARGO) clean --target aarch64-unknown-none

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
