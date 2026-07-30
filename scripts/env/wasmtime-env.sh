#!/usr/bin/env bash
# wasmtime-env.sh — source this to put `wasmtime` (v29+) on PATH.
# Installed by wave 0, task 0-d-install; upgraded to latest stable by
# follow-up task F0-d-install.
#
# Caveat §4.3 requires wasmtime v29+ for the VUMA wasm32 backend row.
# Originally installed by 0-d-install via the official installer
# (https://wasmtime.dev/install.sh, `--version v29.0.0`), which unpacks
# the upstream `wasmtime-v29.0.0-x86_64-linux.tar.xz` release into
# `$WASMTIME_HOME` (`$HOME/.wasmtime`) and drops the `wasmtime` binary
# at `$WASMTIME_HOME/bin/wasmtime`. No root needed.
#
# Follow-up F0-d-install (2026-07-30) re-queried the latest stable:
#   - GitHub releases/latest tag_name = v47.0.2 (2026-07-21 build).
#   - The wasmtime.dev installer's `--version latest` argument is broken
#     in the current install.sh (substitutes the literal token `{` into
#     the download URL, yielding a nested-brace curl error). Worked
#     around by downloading the upstream release tarball directly:
#     https://github.com/bytecodealliance/wasmtime/releases/download/
#       v47.0.2/wasmtime-v47.0.2-x86_64-linux.tar.xz
#     and copying `wasmtime` + `wasmtime-min` into `$WASMTIME_HOME/bin/`.
#   - `wasmtime --version` now reports `wasmtime 47.0.2 (90fed3c6a
#     2026-07-21)`; `wasmtime run --help` exits 0.
#
# Usage:
#   source scripts/env/wasmtime-env.sh
#   wasmtime --version        # -> wasmtime 47.0.2 (...)
#   which wasmtime            # -> /home/z/.wasmtime/bin/wasmtime

# Default WASMTIME_HOME if not already set (matches installer's own export).
export WASMTIME_HOME="${WASMTIME_HOME:-${HOME}/.wasmtime}"

# Idempotent: only prepend $WASMTIME_HOME/bin if not already on PATH.
case ":${PATH}:" in
    *":${WASMTIME_HOME}/bin:"*)
        # Already on PATH; nothing to do.
        ;;
    *)
        export PATH="${WASMTIME_HOME}/bin:${PATH}"
        ;;
esac

# Sanity-check that wasmtime is reachable. This is a no-op if everything
# is in place; it prints a one-line warning otherwise.
if ! command -v wasmtime >/dev/null 2>&1; then
    echo "wasmtime-env.sh: WARNING: wasmtime not found on PATH" \
         "(expected in ${WASMTIME_HOME}/bin)" >&2
fi
