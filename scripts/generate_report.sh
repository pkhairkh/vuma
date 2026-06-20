#!/usr/bin/env bash
# ============================================================================
# generate_report.sh — VUMA consolidated markdown test report
# ----------------------------------------------------------------------------
# Reads the structured .summary and .txt files written by ci_run_tests.sh,
# run_differential.sh, and run_fuzz.sh, and emits a single markdown report
# at $TEST_OUT/REPORT.md.
#
# The report has the following sections:
#   1. Headline pass/fail table (one row per test category).
#   2. Per-backend pass rates (compile_dump_examples.summary).
#   3. Per-category pass rates on x86_64 gold_standard.
#   4. Differential agreement (examples + full gold_standard).
#   5. O0 vs O3 optimizer soundness.
#   6. Fuzz outcome.
#   7. Failure listing (excerpted from the raw .txt transcripts).
#
# Usage: ./scripts/generate_report.sh
# Environment: VUMA_ROOT, TEST_RESULTS_DIR
# Exit: 0 if report written, 2 if no results found.
# ============================================================================
set -uo pipefail

VUMA_ROOT="${VUMA_ROOT:-/tmp/my-project}"
TEST_OUT="${TEST_RESULTS_DIR:-$VUMA_ROOT/test_results}"
REPORT="$TEST_OUT/REPORT.md"

log()  { printf '[report][%s] %s\n' "$(date +%H:%M:%S)" "$*"; }

if [[ ! -d "$TEST_OUT" ]]; then
    echo "[report] no test_results dir at $TEST_OUT — nothing to report" >&2
    exit 2
fi
nfiles=$(find "$TEST_OUT" -maxdepth 1 -type f \( -name '*.summary' -o -name 'SUMMARY.txt' \) 2>/dev/null | wc -l)
if [[ "$nfiles" -eq 0 ]]; then
    echo "[report] no .summary files found in $TEST_OUT — run ci_run_tests.sh first" >&2
    exit 2
fi

# Extract KEY=VAL from a summary line, tolerating parenthesis-wrapped value
# groups (e.g. "(compile=0 crash=0 timeout=0 compilefail=0)"). Strips
# leading "(" and trailing ")" from each field, then strips any trailing
# non-digit characters from the extracted value.
# Usage: extract_val <line> <key>
extract_val() {
    # Returns the integer value of KEY from a "key=val" field. Strips
    # parenthesis-wrappers and any trailing non-digit characters.
    printf '%s\n' "$1" | awk -v k="$2" '{
        for (i = 1; i <= NF; i++) {
            gsub(/^\(/, "", $i); gsub(/\)$/, "", $i)
            if ($i ~ "^" k "=") {
                sub("^" k "=", "", $i)
                gsub(/[^0-9-].*/, "", $i)
                print $i
                exit
            }
        }
    }'
}

extract_raw() {
    # Returns the raw value of KEY from a "key=val" field (no digit
    # stripping). Use for string-valued fields like backend=x86_64.
    printf '%s\n' "$1" | awk -v k="$2" '{
        for (i = 1; i <= NF; i++) {
            gsub(/^\(/, "", $i); gsub(/\)$/, "", $i)
            if ($i ~ "^" k "=") {
                sub("^" k "=", "", $i)
                print $i
                exit
            }
        }
    }'
}

# Helper: pull the value of KEY=VAL out of a summary file.
# getval <file> <key>
getval() {
    [[ -f "$1" ]] || { echo "0"; return; }
    awk -F'=' -v k="$2" '$1==k{print $2; found=1} END{}' "$1" | tail -1
}

# --- Parse compile_dump_examples.summary into per-backend rows ---
emit_backend_table() {
    local f="$TEST_OUT/compile_dump_examples.summary"
    if [[ ! -f "$f" ]]; then return; fi
    echo "### Per-backend pass rate (47 examples)"
    echo ""
    echo "| Backend | Total | Pass | Fail | Compile fail | Crash | Timeout | Exec fail |"
    echo "|---------|------:|----:|----:|-------------:|------:|--------:|----------:|"
    while IFS= read -r line; do
        # line looks like: backend=x86_64 total=47 pass=45 fail=2 (compile=1 crash=0 timeout=1 execfail=0)
        be=$(extract_raw "$line" "backend")
        tot=$(extract_val "$line" "total")
        pas=$(extract_val "$line" "pass")
        fal=$(extract_val "$line" "fail")
        cf=$(extract_val "$line" "compile")
        cr=$(extract_val "$line" "crash")
        tm=$(extract_val "$line" "timeout")
        ef=$(extract_val "$line" "execfail")
        [[ -z "$be" ]] && continue
        echo "| $be | ${tot:-0} | ${pas:-0} | ${fal:-0} | ${cf:-0} | ${cr:-0} | ${tm:-0} | ${ef:-0} |"
    done < "$f"
    echo ""
}

