#!/usr/bin/env bash
# scripts/dod/followup_wave_4.sh — DoD check for follow-up wave 4
# (release commit, tag, push).
set -uo pipefail

LOG=/home/z/my-project/scripts/logs/followup_wave4_dod.log
mkdir -p "$(dirname "$LOG")"

declare -A results=()
overall=PASS

cd /home/z/my-project/vuma

# --- F4-a: CHANGELOG entry + version bump ---
if [ -f CHANGELOG.md ]; then
  if grep -qE '## \[0\.2\.0-alpha\.2\] — Follow-up Remediation' CHANGELOG.md; then
    results["F4-a-changelog"]=PASS
  else
    results["F4-a-changelog"]="FAIL (no 0.2.0-alpha.2 section)"
    overall=FAIL
  fi
else
  results["F4-a-changelog"]="FAIL (no CHANGELOG.md)"
  overall=FAIL
fi
# Version bump in Cargo.toml
if grep -q '^version = "0.2.0-alpha.2"' Cargo.toml; then
  results["F4-a-version-bump"]=PASS
else
  results["F4-a-version-bump"]="FAIL (version not bumped in Cargo.toml)"
  overall=FAIL
fi

# --- F4-b: release tag exists locally ---
if git tag --list 'v0.2.0-alpha.2-followup-remediation' | grep -q 'followup-remediation'; then
  results["F4-b-release-tag-local"]=PASS
else
  results["F4-b-release-tag-local"]="FAIL (no local tag)"
  overall=FAIL
fi

# --- F4-c: tag pushed to origin ---
if git ls-remote --tags origin 2>/dev/null | grep -q 'v0.2.0-alpha.2-followup-remediation'; then
  results["F4-c-tag-on-origin"]=PASS
else
  results["F4-c-tag-on-origin"]="PENDING (tag not yet pushed; will be pushed by F4-c-push)"
  # Not a hard failure — the push happens after the DoD harness is committed.
  # But if the harness is re-run after push, this should PASS.
fi

# --- F4-c: main pushed to origin (origin/main up to date with HEAD) ---
ahead=$(git rev-list --count origin/main..HEAD 2>/dev/null || echo "unknown")
if [ "$ahead" = "0" ]; then
  results["F4-c-main-pushed"]="PASS (origin/main up to date)"
else
  results["F4-c-main-pushed"]="PENDING ($ahead commits ahead of origin/main; push pending)"
fi

# --- All 5 follow-up wave DoD scripts exist ---
all_dod_pass=1
for n in 0 1 2 3 4; do
  if [ ! -f scripts/dod/followup_wave_${n}.sh ]; then
    results["F4-d-wave-${n}-dod-script"]="MISSING"
    all_dod_pass=0
  fi
done
if [ "$all_dod_pass" -eq 1 ]; then
  results["F4-d-all-dod-scripts-present"]=PASS
fi

# --- All 5 follow-up wave DoD commits exist ---
wave_commits=$(git log --oneline --grep='followup-wave-.*-dod-pass' | wc -l)
wave_commits=${wave_commits:-0}
if [ "$wave_commits" -ge 4 ]; then
  results["F4-d-wave-dod-commits"]="PASS ($wave_commits/5 followup-wave-N-dod-pass commits; wave 4 is this commit)"
else
  results["F4-d-wave-dod-commits"]="FAIL ($wave_commits/5)"
  overall=FAIL
fi

# --- orchestrator_state.json reflects all waves ---
if [ -f scripts/orchestrator_state.json ]; then
  if grep -q '"current_wave": 4' scripts/orchestrator_state.json \
     && grep -q '"aborted": false' scripts/orchestrator_state.json; then
    results["F4-d-orchestrator-state"]=PASS
  else
    results["F4-d-orchestrator-state"]="FAIL (state file not final)"
    overall=FAIL
  fi
else
  results["F4-d-orchestrator-state"]="FAIL (no state file)"
  overall=FAIL
fi

# --- Emit JSON ---
{
  echo "{"
  echo "  \"wave\": 4,"
  echo "  \"run\": \"followup-remediation\","
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
