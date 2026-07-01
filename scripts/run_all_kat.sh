#!/bin/bash
set -u
COMPILE_DUMP="/home/z/vuma_real/target/release/compile_dump"
TEST_DIR="/home/z/my-project/scripts/womb_kat_tests"
OUT_DIR="/tmp/womb_kat"
REPORT="/home/z/my-project/download/womb_kat_results.md"
mkdir -p "$OUT_DIR"

{
    echo "# VUMA Womb KAT Results - ALL 84 Modules"
    echo ""
    echo "**Date:** $(date -u +%Y-%m-%dT%H:%M:%SZ)"
    echo ""
    echo "| # | Module | Expected | Actual | Status |"
    echo "|---|--------|----------|--------|--------|"
} > "$REPORT"

pass=0; fail=0; skip=0; idx=0

for test_file in "$TEST_DIR"/test_*.vuma; do
    idx=$((idx + 1))
    name=$(basename "$test_file" .vuma | sed 's/^test_//')
    
    # Extract expected from file comment
    expected=$(grep "Expected exit code:" "$test_file" | head -1 | sed 's/.*: //')
    
    bin="$OUT_DIR/$name.bin"
    err=$($COMPILE_DUMP "$test_file" "$bin" x86_64 2>&1)
    
    if [ $? -ne 0 ]; then
        echo "| $idx | $name | $expected | COMPILE_FAIL | FAIL |" >> "$REPORT"
        fail=$((fail + 1))
        continue
    fi
    
    chmod +x "$bin"
    timeout 10 "$bin" 2>/dev/null
    actual=$?
    
    if [ "$actual" -eq "$expected" ]; then
        echo "| $idx | $name | $expected | $actual | PASS |" >> "$REPORT"
        pass=$((pass + 1))
    else
        echo "| $idx | $name | $expected | $actual | FAIL |" >> "$REPORT"
        fail=$((fail + 1))
    fi
done

{
    echo ""
    echo "## Summary"
    echo ""
    echo "- **PASS:** $pass"
    echo "- **FAIL:** $fail"
    echo "- **Total:** $idx"
    echo ""
    if [ "$fail" -eq 0 ]; then
        echo "**ALL TESTS PASSED** ✅"
    else
        echo "**$fail failures** ❌"
    fi
} >> "$REPORT"

echo "PASS: $pass / $idx, FAIL: $fail"
echo "Report: $REPORT"
