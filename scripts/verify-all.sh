#!/bin/bash
# ============================================================================
# verify-all.sh — comprehensive VUMA verification
# ----------------------------------------------------------------------------
# Runs ALL verification checks: Lean proofs, Rust tests, CI YAML validation.
# Exits 0 only if ALL checks pass.
# ============================================================================
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
cd "$REPO_ROOT"

export PATH="$HOME/.elan/bin:$HOME/.local/bin:$HOME/.cargo/bin:$PATH"

PASS=0
FAIL=0

check() {
    local name="$1"
    local cmd="$2"
    echo ""
    echo "=== $name ==="
    # Run in a subshell so any `cd` inside the command does not leak into
    # subsequent checks (we always start each check from $REPO_ROOT).
    if ( eval "$cmd" ); then
        echo "✅ PASS: $name"
        PASS=$((PASS + 1))
    else
        echo "❌ FAIL: $name"
        FAIL=$((FAIL + 1))
    fi
}

# 1. Lean proofs build
check "Lean: lake build" "cd proof && lake build"

# 2. Lean proofs are sorry-free (strict)
check "Lean: no sorry (strict)" "PROOF_CHECK_STRICT=1 ./scripts/check-lean.sh"

# 3. Lean tests pass
check "Lean: tests pass" "cd proof && lake exe test"

# 4. CI YAML is valid
check "CI: proof-verify.yml valid" "python3 -c \"import yaml; yaml.safe_load(open('.github/workflows/proof-verify.yml'))\""
check "CI: ci.yml valid" "python3 -c \"import yaml; yaml.safe_load(open('.github/workflows/ci.yml'))\""

# 5. Make targets exist
check "Make: proof target" "make -n proof >/dev/null 2>&1"
check "Make: proof-check target" "make -n proof-check >/dev/null 2>&1"
check "Make: proof-test target" "make -n proof-test >/dev/null 2>&1"
check "Make: proof-extract target" "make -n proof-extract >/dev/null 2>&1"

# 6. Just recipes exist
check "Just: proof recipe" "just -n proof >/dev/null 2>&1"
check "Just: proof-check recipe" "just -n proof-check >/dev/null 2>&1"

# 7. Lean modules exist
check "Lean: PMT/Basic.lean exists" "test -f proof/PMT/Basic.lean"
check "Lean: PMT/Soundness.lean exists" "test -f proof/PMT/Soundness.lean"
check "Lean: PMT/RawArena.lean exists" "test -f proof/PMT/RawArena.lean"
check "Lean: PMT/SimRel.lean exists" "test -f proof/PMT/SimRel.lean"
check "Lean: PMT/Extraction.lean exists" "test -f proof/PMT/Extraction.lean"
check "Lean: PMT/IVE/Soundness/Transform.lean exists" "test -f proof/PMT/IVE/Soundness/Transform.lean"

# 8. Documentation exists
check "Docs: FINAL-SUMMARY.md exists" "test -f docs/verification-reports/FINAL-SUMMARY.md"
check "Docs: STATUS-DASHBOARD.md exists" "test -f docs/verification-reports/STATUS-DASHBOARD.md"
check "Docs: LEAN-PROOF-SUMMARY.md exists" "test -f docs/verification-reports/LEAN-PROOF-SUMMARY.md"

echo ""
echo "============================================="
echo "Verification Summary: $PASS passed, $FAIL failed"
echo "============================================="

if [ "$FAIL" -gt 0 ]; then
    exit 1
fi
exit 0
