#!/usr/bin/env bash
# ============================================================================
# cross_backend_test.sh — VUMA cross-backend differential test runner
# ----------------------------------------------------------------------------
# Compiles every .vuma file in the given directory (recursively) on all 7
# native backends (x86_64, arm32, mips64, aarch64, riscv64, ppc64,
# loongarch64), runs each binary (native for x86_64, QEMU for the rest),
# captures exit code AND stdout, compares results across backends, and
# produces a detailed report with:
#   * Per-program results matrix (program x backend exit codes)
#   * Agreement statistics (overall + per-category)
#   * Disagreement details (per-backend exit code + stdout length/hash)
#   * Per-backend pass rates (vs. the 7-backend majority consensus)
#
# Output files (under --output-dir, default $VUMA_ROOT/test_results):
#   cross_backend_<TS>.txt     — human-readable report (always)
#   cross_backend_<TS>.json    — machine-readable JSON (--json only)
#   cross_backend_latest.{txt,json} — symlinks to newest
#
# Usage:
#   ./scripts/cross_backend_test.sh <directory> [--json] [--timeout N] \
#                                    [--output-dir DIR]
# Exit:
#   0 if every program agrees across all 7 backends
#   1 if any program disagrees (or any backend crashed/timed out/failed)
#   2 on usage / setup error
# ============================================================================
set -uo pipefail
export PATH="$HOME/.cargo/bin:$PATH"

VUMA_ROOT="${VUMA_ROOT:-/tmp/my-project}"
COMPILE_DUMP="$VUMA_ROOT/target/release/compile_dump"

DIR="${1:-examples}"
TIMEOUT=3
JSON=false
OUTPUT_DIR="$VUMA_ROOT/test_results"
PROGRESS_EVERY=20

# Move past the directory argument.
shift || true
while [[ $# -gt 0 ]]; do
    case "$1" in
        --json)        JSON=true;         shift ;;
        --timeout)     TIMEOUT="${2:-3}"; shift 2 ;;
        --output-dir)  OUTPUT_DIR="$2";   shift 2 ;;
        --progress)    PROGRESS_EVERY="$2"; shift 2 ;;
        -h|--help)
            sed -n '2,30p' "$0"
            exit 0 ;;
        *) shift ;;
    esac
done

