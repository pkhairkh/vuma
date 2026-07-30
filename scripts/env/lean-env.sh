#!/usr/bin/env bash
# lean-env.sh — source this to put `lean`, `lake`, and `elan` on PATH.
# Installed by wave 0, task 0-e-install.
#
# Caveat §3.2 requires Lean 4 v4.21.0 for the formal proofs under `proof/`.
# Installed via the official elan installer
# (https://raw.githubusercontent.com/leanprover/elan/master/elan-init.sh)
# with `--default-toolchain leanprover/lean4:v4.21.0`. elan writes its
# shims to `$ELAN_HOME/bin` (default `$HOME/.elan/bin`) and pins the
# default toolchain to `leanprover/lean4:v4.21.0` in
# `$ELAN_HOME/settings.toml`. The actual toolchain is fetched on first
# use into `$ELAN_HOME/toolchains/`. No root needed.
#
# The `proof/lean-toolchain` file pins the same channel
# (`leanprover/lean4:v4.21.0`), so `lake build` invoked from inside
# `proof/` auto-selects the right toolchain via elan's override mechanism.
#
# Usage:
#   source scripts/env/lean-env.sh
#   lean --version            # -> Lean (version 4.21.0, ...)
#   lake --version            # -> Lake version 5.0.0-... (Lean version 4.21.0)
#   elan show                 # -> leanprover/lean4:v4.21.0 (default)
#   which lean lake elan      # all under $ELAN_HOME/bin

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
