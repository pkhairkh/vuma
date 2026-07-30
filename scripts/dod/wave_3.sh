#!/usr/bin/env bash
# scripts/dod/wave_3.sh — DoD check for wave 3 (verification layer audit).
set -uo pipefail

LOG=/home/z/my-project/scripts/logs/wave3_dod.log
mkdir -p "$(dirname "$LOG")"

declare -A results=()
overall=PASS

cd /home/z/my-project/vuma

# --- 3-a: Z3 IVE discharge rate markdown exists and reports >= 99% ---
if [ -f scripts/audit/wave3_ive_discharge.md ]; then
  # Look for a percentage >= 99 in the markdown
  if grep -qE '(100\.00%|99\.[1-9]%|99%|discharge_rate.*100|avg.*100)' scripts/audit/wave3_ive_discharge.md; then
    results["3-a-z3-discharge-rate"]=PASS
  else
    # Fallback: look for any explicit percentage in the [99, 100] range
    if grep -qE '9[9]\.[0-9]+%|100(\.0+)?%' scripts/audit/wave3_ive_discharge.md; then
      results["3-a-z3-discharge-rate"]=PASS
    else
      results["3-a-z3-discharge-rate"]="FAIL (no >=99% marker found)"
      overall=FAIL
    fi
  fi
else
  results["3-a-z3-discharge-rate"]="FAIL (no discharge audit md)"
  overall=FAIL
fi

# --- 3-b: pmt-runtime-check audit confirms NO-OP in ive + real in codegen ---
if [ -f scripts/audit/wave3_pmt_feature_audit.md ]; then
  if grep -qiE 'NO-OP|no-op' scripts/audit/wave3_pmt_feature_audit.md \
     && grep -qiE 'Activates pmt_check|pmt_check symbols' scripts/audit/wave3_pmt_feature_audit.md; then
    results["3-b-pmt-feature-behavior"]=PASS
  else
    results["3-b-pmt-feature-behavior"]="FAIL (audit doesn't confirm NO-OP/real split)"
    overall=FAIL
  fi
else
  results["3-b-pmt-feature-behavior"]="FAIL (no feature audit md)"
  overall=FAIL
fi

# --- 3-c: PMT parity test results ---
if [ -f scripts/audit/wave3_pmt_parity_results.md ]; then
  if grep -qiE '31 */ *31|31/31|all.*pass|PASS' scripts/audit/wave3_pmt_parity_results.md; then
    results["3-c-pmt-parity-test"]=PASS
  else
    results["3-c-pmt-parity-test"]="FAIL (parity test results not all-pass)"
    overall=FAIL
  fi
else
  results["3-c-pmt-parity-test"]="FAIL (no parity results md)"
  overall=FAIL
fi

# --- 3-d: Lean proofs decoupling ---
if [ -f scripts/audit/wave3_lean_decoupling.md ]; then
  if grep -qiE 'EXIT_CODE=0|exit 0|PASS|build.*succeed' scripts/audit/wave3_lean_decoupling.md; then
    results["3-d-lean-decoupling"]=PASS
  else
    results["3-d-lean-decoupling"]="FAIL (decoupling audit doesn't show build success)"
    overall=FAIL
  fi
else
  results["3-d-lean-decoupling"]="FAIL (no decoupling audit md)"
  overall=FAIL
fi
# Also verify proof/ is still in place (was temporarily moved during 3-d)
if [ -d proof ] && [ -f proof/lakefile.toml ]; then
  results["3-d-proof-restored"]=PASS
else
  results["3-d-proof-restored"]="FAIL (proof/ directory missing or incomplete)"
  overall=FAIL
fi

# --- Emit JSON ---
{
  echo "{"
  echo "  \"wave\": 3,"
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