emit_gold_table() {
    local f="$TEST_OUT/gold_standard_x86_64.summary"
    if [[ ! -f "$f" ]]; then return; fi
    echo "### Per-category pass rate on x86_64 gold_standard"
    echo ""
    echo "| Category | Programs | Pass | Fail | Compile fail | Crash | Timeout | Exec fail |"
    echo "|----------|---------:|----:|----:|-------------:|------:|--------:|----------:|"
    while IFS= read -r line; do
        # Skip the gold_standard_total aggregator line.
        [[ "$line" =~ ^gold_standard_total ]] && continue
        cat=$(extract_raw "$line" "category")
        progs=$(extract_val "$line" "programs")
        pas=$(extract_val "$line" "pass")
        fal=$(extract_val "$line" "fail")
        cf=$(extract_val "$line" "compile")
        cr=$(extract_val "$line" "crash")
        tm=$(extract_val "$line" "timeout")
        ef=$(extract_val "$line" "execfail")
        [[ -z "$cat" ]] && continue
        echo "| $cat | ${progs:-0} | ${pas:-0} | ${fal:-0} | ${cf:-0} | ${cr:-0} | ${tm:-0} | ${ef:-0} |"
    done < "$f"
    # Append the grand total row if present.
    if grep -q '^gold_standard_total' "$f"; then
        total_line=$(grep '^gold_standard_total' "$f" | tail -1)
        pas=$(extract_val "$total_line" "pass")
        fal=$(extract_val "$total_line" "fail")
        echo "| **TOTAL** | — | **${pas:-0}** | **${fal:-0}** | — | — | — | — |"
    fi
    echo ""
}

emit_differential_table() {
    local f1="$TEST_OUT/differential_examples.summary"
    local f2="$TEST_OUT/differential_full_latest.summary"
    echo "### Cross-backend differential agreement"
    echo ""
    echo "| Suite | Programs | Pass (all 7 agree) | Diff failures | Crashes | Timeouts | Compile fails |"
    echo "|-------|---------:|-------------------:|--------------:|--------:|---------:|--------------:|"
    if [[ -f "$f1" ]]; then
        while IFS= read -r line; do
            [[ "$line" =~ ^differential_examples ]] || continue
            pas=$(extract_val "$line" "pass")
            dif=$(extract_val "$line" "diff")
            cr=$(extract_val "$line" "crash")
            tm=$(extract_val "$line" "timeout")
            cf=$(extract_val "$line" "compilefail")
            echo "| examples (47) | 47 | ${pas:-0} | ${dif:-0} | ${cr:-0} | ${tm:-0} | ${cf:-0} |"
        done < "$f1"
    fi
    if [[ -f "$f2" ]]; then
        # The differential_full_*.summary file has two lines:
        #   differential_full timestamp=... programs=N backends=7 total_runs=R
        #   differential_full pass=P fail=F (diff=D crash=C timeout=T compilefail=CF) tool_exit=E
        # Pull programs= from the metadata line and pass/fail/etc. from the data line.
        meta_line=$(grep '^differential_full timestamp=' "$f2" | tail -1)
        data_line=$(grep '^differential_full pass='       "$f2" | tail -1)
        if [[ -n "$data_line" ]]; then
            progs=$(extract_val "$meta_line" "programs")
            pas=$(extract_val  "$data_line" "pass")
            dif=$(extract_val  "$data_line" "diff")
            cr=$(extract_val   "$data_line" "crash")
            tm=$(extract_val   "$data_line" "timeout")
            cf=$(extract_val   "$data_line" "compilefail")
            echo "| full gold_standard+examples | ${progs:-0} | ${pas:-0} | ${dif:-0} | ${cr:-0} | ${tm:-0} | ${cf:-0} |"
        fi
    fi
    echo ""
}

emit_opt_table() {
    local f="$TEST_OUT/opt_level.summary"
    if [[ ! -f "$f" ]]; then return; fi
    echo "### O0 vs O3 optimizer soundness (x86_64)"
    echo ""
    echo "| Pass | Miscompilation | CrashAsym | TimeoutAsym | CF-O0 | CF-O3 | CF-Both |"
    echo "|----:|---------------:|----------:|------------:|------:|------:|--------:|"
    while IFS= read -r line; do
        pas=$(extract_val "$line" "pass")
        mc=$(extract_val "$line" "miscomp")
        ca=$(extract_val "$line" "crash_asym")
        ta=$(extract_val "$line" "timeout_asym")
        c0=$(extract_val "$line" "cf_o0")
        c3=$(extract_val "$line" "cf_o3")
        cb=$(extract_val "$line" "cf_both")
        echo "| ${pas:-0} | ${mc:-0} | ${ca:-0} | ${ta:-0} | ${c0:-0} | ${c3:-0} | ${cb:-0} |"
    done < "$f"
    echo ""
}

