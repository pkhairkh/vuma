#!/usr/bin/env bash
# ============================================================================
# run_differential.sh — full cross-backend differential tester for VUMA
# ----------------------------------------------------------------------------
# Runs every .vuma program in the gold_standard suite (all categories) PLUS
# the 47 examples/ on all 7 native backends
#   (x86_64, aarch64, riscv64, arm32, mips64el, ppc64, loongarch64),
# compares exit codes AND stdout across backends, and reports any
# disagreements.
#
# The underlying `differential_test` Rust binary takes a single examples
# directory, so we stage a flat temp dir of symlinks pointing at every
# discovered .vuma file (gold_standard categories first, then examples/),
# prefixing each symlink with a category tag so failures are attributable.
#
# Output:
#   test_results/differential_full_<timestamp>.txt   — raw transcript
#   test_results/differential_full_<timestamp>.summary — one-line summary
#   test_results/differential_full_latest.{txt,summary} — symlink to newest
#
# Environment overrides:
#   VUMA_ROOT, TEST_RESULTS_DIR, GOLD_DIR, EXAMPLES_DIR
#   INCLUDE_EXAMPLES  (default: 1) — set to 0 to skip examples/
# Exit code: 0 if all programs agreed, 1 if any disagreement / crash / timeout,
#            2 if staging failed.
# ============================================================================
set -uo pipefail
export PATH="$HOME/.cargo/bin:$PATH"

VUMA_ROOT="${VUMA_ROOT:-/tmp/my-project}"
TEST_OUT="${TEST_RESULTS_DIR:-$VUMA_ROOT/test_results}"
GOLD_DIR="${GOLD_DIR:-$VUMA_ROOT/tests/gold_standard}"
EXAMPLES_DIR="${EXAMPLES_DIR:-$VUMA_ROOT/examples}"
INCLUDE_EXAMPLES="${INCLUDE_EXAMPLES:-1}"

mkdir -p "$TEST_OUT"
BIN="$VUMA_ROOT/target/release"
TS="$(date -u +%Y%m%dT%H%M%SZ)"
OUT_RAW="$TEST_OUT/differential_full_${TS}.txt"
OUT_SUM="$TEST_OUT/differential_full_${TS}.summary"

log()  { printf '[difftest][%s] %s\n' "$(date +%H:%M:%S)" "$*"; }
err()  { printf '[difftest][FAIL] %s\n' "$*" >&2; }

if [[ ! -x "$BIN/differential_test" ]]; then
    err "missing $BIN/differential_test — run ci_run_tests.sh first or build it"
    exit 2
fi

# -----------------------------------------------------------------------------
# Stage flat temp dir of symlinks to every .vuma file we want to test.
# -----------------------------------------------------------------------------
STAGE="$(mktemp -d -t vuma_diffstage.XXXXXX)"
trap 'rm -rf "$STAGE"' EXIT
log "Staging programs under $STAGE"

