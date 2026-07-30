#!/usr/bin/env bash
# z3-env.sh — source this to expose the Z3 4.13.3 dev shim to pkg-config / cargo.
# Installed by wave 0, task 0-a-install.
#
# The system runtime libz3.so.4 (Debian libz3-4 4.13.3-1) is the Z3 library.
# This shim provides the -dev artifacts (headers, libz3.so symlink, z3.pc) under
# $HOME/.local because libz3-dev could not be apt-installed (no root in sandbox).
#
# Usage:
#   source scripts/env/z3-env.sh
#   pkg-config --modversion z3        # -> 4.13.3
#   cargo build ...                   # z3-sys will pick up Z3 via pkg-config

export Z3_PREFIX="${Z3_PREFIX:-$HOME/.local}"
export PKG_CONFIG_PATH="$Z3_PREFIX/lib/pkgconfig${PKG_CONFIG_PATH:+:$PKG_CONFIG_PATH}"
export LIBRARY_PATH="$Z3_PREFIX/lib${LIBRARY_PATH:+:$LIBRARY_PATH}"   # link-time search for gcc
export LD_LIBRARY_PATH="$Z3_PREFIX/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"  # runtime search (follows symlink -> system libz3.so.4)
