#!/usr/bin/env bash
# scripts/dod/wave_1.sh — DoD check for wave 1 (build baseline).
# Verifies the artifacts from the per-task sub-agents are present and consistent.
# Does NOT rebuild (those artifacts were already produced by 1-a/1-b/1-c).
# Exits 0 on PASS, non-zero on FAIL.
set -uo pipefail

LOG=/home/z/my-project/scripts/logs/wave1_dod.log
mkdir -p "$(dirname "$LOG")"

for shim in /home/z/my-project/vuma/scripts/env/*.sh; do
  # shellcheck disable=SC1090
  [ -r "$shim" ] && . "$shim"
done
export PATH="$HOME/.cargo/bin:$HOME/.elan/bin:$HOME/.wasmtime/bin:$HOME/.local/bin:$PATH"
export PKG_CONFIG_PATH="$HOME/.local/lib/pkgconfig:${PKG_CONFIG_PATH:-}"
export LIBRARY_PATH="$HOME/.local/lib:${LIBRARY_PATH:-}"
export LD_LIBRARY_PATH="$HOME/.local/lib:${LD_LIBRARY_PATH:-}"

declare -A results=()
overall=PASS

cd /home/z/my-project/vuma

# --- 1-a: clean release build artifacts ---
if [ -x target/release/vuma ] && [ -f /home/z/my-project/scripts/logs/wave1_build.log ]; then
  if grep -q 'BUILD_EXIT_CODE=0\|Finished.*release.*profile\|^Finished$' /home/z/my-project/scripts/logs/wave1_build.log; then
    results["1-a-clean-release-build"]=PASS
  else
    results["1-a-clean-release-build"]="FAIL (no 'Finished' marker in log)"
    overall=FAIL
  fi
else
  results["1-a-clean-release-build"]="FAIL (missing target/release/vuma or build log)"
  overall=FAIL
fi

# --- 1-b: pmt-runtime-check build artifacts + pmt_check symbols + no Lean ---
if [ -f /home/z/my-project/scripts/logs/wave1_build_pmt.log ] \
   && tail -5 /home/z/my-project/scripts/logs/wave1_build_pmt.log | grep -q 'BUILD_EXIT_CODE=0\|Finished.*release.*profile'; then
  results["1-b-pmt-runtime-check-build"]=PASS
else
  results["1-b-pmt-runtime-check-build"]=FAIL
  overall=FAIL
fi
# Find any codegen rlib from the feature build and check for pmt_check + absence of lean_ symbols
# Iterate ALL codegen rlibs — the feature-built one has a different hash than the
# no-feature one, and may not be the newest if a no-feature rebuild happened.
found_pmt_check=0
for rlib in target/release/deps/libvuma_codegen-*.rlib; do
  if [ -f "$rlib" ]; then
    # NOTE: do NOT use `strings | grep -q` here — under `set -o pipefail`,
    # `strings` receives SIGPIPE when `grep -q` exits early on first match,
    # causing the pipeline to return non-zero even though grep succeeded.
    # Capture strings output to a temp var instead.
    sym_dump=$(strings "$rlib" 2>/dev/null)
    if grep -qE 'verified_pmt_check|verified_capacity_check|src/codegen/src/runtime/pmt_check.rs' <<< "$sym_dump"; then
      found_pmt_check=1
      break
    fi
  fi
done
if [ "$found_pmt_check" -eq 1 ]; then
  results["1-b-pmt-check-symbols"]=PASS
else
  results["1-b-pmt-check-symbols"]="FAIL (no codegen rlib with pmt_check symbols)"
  overall=FAIL
fi
# No Lean runtime symbols should be linked into any vuma rlib
# (Same pipefail caveat — capture find|strings output to a temp file first.)
all_syms=$(mktemp)
for rlib in target/release/deps/libvuma*.rlib; do
  [ -f "$rlib" ] && strings "$rlib" 2>/dev/null
done > "$all_syms"
if grep -qiE '^lean_(init|main|finalize)|lean_(declare|mk_)' "$all_syms"; then
  results["1-b-no-lean-linkage"]=FAIL
  overall=FAIL
else
  results["1-b-no-lean-linkage"]=PASS
fi
rm -f "$all_syms"

# --- 1-c: Lean lake build (cached) — verify no sorry-tactic in prior log ---
if [ -f /home/z/my-project/scripts/logs/wave1_lean.log ]; then
  if grep -q 'Build completed successfully\|Replayed ' /home/z/my-project/scripts/logs/wave1_lean.log; then
    results["1-c-lake-build"]=PASS
  else
    results["1-c-lake-build"]="FAIL (no success marker in lean log)"
    overall=FAIL
  fi
  # Sorry-tactic warnings: the audit module name SorryFreeAudit is allowed.
  # Use grep -c but capture its exit-1-when-no-match separately from the count.
  sorry_count=$(grep -ciE 'uses.*sorry|sorryAx|declaration uses sorry' /home/z/my-project/scripts/logs/wave1_lean.log 2>/dev/null)
  sorry_count=${sorry_count:-0}
  if [ "$sorry_count" -eq 0 ]; then
    results["1-c-zero-sorry-tactic"]=PASS
  else
    results["1-c-zero-sorry-tactic"]="FAIL ($sorry_count sorry-tactic warnings)"
    overall=FAIL
  fi
else
  results["1-c-lake-build"]="FAIL (no lean log)"
  results["1-c-zero-sorry-tactic"]="FAIL (no lean log)"
  overall=FAIL
fi

# --- clippy (re-run only — it's typically <30s incremental) ---
if cargo clippy --workspace --release -- -D warnings >/tmp/w1_clippy.log 2>&1; then
  results["clippy-workspace-clean"]=PASS
else
  results["clippy-workspace-clean"]=FAIL
  overall=FAIL
fi

# --- Emit JSON ---
{
  echo "{"
  echo "  \"wave\": 1,"
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