emit_fuzz_table() {
    local f="$TEST_OUT/fuzz_latest.summary"
    if [[ ! -f "$f" ]]; then
        # Fall back to any fuzz_*.summary present.
        f=$(ls -1t "$TEST_OUT"/fuzz_*.summary 2>/dev/null | grep -v '_latest' | head -1)
        [[ -n "$f" ]] || return
    fi
    echo "### Fuzz outcome"
    echo ""
    echo "| Programs | Seed | Total runs | Pass | Diff failures | Crashes | Timeouts | Compile fails |"
    echo "|---------:|-----:|-----------:|----:|--------------:|--------:|---------:|--------------:|"
    # The summary file has two lines:
    #   fuzz timestamp=... count=N seed=S total_runs=R
    #   fuzz pass=P fail=F (compile=C crash=H timeout=T diff=D) tool_exit=E
    # We extract from each and emit one row.
    local line1 line2
    line1=$(grep '^fuzz timestamp=' "$f" | tail -1)
    line2=$(grep '^fuzz pass='       "$f" | tail -1)
    if [[ -z "$line2" ]]; then
        # Legacy one-line format (no timestamp/seed line).
        line2=$(grep '^fuzz ' "$f" | tail -1)
    fi
    cnt=$(extract_val "$line1"  "count")
    seed=$(extract_val "$line1" "seed")
    runs=$(extract_val "$line1" "total_runs")
    pas=$(extract_val "$line2"  "pass")
    dif=$(extract_val "$line2"  "diff")
    cr=$(extract_val  "$line2"  "crash")
    tm=$(extract_val  "$line2"  "timeout")
    cf=$(extract_val  "$line2"  "compile")
    echo "| ${cnt:-0} | ${seed:-0} | ${runs:-0} | ${pas:-0} | ${dif:-0} | ${cr:-0} | ${tm:-0} | ${cf:-0} |"
    echo ""
}

