#!/usr/bin/env bash
# scripts/dod/completion_wave_cd.sh — DoD for Waves C+D (design docs).
set -uo pipefail
LOG=/home/z/my-project/scripts/logs/completion_wave_cd_dod.log
mkdir -p "$(dirname "$LOG")"
declare -A results=()
overall=PASS
cd /home/z/my-project/vuma

if [ -f scripts/audit/completion_wave_c_riscv64_design.md ]; then
  if grep -qE 'RISC-V|riscv64|callee.saved|phased.rollout' scripts/audit/completion_wave_c_riscv64_design.md; then
    results["CC-a-riscv64-design"]=PASS
  else
    results["CC-a-riscv64-design"]="FAIL"
    overall=FAIL
  fi
else
  results["CC-a-riscv64-design"]="FAIL (no md)"
  overall=FAIL
fi

if [ -f scripts/audit/completion_wave_d_ppc64_design.md ]; then
  if grep -qE 'PPC|ppc64|SVR4|callee.saved|phased.rollout' scripts/audit/completion_wave_d_ppc64_design.md; then
    results["CD-a-ppc64-design"]=PASS
  else
    results["CD-a-ppc64-design"]="FAIL"
    overall=FAIL
  fi
else
  results["CD-a-ppc64-design"]="FAIL (no md)"
  overall=FAIL
fi

{
  echo "{"
  echo "  \"wave\": \"C+D\","
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