# Resolve DIR relative to VUMA_ROOT if not absolute.
if [[ "$DIR" != /* ]]; then
    DIR="$VUMA_ROOT/$DIR"
fi

if [[ ! -d "$DIR" ]]; then
    echo "[cross-backend][FAIL] directory $DIR not found" >&2
    exit 2
fi

if [[ ! -x "$COMPILE_DUMP" ]]; then
    echo "[cross-backend][FAIL] $COMPILE_DUMP missing — build it first" >&2
    exit 2
fi

mkdir -p "$OUTPUT_DIR"
TIMESTAMP="$(date +%Y%m%d_%H%M%S)"
RESULT_FILE="$OUTPUT_DIR/cross_backend_${TIMESTAMP}.txt"
JSON_FILE="$OUTPUT_DIR/cross_backend_${TIMESTAMP}.json"

# -----------------------------------------------------------------------------
# Backend definitions. x86_64 is the reference column (always printed first).
# -----------------------------------------------------------------------------
declare -a BACKENDS=("x86_64" "arm32" "mips64" "aarch64" "riscv64" "ppc64" "loongarch64")
declare -A QEMU
QEMU[arm32]="/tmp/qemu_bins/qemu-arm"
QEMU[mips64]="/tmp/qemu_extracted/usr/bin/qemu-mips64el"
QEMU[aarch64]="/tmp/qemu_bins/qemu-aarch64"
QEMU[riscv64]="/tmp/qemu_bins/qemu-riscv64"
QEMU[ppc64]="/tmp/qemu_bins/qemu-ppc64"
QEMU[loongarch64]="/tmp/qemu_bins/qemu-loongarch64"

NB=${#BACKENDS[@]}

# Workspace for per-run artifacts (binaries + stdout captures).
WORK_DIR="$(mktemp -d -t vuma_cb.XXXXXX)"
trap 'rm -rf "$WORK_DIR"' EXIT

log()  { printf '[cross-backend][%s] %s\n' "$(date +%H:%M:%S)" "$*"; }
err()  { printf '[cross-backend][FAIL] %s\n' "$*" >&2; }

# -----------------------------------------------------------------------------
# Helper: classify a raw numeric exit code into a short symbolic label.
#   CF  = compile failure
#   TO  = timeout (124 from `timeout`, or 137 SIGKILL)
#   CR  = crash (SIGSEGV=139, SIGABRT=134, SIGILL=132, SIGBUS=133)
#   PE  = QEMU path/exec error
#   NN  = a normal exit code (0..255) — printed as the decimal value
# Already-classified labels (CF/TO/CR/PE) pass through unchanged.
# -----------------------------------------------------------------------------
classify_exit() {
    local code="$1"
    case "$code" in
        CF|TO|CR|PE) printf '%s' "$code" ;;
        124)         printf 'TO' ;;
        137)         printf 'TO' ;;
        139)         printf 'CR' ;;
        134)         printf 'CR' ;;
        132)         printf 'CR' ;;
        133)         printf 'CR' ;;
        *)           printf '%s' "$code" ;;
    esac
}

# -----------------------------------------------------------------------------
# Helper: minimal JSON string escape (handles ", \, newline, CR, tab; strips
# other control chars).
# -----------------------------------------------------------------------------
json_escape() {
    local s="$1"
    s="${s//\\/\\\\}"
    s="${s//\"/\\\"}"
    s="${s//$'\n'/\\n}"
    s="${s//$'\r'/\\r}"
    s="${s//$'\t'/\\t}"
    printf '%s' "$s" | tr -d '\000-\010\013\014\016-\037'
}

# Discover all .vuma files (recursive), sorted for deterministic ordering.
mapfile -t FILES < <(find "$DIR" -type f -name '*.vuma' | sort)
TOTAL=${#FILES[@]}

if [[ "$TOTAL" -eq 0 ]]; then
    err "no .vuma files found in $DIR"
    exit 2
fi

log "Discovered $TOTAL .vuma files in $DIR"
log "Backends: ${BACKENDS[*]}"
log "Per-run timeout: ${TIMEOUT}s"
log "Total runs: $((TOTAL * NB))"

# -----------------------------------------------------------------------------
# Per-program result storage (associative arrays keyed by "$idx,$backend").
#   EXITCODES[$i,$b]   = exit label (CF/TO/CR/PE or decimal)
#   STDOUT_HASH[$i,$b] = md5 of stdout (or special label)
#   STDOUT_LEN[$i,$b]  = stdout byte length
#   STDOUT_SNIP[$i,$b] = first line of stdout, truncated (for disagreement detail)
# -----------------------------------------------------------------------------
declare -A EXITCODES STDOUT_HASH STDOUT_LEN STDOUT_SNIP
declare -a PROG_NAMES PROG_PATHS PROG_CATEGORIES

# Per-backend counters.
declare -A BE_COMPILE_FAIL BE_TIMEOUT BE_CRASH BE_TOTAL BE_PASS
for b in "${BACKENDS[@]}"; do
    BE_COMPILE_FAIL[$b]=0
    BE_TIMEOUT[$b]=0
    BE_CRASH[$b]=0
    BE_TOTAL[$b]=0
    BE_PASS[$b]=0
done

# -----------------------------------------------------------------------------
# Main loop: compile + run each program on each backend.
# -----------------------------------------------------------------------------
PROG_DONE=0
for f in "${FILES[@]}"; do
    idx=$PROG_DONE
    PROG_PATHS[$idx]="$f"
    PROG_NAMES[$idx]="$(basename "$f" .vuma)"
    # Category: parent directory's basename (e.g. "examples", "arithmetic").
    PROG_CATEGORIES[$idx]="$(basename "$(dirname "$f")")"

    for b in "${BACKENDS[@]}"; do
        binpath="$WORK_DIR/p${idx}_${b}.bin"
        outpath="$WORK_DIR/p${idx}_${b}.out"
        : > "$outpath"

        # --- Compile ---
        "$COMPILE_DUMP" "$f" "$binpath" "$b" \
            > "$WORK_DIR/p${idx}_${b}.compile.log" 2>&1
        rc=$?
        if [[ $rc -ne 0 ]] || [[ ! -s "$binpath" ]]; then
            EXITCODES[$idx,$b]="CF"
            STDOUT_HASH[$idx,$b]="CF"
            STDOUT_LEN[$idx,$b]=0
            STDOUT_SNIP[$idx,$b]=""
            BE_COMPILE_FAIL[$b]=$(( ${BE_COMPILE_FAIL[$b]} + 1 ))
            BE_TOTAL[$b]=$(( ${BE_TOTAL[$b]} + 1 ))
            continue
        fi
        chmod +x "$binpath" 2>/dev/null

        # --- Run ---
        # `timeout -k 2` sends SIGTERM at $TIMEOUT, then SIGKILL 2s later
        # if the process still refuses to die (QEMU under load can ignore
        # SIGTERM). The surrounding `{ ... ; } 2>/dev/null` swallows the
        # bash "Segmentation fault" notification that the parent shell
        # otherwise prints when a child dies from a signal — that noise is
        # expected here (we capture the exit code ourselves via `$?`).
        if [[ "$b" == "x86_64" ]]; then
            { timeout -k 2 "$TIMEOUT" "$binpath" > "$outpath" 2>/dev/null; } 2>/dev/null
            code=$?
        else
            q="${QEMU[$b]}"
            if [[ ! -x "$q" ]]; then
                EXITCODES[$idx,$b]="PE"
                STDOUT_HASH[$idx,$b]="PE"
                STDOUT_LEN[$idx,$b]=0
                STDOUT_SNIP[$idx,$b]="(qemu missing: $q)"
                BE_TOTAL[$b]=$(( ${BE_TOTAL[$b]} + 1 ))
                continue
            fi
            { timeout -k 2 "$TIMEOUT" "$q" "$binpath" > "$outpath" 2>/dev/null; } 2>/dev/null
            code=$?
        fi

        label="$(classify_exit "$code")"
        EXITCODES[$idx,$b]="$label"
        STDOUT_LEN[$idx,$b]=$(stat -c%s "$outpath" 2>/dev/null || echo 0)
        STDOUT_HASH[$idx,$b]="$(md5sum "$outpath" 2>/dev/null | awk '{print $1}')"
        # Save a short first-line snippet of stdout for disagreement detail.
        STDOUT_SNIP[$idx,$b]="$(head -c 60 "$outpath" 2>/dev/null | tr -d '\n\r' | head -c 60)"

        case "$label" in
            TO) BE_TIMEOUT[$b]=$(( ${BE_TIMEOUT[$b]} + 1 )) ;;
            CR) BE_CRASH[$b]=$(( ${BE_CRASH[$b]} + 1 )) ;;
        esac
        BE_TOTAL[$b]=$(( ${BE_TOTAL[$b]} + 1 ))
    done

    PROG_DONE=$((PROG_DONE + 1))
    if (( PROGRESS_EVERY > 0 )) && (( PROG_DONE % PROGRESS_EVERY == 0 )); then
        log "Progress: $PROG_DONE / $TOTAL programs"
    fi
done
log "Completed $PROG_DONE / $TOTAL programs"

# -----------------------------------------------------------------------------
# Compute agreement per program.
# A program "agrees" iff all 7 backends produced identical (exit, stdout_hash).
# For each backend we track whether it matched the majority (exit|stdout_hash)
# for each program — that becomes the per-backend pass rate.
# -----------------------------------------------------------------------------
TOTAL_AGREE=0
TOTAL_DISAGREE=0
declare -A CAT_TOTAL CAT_AGREE
declare -a DISAGREE_IDXS

# Per-program majority-key cache (re-used at report time).
declare -a PROG_MAJORITY_KEY PROG_MAJORITY_COUNT

for ((i=0; i<PROG_DONE; i++)); do
    cat="${PROG_CATEGORIES[$i]}"
    CAT_TOTAL[$cat]=$(( ${CAT_TOTAL[$cat]:-0} + 1 ))

    # Tally (exit|stdout_hash) keys across the 7 backends.
    declare -A ec=()
    for b in "${BACKENDS[@]}"; do
        e="${EXITCODES[$i,$b]}"
        h="${STDOUT_HASH[$i,$b]}"
        key="${e}|${h}"
        ec["$key"]=$(( ${ec["$key"]:-0} + 1 ))
    done

    # Majority = key with the highest count (ties broken by lex order).
    max_count=0
    majority_key=""
    for key in "${!ec[@]}"; do
        c="${ec[$key]}"
        if (( c > max_count )); then
            max_count=$c
            majority_key="$key"
        fi
    done
    PROG_MAJORITY_KEY[$i]="$majority_key"
    PROG_MAJORITY_COUNT[$i]=$max_count

    # Per-backend pass: did this backend produce the majority key?
    for b in "${BACKENDS[@]}"; do
        e="${EXITCODES[$i,$b]}"
        h="${STDOUT_HASH[$i,$b]}"
        key="${e}|${h}"
        if [[ "$key" == "$majority_key" ]]; then
            BE_PASS[$b]=$(( ${BE_PASS[$b]} + 1 ))
        fi
    done

    # Program agrees iff all NB backends share the same key.
    if (( max_count == NB )); then
        TOTAL_AGREE=$((TOTAL_AGREE + 1))
        CAT_AGREE[$cat]=$(( ${CAT_AGREE[$cat]:-0} + 1 ))
    else
        TOTAL_DISAGREE=$((TOTAL_DISAGREE + 1))
        DISAGREE_IDXS+=($i)
    fi
    unset ec
done

pct() { awk -v a="$1" -v t="$2" 'BEGIN{ if(t==0){print "0.00%"}else{printf "%.2f%%", 100*a/t} }'; }

# -----------------------------------------------------------------------------
# Build the human-readable report. We assemble it in a temp file so we can
# both `tee` to stdout and write the saved copy in one pass.
# -----------------------------------------------------------------------------
REPORT_TMP="$WORK_DIR/report.txt"
{
echo "================================================================================"
echo "VUMA Cross-Backend Test Report"
echo "================================================================================"
echo "Date       : $(date)"
echo "Directory  : $DIR"
echo "Programs   : $TOTAL"
echo "Backends   : ${BACKENDS[*]} (${NB} total)"
echo "Timeout    : ${TIMEOUT}s"
echo "Total runs : $((TOTAL * NB))"
echo "================================================================================"

echo ""
echo "=== PER-PROGRAM RESULTS MATRIX ==="
echo "Legend: exit code | CF=compile_fail  TO=timeout  CR=crash  PE=qemu_missing"
echo "        '*' after the code marks a backend that disagreed with the majority"
echo "        'Agree?' = YES iff all ${NB} backends share the same (exit, stdout)"
echo ""
# Header row.
printf '%-30s' "Program"
for b in "${BACKENDS[@]}"; do
    printf ' %11s' "${b:0:10}"
done
printf ' %7s\n' "Agree?"
# Separator.
sep=""
for ((n=0; n<30; n++)); do sep="${sep}-"; done
for ((n=0; n<NB; n++)); do sep="${sep}------------"; done
sep="${sep} -------"
printf '%s\n' "$sep"

for ((i=0; i<PROG_DONE; i++)); do
    name="${PROG_NAMES[$i]}"
    [[ ${#name} -gt 30 ]] && name="${name:0:27}..."
    printf '%-30s' "$name"
    majority_key="${PROG_MAJORITY_KEY[$i]}"
    for b in "${BACKENDS[@]}"; do
        e="${EXITCODES[$i,$b]}"
        h="${STDOUT_HASH[$i,$b]}"
        key="${e}|${h}"
        if [[ "$key" == "$majority_key" ]]; then
            marker=" "
        else
            marker="*"
        fi
        printf ' %10s%s' "$e" "$marker"
    done
    if [[ "${PROG_MAJORITY_COUNT[$i]}" -eq $NB ]]; then
        printf ' %7s\n' "YES"
    else
        printf ' %7s\n' "NO"
    fi
done
echo ""

echo "=== DISAGREEMENT DETAILS ==="
if [[ ${#DISAGREE_IDXS[@]} -eq 0 ]]; then
    echo "No disagreements — all $TOTAL programs agree across all $NB backends."
else
    echo "Disagreements: $TOTAL_DISAGREE program(s)"
    for i in "${DISAGREE_IDXS[@]}"; do
        path="${PROG_PATHS[$i]}"
        cat="${PROG_CATEGORIES[$i]}"
        majority_key="${PROG_MAJORITY_KEY[$i]}"
        majority_count="${PROG_MAJORITY_COUNT[$i]}"
        echo ""
        echo "--- [$cat] $(basename "$path")  ($path) ---"
        echo "    Majority: ${majority_count}/${NB} backends -> ${majority_key}"
        printf '    %-14s %10s %10s %10s  %s\n' \
            "Backend" "Exit" "StdoutLen" "InMaj?" "StdoutSnippet"
        for b in "${BACKENDS[@]}"; do
            e="${EXITCODES[$i,$b]}"
            h="${STDOUT_HASH[$i,$b]}"
            slen="${STDOUT_LEN[$i,$b]}"
            snip="${STDOUT_SNIP[$i,$b]}"
            key="${e}|${h}"
            if [[ "$key" == "$majority_key" ]]; then
                m="yes"
            else
                m="NO"
            fi
            printf '    %-14s %10s %10s %10s  "%s"\n' \
                "$b" "$e" "$slen" "$m" "$snip"
        done
    done
fi
echo ""

echo "=== PER-BACKEND PASS RATE (vs majority) ==="
printf '%-14s %8s %8s %8s %8s %8s %10s\n' \
    "Backend" "Total" "Pass" "CFail" "Timeout" "Crash" "Pass%"
for b in "${BACKENDS[@]}"; do
    tot=${BE_TOTAL[$b]}
    pass=${BE_PASS[$b]}
    cf=${BE_COMPILE_FAIL[$b]}
    to=${BE_TIMEOUT[$b]}
    cr=${BE_CRASH[$b]}
    pctv="$(pct "$pass" "$tot")"
    printf '%-14s %8d %8d %8d %8d %8d %10s\n' \
        "$b" "$tot" "$pass" "$cf" "$to" "$cr" "$pctv"
done
echo ""

echo "=== PER-CATEGORY AGREEMENT ==="
printf '%-22s %8s %8s %10s\n' "Category" "Total" "Agree" "Agree%"
mapfile -t CATS < <(printf '%s\n' "${!CAT_TOTAL[@]}" | sort)
for c in "${CATS[@]}"; do
    t=${CAT_TOTAL[$c]}
    a=${CAT_AGREE[$c]:-0}
    pctv="$(pct "$a" "$t")"
    printf '%-22s %8d %8d %10s\n' "$c" "$t" "$a" "$pctv"
done
echo ""

echo "=== OVERALL SUMMARY ==="
echo "Programs tested     : $TOTAL"
echo "Total runs          : $((TOTAL * NB))"
echo "All-backends-agree  : $TOTAL_AGREE / $TOTAL  ($(pct "$TOTAL_AGREE" "$TOTAL"))"
echo "Disagreements       : $TOTAL_DISAGREE"
echo ""
echo "Report file : $RESULT_FILE"
if $JSON; then
    echo "JSON file   : $JSON_FILE"
fi
echo "================================================================================"
} > "$REPORT_TMP"

# Print to stdout AND save to $RESULT_FILE in one pass.
cat "$REPORT_TMP" | tee "$RESULT_FILE"

# Refresh "latest" symlinks.
ln -sf "cross_backend_${TIMESTAMP}.txt"  "$OUTPUT_DIR/cross_backend_latest.txt"

# -----------------------------------------------------------------------------
# Optional JSON output for CI integration.
# -----------------------------------------------------------------------------
if $JSON; then
    {
    printf '{'
    printf '"metadata":{"directory":%s,"timeout":%d,"timestamp":%s,"total_programs":%d,"backends":[' \
        "\"$(json_escape "$DIR")\"" "$TIMEOUT" "\"$(json_escape "$TIMESTAMP")\"" "$TOTAL"
    for ((k=0; k<NB; k++)); do
        b="${BACKENDS[$k]}"
        printf '%s"%s"' "$([[ $k -gt 0 ]] && echo ',')" "$(json_escape "$b")"
    done
    printf '],"total_runs":%d},' $((TOTAL * NB))

    printf '"summary":{"all_agree":%d,"disagree":%d,"agreement_rate":%s,' \
        "$TOTAL_AGREE" "$TOTAL_DISAGREE" \
        "$(awk -v a=$TOTAL_AGREE -v t=$TOTAL 'BEGIN{printf "%.4f", (t==0?0:a/t)}')"

    printf '"per_backend":{'
    for ((k=0; k<NB; k++)); do
        b="${BACKENDS[$k]}"
        tot=${BE_TOTAL[$b]}
        pass=${BE_PASS[$b]}
        cf=${BE_COMPILE_FAIL[$b]}
        to=${BE_TIMEOUT[$b]}
        cr=${BE_CRASH[$b]}
        printf '%s"%s":{"total":%d,"pass":%d,"compile_fail":%d,"timeout":%d,"crash":%d,"pass_rate":%s}' \
            "$([[ $k -gt 0 ]] && echo ',')" \
            "$(json_escape "$b")" \
            "$tot" "$pass" "$cf" "$to" "$cr" \
            "$(awk -v p=$pass -v t=$tot 'BEGIN{printf "%.4f", (t==0?0:p/t)}')"
    done
    printf '}},'

    printf '"per_category":{'
    first=1
    for c in "${CATS[@]}"; do
        t=${CAT_TOTAL[$c]}
        a=${CAT_AGREE[$c]:-0}
        printf '%s"%s":{"total":%d,"agree":%d,"agree_rate":%s}' \
            "$([[ $first -eq 0 ]] && echo ',')" \
            "$(json_escape "$c")" \
            "$t" "$a" \
            "$(awk -v a=$a -v t=$t 'BEGIN{printf "%.4f", (t==0?0:a/t)}')"
        first=0
    done
    printf '},'

    printf '"programs":['
    for ((i=0; i<PROG_DONE; i++)); do
        path="${PROG_PATHS[$i]}"
        name="${PROG_NAMES[$i]}"
        cat="${PROG_CATEGORIES[$i]}"
        mk="${PROG_MAJORITY_KEY[$i]}"
        mc="${PROG_MAJORITY_COUNT[$i]}"
        if [[ "$mc" -eq $NB ]]; then agree="true"; else agree="false"; fi
        printf '%s{' "$([[ $i -gt 0 ]] && echo ',')"
        printf '"file":%s,"name":%s,"category":%s,"majority_count":%d,"all_agree":%s,' \
            "\"$(json_escape "$path")\"" \
            "\"$(json_escape "$name")\"" \
            "\"$(json_escape "$cat")\"" \
            "$mc" "$agree"
        printf '"results":{'
        for ((k=0; k<NB; k++)); do
            b="${BACKENDS[$k]}"
            e="${EXITCODES[$i,$b]}"
            h="${STDOUT_HASH[$i,$b]}"
            slen="${STDOUT_LEN[$i,$b]}"
            snip="${STDOUT_SNIP[$i,$b]}"
            in_maj="false"
            if [[ "${e}|${h}" == "$mk" ]]; then in_maj="true"; fi
            printf '%s"%s":{"exit":%s,"stdout_len":%d,"stdout_hash":%s,"stdout_snippet":%s,"in_majority":%s}' \
                "$([[ $k -gt 0 ]] && echo ',')" \
                "$(json_escape "$b")" \
                "\"$(json_escape "$e")\"" \
                "$slen" \
                "\"$(json_escape "$h")\"" \
                "\"$(json_escape "$snip")\"" \
                "$in_maj"
        done
        printf '}}'
    done
    printf ']}\n'
    } > "$JSON_FILE"
    ln -sf "cross_backend_${TIMESTAMP}.json" "$OUTPUT_DIR/cross_backend_latest.json"
    log "JSON report written to $JSON_FILE"
fi

# -----------------------------------------------------------------------------
# Final exit code: 0 only if every program agreed across all backends.
# -----------------------------------------------------------------------------
log "Done. agree=$TOTAL_AGREE disagree=$TOTAL_DISAGREE (of $TOTAL programs)"

# Surface headline counts inline for CI log readability.
echo ""
echo "=== Cross-backend test complete ==="
echo "  programs      : $TOTAL"
echo "  all-agree     : $TOTAL_AGREE"
echo "  disagreements : $TOTAL_DISAGREE"
echo "  report        : $RESULT_FILE"
if $JSON; then
    echo "  json          : $JSON_FILE"
fi

if [[ "$TOTAL_DISAGREE" -gt 0 ]]; then
    err "$TOTAL_DISAGREE program(s) disagree across backends"
    exit 1
fi
log "All $TOTAL programs agree across all $NB backends"
exit 0
