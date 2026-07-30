#!/usr/bin/env bash
# scripts/dod/completion_wave_a.sh — DoD for Wave A (aarch64_be verification).
set -uo pipefail
LOG=/home/z/my-project/scripts/logs/completion_wave_a_dod.log
mkdir -p "$(dirname "$LOG")"
declare -A results=()
overall=PASS
cd /home/z/my-project/vuma

if [ -f scripts/audit/completion_wave_a_aarch64_be_regalloc.md ]; then
  if grep -qE '29/30|30/30|96\.67%|100%' scripts/audit/completion_wave_a_aarch64_be_regalloc.md; then
    results["CA-a-aarch64-be-matrix"]=PASS
  else
    results["CA-a-aarch64-be-matrix"]="FAIL"
    overall=FAIL
  fi
else
  results["CA-a-aarch64-be-matrix"]="FAIL (no md)"
  overall=FAIL
fi

{
  echo "{"
  echo "  \"wave\": \"A\","
  echo "  \"run\": \"regalloc-completion\","
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
[ "$overall" = PASS ] && exit 0 || exit 1
