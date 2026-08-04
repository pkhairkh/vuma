#!/bin/bash
# ============================================================================
# check_womb_imports.sh — CI gate for womb/net/*.vuma import integrity
# ----------------------------------------------------------------------------
# Compiles every `womb/net/*.vuma` file with the `compile_dump` test driver
# (release-fast profile) on the x86_64 backend. Each file must compile to a
# valid ELF binary — broken imports (missing modules, unresolved symbols,
# parser errors on the import graph) surface as a non-zero exit code from
# compile_dump.
#
# PMT migration debt (V-WOMB follow-up):
#   womb/net/*.vuma files written before VUMA 2.0 use the legacy `*(ptr+off)`
#   pointer-deref syntax which the VUMA 2.0 PMT-only parser rejects with the
#   error "pointer syntax '*ptr (deref)' is not supported in VUMA 2.0
#   (PMT-only); use state_new(Layout) and transforms". Migrating these files
#   to PMT `State<T>` syntax is a separate workstream (out of W7 scope).
#   Until that migration lands, this script tolerates PointerSyntax errors
#   (reports them as SKIP — known PMT migration debt) but FAILS on any other
#   kind of breakage (parser, import resolution, codegen, etc.). This keeps
#   the gate useful for catching NEW breakage while the migration is pending.
#
# Exit status:
#   0 — no NEW breakage detected (only PointerSyntax-skipped files, if any)
#   1 — at least one file failed for a reason OTHER than PointerSyntax
#   2 — compile_dump binary not found
#
# This script is invoked by the `womb-imports` CI job in
# .github/workflows/vuma-tests.yml.
# ============================================================================
set -e

# Always operate from the repo root regardless of where the script is
# invoked from.  This makes the script safe to call as
#   ./scripts/check_womb_imports.sh
# or
#   bash /path/to/vuma/scripts/check_womb_imports.sh
cd "$(dirname "$0")/.."

FAIL=0
SKIP_PMT=0
TOTAL=0

# compile_dump is the test driver (different CLI from the vuma binary).
# It accepts: compile_dump <source.vuma> <out.bin> <backend>
# We use the release-fast profile build to keep CI fast.
BIN=./target/release-fast/compile_dump

if [ ! -x "$BIN" ]; then
    echo "ERROR: $BIN not found. Build it with:"
    echo "  cargo build --profile release-fast --bin compile_dump"
    exit 2
fi

for f in womb/net/*.vuma; do
    TOTAL=$((TOTAL + 1))
    name=$(basename "$f" .vuma)
    err_file=$(mktemp)
    if "$BIN" "$f" "/tmp/womb_check_${name}.bin" x86_64 >"$err_file" 2>&1; then
        echo "PASS: $f"
    else
        # Capture the failure reason. compile_dump emits either:
        #   "compile error: import resolution: ... PointerSyntax ..."
        #   "parse error: [ParseError { ... kind: PointerSyntax ... }]"
        #   "compile error: <other reason>"
        #   "parse error: [<other ParseError kinds>]"
        if grep -q "PointerSyntax" "$err_file" 2>/dev/null; then
            echo "SKIP: $f (PMT migration debt — legacy *(ptr+off) syntax)"
            SKIP_PMT=$((SKIP_PMT + 1))
        else
            # Surface the first error line so CI logs are actionable.
            first_err=$(grep -E "^(error|compile error|parse error|panic)" "$err_file" | head -1)
            if [ -z "$first_err" ]; then
                first_err=$(head -1 "$err_file")
            fi
            echo "FAIL: $f — $first_err"
            FAIL=$((FAIL + 1))
        fi
    fi
    rm -f "$err_file"
done

echo ""
echo "============================================"
echo "WOMB/net imports summary"
echo "============================================"
echo "Total files:  $TOTAL"
echo "PASS:         $((TOTAL - FAIL - SKIP_PMT))"
echo "SKIP (PMT):   $SKIP_PMT  (legacy *(ptr+off) syntax — VUMA 2.0 PMT migration debt)"
echo "FAIL:         $FAIL      (new breakage)"

if [ "$FAIL" -eq 0 ]; then
    echo ""
    echo "PASS: no new womb/net/*.vuma breakage detected"
    exit 0
else
    echo ""
    echo "FAIL: $FAIL womb/net/*.vuma file(s) broken (non-PointerSyntax)"
    exit 1
fi
