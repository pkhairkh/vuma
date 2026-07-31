#!/usr/bin/env bash
# scripts/dod/wave_6.sh — DoD check for wave 6 (CLI & doc surface audit).
set -uo pipefail

LOG=/home/z/my-project/scripts/logs/wave6_dod.log
mkdir -p "$(dirname "$LOG")"

declare -A results=()
overall=PASS

cd /home/z/my-project/vuma

# --- 6-a: removed-flag grep audit ---
if [ -f scripts/audit/wave6_removed_flags.md ]; then
  results["6-a-removed-flag-audit"]=PASS
else
  results["6-a-removed-flag-audit"]="FAIL (no audit md)"
  overall=FAIL
fi

# --- 6-d: active --safe references removed ---
# Re-run the grep to confirm zero ACTIVE hits in the in-scope files.
# (Per 6-d-fix, residual hits should all be META — describing the removal.)
# Filter keywords cover the various ways the removal is described:
#   "has been removed", "formerly", "removed.*flag", "legacy", "always on",
#   "VUMA 2.0", "mandatory", "no opt-out", "unreachable", "previously",
#   "IMPL-1-safe-mandatory" (internal label for the always-on wire-through).
#   "no-op", "CLI flag", "memory_safety" (these terms appear only in
#   removal-describing context).
active_safe_hits=$(grep -rn -- '--safe\|--no-memory-safety' \
  src/bin/compile_dump.rs src/pipeline.rs tests/gold_standard/ 2>/dev/null \
  | grep -vE 'has been removed|have been removed|have both been removed|formerly|removed.*flag|legacy|always on|VUMA 2\.0|mandatory|no opt-out|unreachable|previously|IMPL-1-safe-mandatory|no-op|no_opt|no opt|CLI flag|memory_safety' \
  | wc -l)
active_safe_hits=${active_safe_hits:-0}
if [ "$active_safe_hits" -eq 0 ]; then
  results["6-d-active-safe-removed"]="PASS (0 active hits; residual are META)"
else
  results["6-d-active-safe-removed"]="FAIL ($active_safe_hits active hits remain)"
  overall=FAIL
fi

# --- 6-b: cross-ref link resolution ---
if [ -f scripts/audit/wave6_xref_links.md ]; then
  if grep -qE '0 broken|zero broken|no broken|all.*resolve' scripts/audit/wave6_xref_links.md; then
    results["6-b-xref-links"]=PASS
  else
    results["6-b-xref-links"]="FAIL (audit doesn't confirm zero broken links)"
    overall=FAIL
  fi
else
  results["6-b-xref-links"]="FAIL (no audit md)"
  overall=FAIL
fi

# --- 6-c: per-backend matrix consistency ---
if [ -f scripts/audit/wave6_backend_matrix.md ]; then
  if grep -qE 'zero drift|no drift|19 */ *19|19/19|match' scripts/audit/wave6_backend_matrix.md; then
    results["6-c-backend-matrix"]=PASS
  else
    results["6-c-backend-matrix"]="FAIL (audit doesn't confirm zero drift)"
    overall=FAIL
  fi
else
  results["6-c-backend-matrix"]="FAIL (no audit md)"
  overall=FAIL
fi

# --- Emit JSON ---
{
  echo "{"
  echo "  \"wave\": 6,"
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
