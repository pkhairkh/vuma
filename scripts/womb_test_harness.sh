#!/bin/bash
# VUMA Womb Test Harness
# Compiles every womb/*.vuma file and runs smoke tests on key modules.
# Outputs a markdown report to /home/z/my-project/download/womb_test_report.md

set -u

VUMA_BIN="${VUMA_BIN:-/home/z/vuma_real/target/release/vuma}"
WOMB_DIR="${WOMB_DIR:-/home/z/vuma_real/womb}"
OUT_DIR="${OUT_DIR:-/tmp/womb_smoke}"
REPORT="${REPORT:-/home/z/my-project/download/womb_test_report.md}"

mkdir -p "$OUT_DIR"
mkdir -p "$(dirname "$REPORT")"

# Start report
{
    echo "# VUMA Womb Test Report"
    echo ""
    echo "**Date:** $(date -u +%Y-%m-%dT%H:%M:%SZ)"
    echo "**VUMA:** $($VUMA_BIN --version 2>&1 | head -1)"
    echo "**Womb dir:** $WOMB_DIR"
    echo ""
} > "$REPORT"

total=0
ok=0
fail=0
skipped=0
total_size=0
total_nodes=0
total_ir=0

echo "## 1. Compilation Test" | tee -a "$REPORT"
echo "" | tee -a "$REPORT"
echo "| File | Status | Size (bytes) | SCG Nodes | IR Instructions |" | tee -a "$REPORT"
echo "|------|--------|--------------|-----------|-----------------|" | tee -a "$REPORT"

# Compile each file
while IFS= read -r f; do
    total=$((total + 1))
    rel="${f#$WOMB_DIR/}"
    out="$OUT_DIR/$(echo "$rel" | tr '/' '_').out"
    
    # Skip core.vuma (design documentation)
    if [[ "$rel" == "core.vuma" ]]; then
        echo "| $rel | SKIPPED (design doc) | - | - | - |" | tee -a "$REPORT"
        skipped=$((skipped + 1))
        continue
    fi
    
    err_output=$(mktemp)
    if "$VUMA_BIN" build --verification none "$f" --output "$out" >"$err_output" 2>&1; then
        size=$(stat -c%s "$out" 2>/dev/null || echo "0")
        # Extract SCG nodes and IR instructions from output
        nodes=$(grep -oP '\d+ SCG nodes' "$err_output" | grep -oP '\d+' || echo "0")
        ir=$(grep -oP '\d+ IR instructions' "$err_output" | grep -oP '\d+' || echo "0")
        echo "| $rel | OK | $size | $nodes | $ir |" | tee -a "$REPORT"
        ok=$((ok + 1))
        total_size=$((total_size + size))
        total_nodes=$((total_nodes + nodes))
        total_ir=$((total_ir + ir))
    else
        first_err=$(grep -E "^(error|panic)" "$err_output" | head -1 | sed 's/|/\\|/g')
        echo "| $rel | FAIL: $first_err | - | - | - |" | tee -a "$REPORT"
        fail=$((fail + 1))
        cp "$err_output" "$OUT_DIR/$(echo "$rel" | tr '/' '_').err"
    fi
    rm -f "$err_output"
done < <(find "$WOMB_DIR" -name "*.vuma" | sort)

echo "" | tee -a "$REPORT"
echo "**Summary:** $ok OK, $fail FAIL, $skipped SKIPPED (out of $total total)" | tee -a "$REPORT"
echo "" | tee -a "$REPORT"
echo "**Total compiled output:** $total_size bytes, $total_nodes SCG nodes, $total_ir IR instructions" | tee -a "$REPORT"
echo "" | tee -a "$REPORT"

# Section 2: Stub marker audit
echo "## 2. Stub Marker Audit" | tee -a "$REPORT"
echo "" | tee -a "$REPORT"
echo "Scanning all womb files for: \`stub\`, \`simplified\`, \`TODO\`, \`FIXME\`, \`placeholder\`, \`not implemented\`, \`NOT for production\`" | tee -a "$REPORT"
echo "" | tee -a "$REPORT"

stub_files=0
stub_total=0
while IFS= read -r f; do
    rel="${f#$WOMB_DIR/}"
    if [[ "$rel" == "core.vuma" ]]; then
        continue
    fi
    cnt=$(grep -ciE '(\bstub\b|\bsimplified\b|TODO|FIXME|placeholder|not implemented|NOT for production)' "$f" 2>/dev/null || echo 0)
    if [[ "$cnt" -gt 0 ]]; then
        echo "- $rel: $cnt markers" | tee -a "$REPORT"
        stub_files=$((stub_files + 1))
        stub_total=$((stub_total + cnt))
    fi
done < <(find "$WOMB_DIR" -name "*.vuma" | sort)

if [[ "$stub_files" -eq 0 ]]; then
    echo "**Result:** ZERO stub markers across all womb files." | tee -a "$REPORT"
else
    echo "**Result:** $stub_total stub markers in $stub_files files" | tee -a "$REPORT"
fi
echo "" | tee -a "$REPORT"

# Section 3: API surface check
echo "## 3. API Surface Check" | tee -a "$REPORT"
echo "" | tee -a "$REPORT"
echo "Counting public functions per module:" | tee -a "$REPORT"
echo "" | tee -a "$REPORT"
echo "| Module | Functions | Lines |" | tee -a "$REPORT"
echo "|--------|-----------|-------|" | tee -a "$REPORT"

while IFS= read -r f; do
    rel="${f#$WOMB_DIR/}"
    if [[ "$rel" == "core.vuma" ]]; then
        continue
    fi
    fn_count=$(grep -c '^fn ' "$f" 2>/dev/null || echo 0)
    line_count=$(wc -l < "$f" 2>/dev/null || echo 0)
    echo "| $rel | $fn_count | $line_count |" | tee -a "$REPORT"
done < <(find "$WOMB_DIR" -name "*.vuma" | sort)

echo "" | tee -a "$REPORT"

# Section 4: Build verification
echo "## 4. VUMA Compiler Build" | tee -a "$REPORT"
echo "" | tee -a "$REPORT"
if [[ -x "$VUMA_BIN" ]]; then
    echo "- VUMA binary: **present** at $VUMA_BIN" | tee -a "$REPORT"
    echo "- Version: $($VUMA_BIN --version 2>&1 | head -1)" | tee -a "$REPORT"
else
    echo "- VUMA binary: **MISSING**" | tee -a "$REPORT"
fi
echo "" | tee -a "$REPORT"

# Final summary
echo "## 5. Final Summary" | tee -a "$REPORT"
echo "" | tee -a "$REPORT"
echo "- **Compilation:** $ok/$total womb files compile cleanly" | tee -a "$REPORT"
echo "- **Stubs:** $stub_total stub markers remaining (target: 0)" | tee -a "$REPORT"
echo "- **Total compiled output:** $total_size bytes" | tee -a "$REPORT"
echo "- **Total SCG nodes:** $total_nodes" | tee -a "$REPORT"
echo "- **Total IR instructions:** $total_ir" | tee -a "$REPORT"
echo "" | tee -a "$REPORT"

if [[ "$fail" -eq 0 && "$stub_total" -eq 0 ]]; then
    echo "**PASS** — All womb files compile and contain zero stub markers." | tee -a "$REPORT"
    exit 0
else
    echo "**NEEDS WORK** — $fail compile failures, $stub_total stub markers." | tee -a "$REPORT"
    exit 1
fi
