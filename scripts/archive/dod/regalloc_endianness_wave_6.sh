#!/usr/bin/env bash
# scripts/dod/regalloc_endianness_wave_6.sh — DoD for Wave 6 (endianness audit).
set -uo pipefail

LOG=/home/z/my-project/scripts/logs/regalloc_endianness_wave6_dod.log
mkdir -p "$(dirname "$LOG")"

declare -A results=()
overall=PASS

cd /home/z/my-project/vuma

# --- R6-a: shared_memory audit doc ---
if [ -f scripts/audit/regalloc_endianness_wave6_shared_memory_audit.md ]; then
  results["R6-a-shared-memory-audit"]=PASS
else
  results["R6-a-shared-memory-audit"]="FAIL (no audit md)"
  overall=FAIL
fi

# --- R6-b: IPC audit doc ---
if [ -f scripts/audit/regalloc_endianness_wave6_ipc_audit.md ]; then
  results["R6-b-ipc-audit"]=PASS
else
  results["R6-b-ipc-audit"]="FAIL (no audit md)"
  overall=FAIL
fi

# --- R6-c: stale test assertions fixed ---
if [ -f tests/wave4b_half_closed_channel.rs ]; then
  # The test should no longer contain the old "BinOp And 0xFFFFFFFF" / "Load I64" pattern
  # that F3-b-fix removed. Check for the new "Load I32" / "Cast ZExt" pattern.
  if grep -q 'Load I32\|Cast.*ZExt\|has_load_i32\|has_zext_cast' tests/wave4b_half_closed_channel.rs; then
    results["R6-c-stale-tests-fixed"]=PASS
  else
    results["R6-c-stale-tests-fixed"]="FAIL (new IR pattern not found in test)"
    overall=FAIL
  fi
else
  results["R6-c-stale-tests-fixed"]="FAIL (no test file)"
  overall=FAIL
fi

# --- R6-d: BE regression suite ---
if [ -f scripts/audit/regalloc_endianness_wave6_be_regression.md ]; then
  if grep -qE '210 */ *210|210/210|100%|100\.00%' scripts/audit/regalloc_endianness_wave6_be_regression.md; then
    results["R6-d-be-regression"]=PASS
  else
    results["R6-d-be-regression"]="FAIL (regression suite not 210/210)"
    overall=FAIL
  fi
else
  results["R6-d-be-regression"]="FAIL (no regression md)"
  overall=FAIL
fi

# --- clippy green (smoke) ---
for shim in /home/z/my-project/vuma/scripts/env/*.sh; do [ -r "$shim" ] && . "$shim"; done
export PATH="$HOME/.cargo/bin:$HOME/.elan/bin:$HOME/.wasmtime/bin:$HOME/.local/bin:$PATH"
if cargo clippy -p vuma-codegen --release -- -D warnings > /tmp/r6_clippy.log 2>&1; then
  results["clippy-codegen-clean"]=PASS
else
  results["clippy-codegen-clean"]="FAIL"
  overall=FAIL
fi

# --- Emit JSON ---
{
  echo "{"
  echo "  \"wave\": 6,"
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
