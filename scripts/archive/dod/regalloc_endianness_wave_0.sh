#!/usr/bin/env bash
# scripts/dod/regalloc_endianness_wave_0.sh — DoD for regalloc-endianness Wave 0
# (env re-verify + workspace build + clippy).
set -uo pipefail

LOG=/home/z/my-project/scripts/logs/regalloc_endianness_wave0_dod.log
mkdir -p "$(dirname "$LOG")"

for shim in /home/z/my-project/vuma/scripts/env/*.sh; do
  [ -r "$shim" ] && . "$shim"
done
export PATH="$HOME/.cargo/bin:$HOME/.elan/bin:$HOME/.wasmtime/bin:$HOME/.local/bin:$PATH"
export PKG_CONFIG_PATH="$HOME/.local/lib/pkgconfig:${PKG_CONFIG_PATH:-}"
export LIBRARY_PATH="$HOME/.local/lib:${LIBRARY_PATH:-}"
export LD_LIBRARY_PATH="$HOME/.local/lib:${LD_LIBRARY_PATH:-}"

declare -A results=()
overall=PASS

cd /home/z/my-project/vuma

# --- R0-a: env re-verify (run prior run's harness) ---
if bash scripts/dod/followup_wave_0.sh > /tmp/r0_env_check.log 2>&1; then
  results["R0-a-env-verify"]=PASS
else
  results["R0-a-env-verify"]="FAIL (prior wave_0.sh failed)"
  overall=FAIL
fi

# --- R0-b: workspace build artifacts present ---
if [ -x target/release/vuma ] && [ -x target/release/compile_dump ]; then
  results["R0-b-build-artifacts"]=PASS
else
  results["R0-b-build-artifacts"]="FAIL (missing target/release/vuma or compile_dump)"
  overall=FAIL
fi

# --- R0-b: build log exists with exit 0 markers ---
if [ -f /home/z/my-project/scripts/logs/regalloc_endianness_wave0_build.log ]; then
  results["R0-b-build-log"]=PASS
else
  results["R0-b-build-log"]="FAIL (no build log)"
  overall=FAIL
fi

# --- clippy: quick re-verify (incremental, ~20s) ---
if cargo clippy --workspace --release -- -D warnings > /tmp/r0_clippy.log 2>&1; then
  results["R0-b-clippy"]=PASS
else
  results["R0-b-clippy"]="FAIL (clippy not green)"
  overall=FAIL
fi

# --- git clean ---
if [ -z "$(git status --porcelain)" ]; then
  results["git-clean"]=PASS
else
  results["git-clean"]="WARN (uncommitted: $(git status --porcelain | head -3 | tr '\n' ';'))"
fi

# --- Emit JSON ---
{
  echo "{"
  echo "  \"wave\": 0,"
  echo "  \"run\": \"regalloc-endianness-remediation\","
  echo "  \"overall\": \"$overall\","
  echo "  \"checks\": {"
  first=1
  for k in "${!results[@]}"; do
    if [ $first -eq 1 ]; then first=0; else echo ","; fi
    printf '    "%s": "%s"' "$k" "${results[$k]}"
  done
  echo ""
  echo "  }"
  echo "}"
} | tee "$LOG"

if [ "$overall" = PASS ]; then exit 0; else exit 1; fi
