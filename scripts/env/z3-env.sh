#!/usr/bin/env bash
# z3-env.sh — source this to expose the Z3 5.0.0 library to pkg-config / cargo.
# Installed by wave 0, task 0-a-install (upgraded to latest stable by follow-up
# task F0-a-install).
#
# The prior caveats-remediation run (tag v0.2.0-alpha.1-caveats-remediation)
# shipped Z3 4.13.3 here as a dev shim that symlinked $HOME/.local/lib/libz3.so
# to the Debian system libz3.so.4 (4.13.3-1). Follow-up task F0-a-install
# upgraded the shim to the latest stable release, Z3 5.0.0, by installing the
# official x64-glibc-2.39 prebuilt release asset (libz3.so, headers, z3 CLI)
# directly under $HOME/.local — no source rebuild was needed because (a) the
# sandbox has only 2 cores and no cmake, making a source build exceed the task
# time budget, and (b) a pre-built release asset is itself a release asset.
# Symbol audit: all 791 Z3_* FFI symbols expected by z3-sys 0.11.0 are present
# in the Z3 5.0.0 libz3.so, so cargo build -p vuma-ive --release links cleanly.
#
# Usage:
#   source scripts/env/z3-env.sh
#   pkg-config --modversion z3        # -> 5.0.0
#   cargo build ...                   # z3-sys will pick up Z3 via pkg-config

export Z3_PREFIX="${Z3_PREFIX:-$HOME/.local}"
export PKG_CONFIG_PATH="$Z3_PREFIX/lib/pkgconfig${PKG_CONFIG_PATH:+:$PKG_CONFIG_PATH}"
export LIBRARY_PATH="$Z3_PREFIX/lib${LIBRARY_PATH:+:$LIBRARY_PATH}"   # link-time search for gcc
export LD_LIBRARY_PATH="$Z3_PREFIX/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"  # runtime search (follows symlink -> system libz3.so.4)
