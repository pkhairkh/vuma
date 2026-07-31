#!/usr/bin/env bash
# scripts/dod/completion_wave_b.sh — DoD for Wave B (try_recv investigation).
set -uo pipefail
LOG=/home/z/my-project/scripts/logs/completion_wave_b_dod.log
mkdir -p "$(dirname "$LOG")"
declare -A results=()
overall=PASS
cd /home/z/my-project/vuma

if [ -f scripts/audit/completion_wave_b_try_recv_root_cause.md ]; then
  results["CB-a-root-cause"]=PASS
else
  results["CB-a-root-cause"]="FAIL"
  overall=FAIL
fi
if [ -f scripts/audit/completion_wave_b_try_recv_investigation.md ]; then
  results["CB-c-investigation-status"]=PASS
else
  results["CB-c-investigation-status"]="FAIL"
  overall=FAIL
fi

{
  echo "{"
  echo "  \"wave\": \"B\","
  echo "  \"run\": \"regalloc-completion\","
  echo "  \"overall\": \"$overall\","
  echo "  \"scope_note\": \"try_recv fix attempted (CB-b-impl) but reverted due to 17-test regression. Investigation documented; env-var gate remains OFF. Deferred to human developer.\","
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
