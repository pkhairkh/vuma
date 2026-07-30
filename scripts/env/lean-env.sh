#!/usr/bin/env bash
# lean-env.sh — source this to put `lean`, `lake`, and `elan` on PATH.
# Installed by wave 0, task 0-e-install; re-queried latest stable by
# follow-up task F0-e-install.
#
# CAVEAT §3.2 requires Lean 4 v4.21.0 for the formal proofs under `proof/`.
# The `proof/lean-toolchain` file pins `leanprover/lean4:v4.21.0`, so
# `lake build` invoked from inside `proof/` auto-selects v4.21.0 via
# elan's directory-override mechanism. This pin is canonical and MUST NOT
# be edited.
#
# elan writes its shims to `$ELAN_HOME/bin` (default `$HOME/.elan/bin`)
# and fetches each toolchain into `$ELAN_HOME/toolchains/` on first use.
# No root needed.
#
# DUAL-VERSION STATE (after F0-e-install, 2026-07-30):
#   * elan DEFAULT toolchain (used outside proof/): leanprover/lean4:v4.32.2
#     — the latest stable Lean 4 release per GitHub `releases/latest`.
#       `lean --version`  -> Lean (version 4.32.2, ..., commit f3b06c705e6c)
#       `lake --version`  -> Lake version 5.0.0-src+f3b06c7 (Lean 4.32.2)
#   * Project pin (used inside proof/ via lean-toolchain): v4.21.0
#       `cd proof && lean --version` -> Lean (version 4.21.0, ..., commit 6741444a63ee)
#       `cd proof && lake build`     -> Build completed successfully (exit 0)
#   Both toolchains live side-by-side under `$ELAN_HOME/toolchains/`.
#   elan's override layer selects v4.21.0 automatically whenever cwd is
#   inside `proof/` (because of `proof/lean-toolchain`), and falls back to
#   the default (v4.32.2) everywhere else.
#
# Installation notes (F0-e-install):
#   * `elan toolchain install` does NOT accept a `--default` flag (unlike
#     rustup); the default must be set separately via `elan default <toolchain>`.
#   * The v4.32.2 tarball (~564 MB .tar.zst) could not be extracted by
#     elan directly because the host filesystem was disk-constrained
#     (<2.2 GB free; elan's download+extract-to-tmp peaked above that).
#     Workaround: stream-downloaded the tarball and decompressed it with
#     the Python `zstandard` module (no `zstd` binary available, no root
#     for apt), extracting directly into the final toolchain directory
#     (no intermediate .tmp copy). The optional `src/` tree (Lean's own
#     .lean sources for development) was omitted to save ~150 MB; it is
#     not required to run `lean`/`lake`/`lake build`.
#   * `rustup` toolchain `share/doc` and `share/man` trees were removed
#     to free ~47 MB of headroom for the v4.21.0 `lake build` cache.
#   * All static archives (`*.a`, ~396 MB total: `lib/lean/libLean.a`
#     alone is 297 MB; the remaining ~99 MB spans `libStd.a`, `libInit.a`,
#     `libLake.a`, `libleancpp.a`, `libcrypto.a`, `libssl.a`, `libgmp.a`,
#     `libc++.a`, etc.) were removed from the v4.32.2 toolchain to free
#     space for git operations and the v4.21.0 build cache. Static
#     archives are only used for STATIC linking of Lean/C++ deps into
#     third-party binaries; the `lean` and `lake` executables load
#     `libleanshared.so` / `libLake_shared.so` at runtime and are
#     unaffected. Normal `lake build` (shared-link mode) is unaffected.
#     To restore them, re-run `elan toolchain install
#     leanprover/lean4:v4.32.2` on a host with more free space.
#
# Usage:
#   source scripts/env/lean-env.sh
#   lean --version            # -> Lean (version 4.32.2, ...) [elan default]
#   lake --version            # -> Lake version 5.0.0-src+... (Lean 4.32.2)
#   elan show                 # -> leanprover/lean4:v4.32.2 (default)
#                                     + leanprover/lean4:v4.21.0 (installed)
#   cd proof && lean --version  # -> Lean (version 4.21.0, ...) [project pin]
#   which lean lake elan        # all under $ELAN_HOME/bin

# Default ELAN_HOME if not already set (matches elan's own convention).
export ELAN_HOME="${ELAN_HOME:-${HOME}/.elan}"

# Idempotent: only prepend $ELAN_HOME/bin if not already on PATH.
case ":${PATH}:" in
    *":${ELAN_HOME}/bin:"*)
        # Already on PATH; nothing to do.
        ;;
    *)
        export PATH="${ELAN_HOME}/bin:${PATH}"
        ;;
esac

# Sanity-check that lean/lake/elan are reachable. This is a no-op if
# everything is in place; it prints a one-line warning otherwise.
if ! command -v lean >/dev/null 2>&1; then
    echo "lean-env.sh: WARNING: lean not found on PATH" \
         "(expected in ${ELAN_HOME}/bin)" >&2
fi
if ! command -v elan >/dev/null 2>&1; then
    echo "lean-env.sh: WARNING: elan not found on PATH" \
         "(expected in ${ELAN_HOME}/bin)" >&2
fi