count=0
if [[ -d "$GOLD_DIR" ]]; then
    for catdir in $(ls -d "$GOLD_DIR"/*/ 2>/dev/null | sed 's#//*$##' | sort); do
        catname="$(basename "$catdir")"
        for f in "$catdir"/*.vuma; do
            [[ -e "$f" ]] || continue
            base="$(basename "$f" .vuma)"
            # Disambiguate names that occur in multiple categories (e.g.
            # `struct_demo.vuma` exists in both examples/ and structs/).
            ln -sf "$f" "$STAGE/gold_${catname}__${base}.vuma"
            count=$((count + 1))
        done
    done
fi
if [[ "$INCLUDE_EXAMPLES" == "1" && -d "$EXAMPLES_DIR" ]]; then
    for f in "$EXAMPLES_DIR"/*.vuma; do
        [[ -e "$f" ]] || continue
        base="$(basename "$f" .vuma)"
        ln -sf "$f" "$STAGE/ex_${base}.vuma"
        count=$((count + 1))
    done
fi

if [[ "$count" -eq 0 ]]; then
    err "no .vuma programs found (GOLD_DIR=$GOLD_DIR, EXAMPLES_DIR=$EXAMPLES_DIR)"
    exit 2
fi
log "Staged $count programs"

# -----------------------------------------------------------------------------
# Ensure /tmp/qemu_bins/qemu-<arch> symlinks exist (differential_test.rs
# hard-codes those paths).
# -----------------------------------------------------------------------------
mkdir -p /tmp/qemu_bins
for suffix in aarch64 riscv64 arm mips64el ppc64 loongarch64; do
    if [[ ! -e "/tmp/qemu_bins/qemu-$suffix" ]]; then
        for cand in "/tmp/qemu_extracted/usr/bin/qemu-$suffix" "/usr/bin/qemu-$suffix" "/usr/local/bin/qemu-$suffix"; do
            if [[ -x "$cand" ]]; then ln -sf "$cand" "/tmp/qemu_bins/qemu-$suffix"; break; fi
        done
        if [[ ! -e "/tmp/qemu_bins/qemu-$suffix" ]] && command -v "qemu-$suffix" >/dev/null 2>&1; then
            ln -sf "$(command -v "qemu-$suffix")" "/tmp/qemu_bins/qemu-$suffix"
        fi
    fi
done

# -----------------------------------------------------------------------------
# Run differential_test against the staged flat directory.
# -----------------------------------------------------------------------------
log "Running differential_test on $count programs × 7 backends ($((count * 7)) total runs)"
"$BIN/differential_test" "$STAGE" > "$OUT_RAW" 2>&1
dt_exit=$?

# Parse the trailing SUMMARY block that differential_test prints.
dt_total=$(awk '/^Examples scanned:/{print $NF}' "$OUT_RAW" | tail -1)
dt_pass=$(awk  '/^[[:space:]]*PASS[[:space:]]*:[[:space:]]/{print $(NF-2)}' "$OUT_RAW" | tail -1)
dt_diff=$(awk  '/^[[:space:]]*DIFFERENTIAL FAILURE[[:space:]]*:[[:space:]]/{print $(NF-2)}' "$OUT_RAW" | tail -1)
dt_crash=$(awk '/^[[:space:]]*CRASH \(some backend\)[[:space:]]*:[[:space:]]/{print $(NF-2)}' "$OUT_RAW" | tail -1)
dt_tmo=$(awk   '/^[[:space:]]*TIMEOUT \(some backend\)[[:space:]]*:[[:space:]]/{print $(NF-2)}' "$OUT_RAW" | tail -1)
dt_cfail=$(awk  '/^[[:space:]]*COMPILE FAIL \(all\)[[:space:]]*:[[:space:]]/{print $(NF-2)}' "$OUT_RAW" | tail -1)
dt_total="${dt_total:-0}"; dt_pass="${dt_pass:-0}"; dt_diff="${dt_diff:-0}"
dt_crash="${dt_crash:-0}"; dt_tmo="${dt_tmo:-0}"; dt_cfail="${dt_cfail:-0}"
dt_fail=$((dt_diff + dt_crash + dt_tmo + dt_cfail))

{
    echo "differential_full timestamp=$TS programs=$count backends=7 total_runs=$((count * 7))"
    echo "differential_full pass=$dt_pass fail=$dt_fail (diff=$dt_diff crash=$dt_crash timeout=$dt_tmo compilefail=$dt_cfail) tool_exit=$dt_exit"
} | tee "$OUT_SUM"

# Refresh "latest" symlinks.
ln -sf "differential_full_${TS}.txt"    "$TEST_OUT/differential_full_latest.txt"
ln -sf "differential_full_${TS}.summary" "$TEST_OUT/differential_full_latest.summary"

# -----------------------------------------------------------------------------
# Surface disagreements inline for CI log readability.
# -----------------------------------------------------------------------------
echo ""
echo "=== Differential disagreements (program → per-backend exit codes) ==="
# Look for the "Differential failures" detail block in the raw transcript.
awk '
    /^--- DIFFERENTIAL FAILURES \(/ { in_block=1; next }
    /^--- CRASHES \(|^--- TIMEOUTS \(|^--- COMPILE FAIL|^--- PASS \(|^=== / { in_block=0 }
    in_block { print }
' "$OUT_RAW" | head -100

echo ""
echo "=== Differential test complete ==="
echo "  programs   : $count"
echo "  pass       : $dt_pass"
echo "  diff fails : $dt_diff"
echo "  crashes    : $dt_crash"
echo "  timeouts   : $dt_tmo"
echo "  compile    : $dt_cfail"
echo "  raw        : $OUT_RAW"
echo "  summary    : $OUT_SUM"

if [[ "$dt_fail" -gt 0 ]]; then
    err "$dt_fail differential failures detected"
    exit 1
fi
log "All $count programs agree across all 7 backends"
exit 0
