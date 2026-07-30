#!/usr/bin/env bash
# rust-env.sh — source this to put the pinned Rust nightly toolchain on PATH.
# Installed by wave 0, task 0-b-install.
#
# The VUMA repo pins `nightly-2026-03-01` via `rust-toolchain.toml`. Rustup
# honours that file automatically whenever `cargo`/`rustc` is invoked from
# inside the repo, so sourcing this script is only required for shells that
# were started before the installer added `$HOME/.cargo/bin` to PATH (the
# rustup installer appends the standard `. "$HOME/.cargo/env"` block to
# ~/.bashrc / ~/.profile, but those only take effect in fresh login shells).
#
# Installed components: rustfmt, clippy, rust-src
# Installed targets:    aarch64-unknown-linux-gnu, aarch64-unknown-none
#                        (plus the default host x86_64-unknown-linux-gnu)
#
# Usage:
#   source scripts/env/rust-env.sh
#   rustc --version        # -> rustc 1.96.0-nightly (38c0de8dc 2026-02-28)
#   cargo --version        # -> cargo 1.96.0-nightly (f298b8c82 2026-02-24)
#   rustup toolchain list  # -> nightly-2026-03-01-x86_64-unknown-linux-gnu (active, default)

# Idempotent: only prepend if not already present.
case ":${PATH}:" in
    *":${HOME}/.cargo/bin:"*)
        # Already on PATH; nothing to do.
        ;;
    *)
        export PATH="${HOME}/.cargo/bin:${PATH}"
        ;;
esac

# Tell rustup where its data lives (it would auto-detect this anyway, but
# being explicit helps when this script is sourced from non-login shells such
# as CI runners or `bash -c`).
export RUSTUP_HOME="${RUSTUP_HOME:-${HOME}/.rustup}"
export CARGO_HOME="${CARGO_HOME:-${HOME}/.cargo}"
