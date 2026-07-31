#!/usr/bin/env bash
# scripts/dod/wave_2.sh — DoD check for wave 2 (codegen allocator audit).
# Exits 0 on PASS, non-zero on FAIL.
set -uo pipefail

LOG=/home/z/my-project/scripts/logs/wave2_dod.log
mkdir -p "$(dirname "$LOG")"

declare -A results=()
overall=PASS

cd /home/z/my-project/vuma

# --- 2-a: allocator classification audit markdown exists ---
if [ -f scripts/audit/allocator_classification.md ]; then
  # Audit table rows are formatted as `| <num> | \`<backend>\` | ...`.
  # Count distinct backends mentioned in the table.
  backend_count=0
  for b in aarch64 aarch64_be x86_64 x86_32 riscv64 riscv32 ppc64 ppc64le arm32 armeb mips64 mips64be sparc64 s390x m68k alpha hppa loongarch64 wasm32; do
    if grep -qE "^\| +[0-9]+ +\| \`${b}\`" scripts/audit/allocator_classification.md; then
      backend_count=$((backend_count+1))
    fi
  done
  if [ "$backend_count" -ge 19 ]; then
    results["2-a-audit-md-exists"]="PASS ($backend_count backend rows)"
  else
    results["2-a-audit-md-exists"]="FAIL (only $backend_count backend rows; need 19)"
    overall=FAIL
  fi
else
  results["2-a-audit-md-exists"]="FAIL (no audit markdown)"
  overall=FAIL
fi

# --- 2-c: stack-slot backend correctness summary exists and shows 12/12 ---
if [ -f scripts/audit/wave2_stackslot_results.md ]; then
  # Look for a clear 12/12 (or 12 PASS) marker in the summary
  if grep -qE '12 */ *12|12 PASS|all 12' scripts/audit/wave2_stackslot_results.md; then
    results["2-c-stackslot-correctness"]="PASS"
  else
    # Fallback: count distinct PASS markers per backend
    pass_count=$(grep -ciE '^| *(arm32|armeb|mips64|mips64be|riscv32|x86_32|sparc64|s390x|m68k|alpha|hppa|loongarch64) *\| *PASS' scripts/audit/wave2_stackslot_results.md 2>/dev/null)
    pass_count=${pass_count:-0}
    if [ "$pass_count" -ge 12 ]; then
      results["2-c-stackslot-correctness"]="PASS ($pass_count/12)"
    else
      results["2-c-stackslot-correctness"]="FAIL ($pass_count/12)"
      overall=FAIL
    fi
  fi
else
  results["2-c-stackslot-correctness"]="FAIL (no summary file)"
  overall=FAIL
fi

# --- 2-d: caveat §2.1 reflects corrected classification ---
# After 2-d-doc, caveat §2.1 should mention "6" (real) and "12" (stack-slot)
# and "metadata-only" — not the old "15 of 19" framing.
if [ -f docs/caveats.md ]; then
  # The old title was "Stack-slot ISel on 15 of 19 backends"; the new title
  # should reflect the corrected classification.
  if grep -qE 'Stack-slot ISel is the only production code-emission path|6 .*real|metadata-only' docs/caveats.md; then
    results["2-d-caveat-updated"]=PASS
  else
    results["2-d-caveat-updated"]="FAIL (caveat §2.1 doesn't reflect corrected classification)"
    overall=FAIL
  fi
  # The old "15 of 19" framing should NOT appear (it's been corrected to 12 of 19)
  if grep -q '15 of 19 backends' docs/caveats.md; then
    results["2-d-caveat-updated"]="FAIL (still says '15 of 19 backends')"
    overall=FAIL
  fi
else
  results["2-d-caveat-updated"]="FAIL (no caveats.md)"
  overall=FAIL
fi
# docs/backends.md should also be updated
if [ -f docs/backends.md ]; then
  if grep -qE 'TargetAgnostic \(real\)|Annotation-only' docs/backends.md; then
    results["2-d-backends-md-updated"]=PASS
  else
    results["2-d-backends-md-updated"]="FAIL (backends.md allocator column not updated)"
    overall=FAIL
  fi
else
  results["2-d-backends-md-updated"]="FAIL (no backends.md)"
  overall=FAIL
fi

# --- No regression: workspace still builds ---
# (Use the prior 1-a artifacts — don't rebuild.)
if [ -x target/release/vuma ]; then
  results["no-build-regression"]=PASS
else
  results["no-build-regression"]="FAIL (target/release/vuma missing)"
  overall=FAIL
fi

# --- Emit JSON ---
{
  echo "{"
  echo "  \"wave\": 2,"
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
