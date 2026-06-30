#!/bin/bash
# Test harness: compile every womb/*.vuma file with vuma build
# Reports per-file status: OK / FAIL / ERROR

set -u

VUMA_BIN="${VUMA_BIN:-/home/z/vuma_real/target/release/vuma}"
WOMB_DIR="${WOMB_DIR:-/home/z/vuma_real/womb}"
OUT_DIR="${OUT_DIR:-/tmp/womb_test}"
REPORT_FILE="${REPORT_FILE:-/home/z/my-project/download/womb_compile_report.txt}"

mkdir -p "$OUT_DIR"
mkdir -p "$(dirname "$REPORT_FILE")"

# Header
{
    echo "VUMA Womb Compilation Test Report"
    echo "Date: $(date -u +%Y-%m-%dT%H:%M:%SZ)"
    echo "VUMA binary: $VUMA_BIN"
    echo "Womb dir: $WOMB_DIR"
    echo "============================================"
    echo ""
} > "$REPORT_FILE"

total=0
ok=0
fail=0
skipped=0

printf "%-50s %s\n" "FILE" "STATUS" | tee -a "$REPORT_FILE"
printf "%-50s %s\n" "----" "------" | tee -a "$REPORT_FILE"

# Find all .vuma files
while IFS= read -r f; do
    total=$((total + 1))
    rel="${f#$WOMB_DIR/}"
    out="$OUT_DIR/$(echo "$rel" | tr '/' '_').out"
    
    # Skip core.vuma — it's intentionally uncompilable design documentation
    if [[ "$rel" == "core.vuma" ]]; then
        printf "%-50s SKIP (design doc)\n" "$rel" | tee -a "$REPORT_FILE"
        skipped=$((skipped + 1))
        continue
    fi
    
    # Run vuma build with --verification none (verification is separate)
    err_output=$(mktemp)
    if "$VUMA_BIN" build --verification none "$f" --output "$out" >"$err_output" 2>&1; then
        size=$(stat -c%s "$out" 2>/dev/null || echo "?")
        printf "%-50s OK (%s bytes)\n" "$rel" "$size" | tee -a "$REPORT_FILE"
        ok=$((ok + 1))
    else
        # Capture first error
        first_err=$(grep -E "^(error|panic)" "$err_output" | head -1)
        if [ -z "$first_err" ]; then
            first_err=$(head -1 "$err_output")
        fi
        printf "%-50s FAIL %s\n" "$rel" "$first_err" | tee -a "$REPORT_FILE"
        fail=$((fail + 1))
        # Save full error log
        cp "$err_output" "$OUT_DIR/$(echo "$rel" | tr '/' '_').err"
    fi
    rm -f "$err_output"
done < <(find "$WOMB_DIR" -name "*.vuma" | sort)

{
    echo ""
    echo "============================================"
    echo "SUMMARY"
    echo "============================================"
    echo "Total files: $total"
    echo "OK: $ok"
    echo "FAIL: $fail"
    echo "Skipped: $skipped"
    echo ""
    if [ "$fail" -gt 0 ]; then
        echo "FAILED FILES (error logs in $OUT_DIR/*.err):"
        ls "$OUT_DIR"/*.err 2>/dev/null | while read -r errf; do
            echo "  - $(basename "$errf" .err)"
        done
    fi
} | tee -a "$REPORT_FILE"

echo ""
echo "Report saved to: $REPORT_FILE"
exit $fail