emit_failure_listings() {
    echo "### Failure listings (excerpted)"
    echo ""
    # compile_dump per-backend failures
    local cf="$TEST_OUT/compile_dump_examples.txt"
    if [[ -f "$cf" ]]; then
        echo "#### compile_dump — per-backend failures (examples/)"
        echo ""
        echo '```'
        # Print the lines under "Compile failures (" and "Crashes (" and "Timeouts (" headers,
        # keeping per-backend sections visible.
        awk '
            /^=== Backend: / { print "\n" $0 }
            /^Compile failures \(/ || /^Crashes \(/ || /^Timeouts \(/ || /^Exec fail \(/ { hdr=$0; getline_count=0 }
            /^  [XCT]/ { print "  " $0 }
        ' "$cf" | head -120
        echo '```'
        echo ""
    fi
    # Differential disagreements
    local dt="$TEST_OUT/differential_full_latest.txt"
    [[ -L "$dt" || -f "$dt" ]] || dt="$TEST_OUT/differential_examples.txt"
    if [[ -f "$dt" ]]; then
        echo "#### Differential disagreements"
        echo ""
        echo '```'
        awk '
            /^--- DIFFERENTIAL FAILURES \(/ { in_block=1; print; next }
            /^--- CRASHES \(|^--- TIMEOUTS \(|^--- COMPILE FAIL|^--- PASS \(|^=== / { in_block=0 }
            in_block { print }
        ' "$dt" | head -100
        echo '```'
        echo ""
    fi
    # Fuzz failures
    local fz="$TEST_OUT/fuzz_latest.txt"
    if [[ -f "$fz" ]]; then
        echo "#### Fuzz failures"
        echo ""
        echo '```'
        awk '
            /^--- Differential failures \(/ { in_block=1; print; next }
            /^--- Crashes \(|^=== / { in_block=0 }
            in_block { print }
        ' "$fz" | head -60
        awk '
            /^--- Crashes \(/ { in_block=1; print; next }
            /^=== / { in_block=0 }
            in_block { print }
        ' "$fz" | head -60
        echo '```'
        echo ""
    fi
}

# -----------------------------------------------------------------------------
# Render the report.
# -----------------------------------------------------------------------------
{
    echo "# VUMA Compiler Test Report"
    echo ""
    echo "Generated: $(date -u +%Y-%m-%dT%H:%M:%SZ)  •  Runner: $(hostname)  •  Root: \`$VUMA_ROOT\`"
    echo ""

    # --- Headline table ---
    echo "## Headline"
    echo ""
    echo "| Category | Pass | Fail |"
    echo "|----------|----:|----:|"

    # compile_dump examples per-backend totals (sum across backends).
    exf="$TEST_OUT/compile_dump_examples.summary"
    if [[ -f "$exf" ]]; then
        ex_pas=$(awk '{for(i=1;i<=NF;i++) if($i ~ /^pass=/){sub(/^pass=/,"",$i); s+=$i}} END{print s+0}' "$exf")
        ex_fal=$(awk '{for(i=1;i<=NF;i++) if($i ~ /^fail=/){sub(/^fail=/,"",$i); s+=$i}} END{print s+0}' "$exf")
        echo "| compile_dump (examples × 7 backends) | $ex_pas | $ex_fal |"
    fi
    # gold standard.
    gsf="$TEST_OUT/gold_standard_x86_64.summary"
    if [[ -f "$gsf" ]] && grep -q '^gold_standard_total' "$gsf"; then
        gs_pas=$(grep '^gold_standard_total' "$gsf" | sed -n 's/.*pass=\([0-9]*\).*/\1/p')
        gs_fal=$(grep '^gold_standard_total' "$gsf" | sed -n 's/.*fail=\([0-9]*\).*/\1/p')
        echo "| gold_standard on x86_64 | ${gs_pas:-0} | ${gs_fal:-0} |"
    fi
    # differential examples.
    dtf="$TEST_OUT/differential_examples.summary"
    if [[ -f "$dtf" ]]; then
        dt_pas=$(awk '{for(i=1;i<=NF;i++) if($i ~ /^pass=/){sub(/^pass=/,"",$i); print $i; exit}}' "$dtf")
        dt_fal=$(awk '{for(i=1;i<=NF;i++) if($i ~ /^fail=/){sub(/^fail=/,"",$i); print $i; exit}}' "$dtf")
        echo "| differential (examples) | ${dt_pas:-0} | ${dt_fal:-0} |"
    fi
    # differential full.
    dff="$TEST_OUT/differential_full_latest.summary"
    if [[ -f "$dff" ]]; then
        df_pas=$(awk '{for(i=1;i<=NF;i++) if($i ~ /^pass=/){sub(/^pass=/,"",$i); print $i; exit}}' "$dff")
        df_fal=$(awk '{for(i=1;i<=NF;i++) if($i ~ /^fail=/){sub(/^fail=/,"",$i); print $i; exit}}' "$dff")
        echo "| differential (full gold_standard+examples) | ${df_pas:-0} | ${df_fal:-0} |"
    fi
    # opt_level.
    optf="$TEST_OUT/opt_level.summary"
    if [[ -f "$optf" ]]; then
        op_pas=$(awk '{for(i=1;i<=NF;i++) if($i ~ /^pass=/){sub(/^pass=/,"",$i); print $i; exit}}' "$optf")
        op_fal=$(awk '{for(i=1;i<=NF;i++) if($i ~ /^fail=/){sub(/^fail=/,"",$i); print $i; exit}}' "$optf")
        echo "| O0 vs O3 (x86_64) | ${op_pas:-0} | ${op_fal:-0} |"
    fi
    # fuzz.
    fzf="$TEST_OUT/fuzz_latest.summary"
    if [[ -f "$fzf" ]]; then
        fz_pas=$(awk '{for(i=1;i<=NF;i++) if($i ~ /^pass=/){sub(/^pass=/,"",$i); print $i; exit}}' "$fzf")
        fz_fal=$(awk '{for(i=1;i<=NF;i++) if($i ~ /^fail=/){sub(/^fail=/,"",$i); print $i; exit}}' "$fzf")
        echo "| fuzz (latest run) | ${fz_pas:-0} | ${fz_fal:-0} |"
    fi
    echo ""

    echo "## Detailed tables"
    echo ""
    emit_backend_table
    emit_gold_table
    emit_differential_table
    emit_opt_table
    emit_fuzz_table

    echo "## Failures"
    echo ""
    emit_failure_listings

    echo "## Artifacts"
    echo ""
    echo "Raw transcripts and one-line summaries for every category live under"
    echo "\`test_results/\`. Files of note:"
    echo ""
    echo "- \`SUMMARY.txt\` — top-level pass/fail tally across all categories"
    echo "- \`compile_dump_examples.{txt,summary}\` — 47 examples × 7 backends"
    echo "- \`gold_standard_x86_64.{txt,summary}\` — gold_standard suite on x86_64"
    echo "- \`differential_examples.{txt,summary}\` — 47 examples × 7 backends"
    echo "- \`differential_full_*.{txt,summary}\` — full gold_standard+examples × 7 backends"
    echo "- \`opt_level.{txt,summary}\` — O0 vs O3 on x86_64"
    echo "- \`fuzz_*.{txt,summary}\` — fuzz_driver runs"
    echo "- \`build.log\` — cargo build log"
} > "$REPORT"

log "Wrote $REPORT ($(wc -l < "$REPORT") lines)"
echo "$REPORT"
