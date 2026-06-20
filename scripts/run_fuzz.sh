#!/usr/bin/env bash
# ============================================================================
# run_fuzz.sh — VUMA fuzzing driver wrapper
# ----------------------------------------------------------------------------
# Runs the integrated fuzz_driver binary for N (default 1000) randomly
# generated programs, exercising all 7 native backends
#   (x86_64, aarch64, riscv64, arm32, mips64el, ppc64, loongarch64).
#
# The fuzz_driver binary exits non-zero when any failure (crash, timeout,
# differential disagreement, or all-backends compile fail) is observed,
# which makes this script CI-friendly.
#
# Output:
#   test_results/fuzz_<N>_<timestamp>.txt       — raw transcript
#   test_results/fuzz_<N>_<timestamp>.summary   — one-line summary
#   test_results/fuzz_latest.{txt,summary}      — symlinks to newest
#
# Usage:  ./scripts/run_fuzz.sh [N] [SEED]
#   N    — number of programs (default: 1000, or $FUZZ_COUNT if set)
#   SEED — RNG seed for reproducibility (default: 42, or $FUZZ_SEED if set)
#
# Environment overrides:
#   VUMA_ROOT, TEST_RESULTS_DIR, FUZZ_COUNT, FUZZ_SEED, FUZZ_DUMP
# Exit code: 0 if no failures, 1 if any failure observed, 2 if binary missing.
# ============================================================================
set -uo pipefail
export PATH="$HOME/.cargo/bin:$PATH"

VUMA_ROOT="${VUMA_ROOT:-/tmp/my-project}"
TEST_OUT="${TEST_RESULTS_DIR:-$VUMA_ROOT/test_results}"
BIN="$VUMA_ROOT/target/release"

N="${1:-${FUZZ_COUNT:-1000}}"
SEED="${2:-${FUZZ_SEED:-42}}"

mkdir -p "$TEST_OUT"

log()  { printf '[fuzz][%s] %s\n' "$(date +%H:%M:%S)" "$*"; }
err()  { printf '[fuzz][FAIL] %s\n' "$*" >&2; }

if [[ ! -x "$BIN/fuzz_driver" ]]; then
    err "missing $BIN/fuzz_driver — run ci_run_tests.sh first or build it"
    exit 2
fi

# Ensure QEMU symlinks exist (fuzz_driver hard-codes /tmp/qemu_bins paths).
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

TS="$(date -u +%Y%m%dT%H%M%SZ)"
OUT_RAW="$TEST_OUT/fuzz_${N}_${TS}.txt"
OUT_SUM="$TEST_OUT/fuzz_${N}_${TS}.summary"

log "Running fuzz_driver with count=$N seed=$SEED"
ARGS=(--count "$N" --seed "$SEED")
if [[ "${FUZZ_DUMP:-0}" == "1" ]]; then
    ARGS+=(--dump)
fi

"$BIN/fuzz_driver" "${ARGS[@]}" > "$OUT_RAW" 2>&1
fz_exit=$?

# Parse the report block fuzz_driver prints:
#   Programs generated : N
#   Pass                  : N
#   Compile failures      : N
#   Crashes               : N
#   Timeouts              : N
#   Differential failures : N
fz_pass=$(awk   '/^[[:space:]]*Pass[[:space:]]*:[[:space:]]/{print $NF}' "$OUT_RAW" | tail -1)
fz_cfail=$(awk  '/Compile failures[[:space:]]*:[[:space:]]/{print $NF}' "$OUT_RAW" | tail -1)
fz_crash=$(awk  '/^[[:space:]]*Crashes[[:space:]]*:[[:space:]]/{print $NF}' "$OUT_RAW" | tail -1)
fz_tmo=$(awk    '/Timeouts[[:space:]]*:[[:space:]]/{print $NF}' "$OUT_RAW" | tail -1)
fz_diff=$(awk   '/Differential failures[[:space:]]*:[[:space:]]/{print $NF}' "$OUT_RAW" | tail -1)
fz_pass="${fz_pass:-0}"; fz_cfail="${fz_cfail:-0}"; fz_crash="${fz_crash:-0}"
fz_tmo="${fz_tmo:-0}"; fz_diff="${fz_diff:-0}"
fz_fail=$((fz_cfail + fz_crash + fz_tmo + fz_diff))

{
    echo "fuzz timestamp=$TS count=$N seed=$SEED total_runs=$((N * 7))"
    echo "fuzz pass=$fz_pass fail=$fz_fail (compile=$fz_cfail crash=$fz_crash timeout=$fz_tmo diff=$fz_diff) tool_exit=$fz_exit"
} | tee "$OUT_SUM"

# Refresh "latest" symlinks.
ln -sf "fuzz_${N}_${TS}.txt"     "$TEST_OUT/fuzz_latest.txt"
ln -sf "fuzz_${N}_${TS}.summary" "$TEST_OUT/fuzz_latest.summary"

# Surface headline failures inline for CI log readability.
echo ""
echo "=== Fuzz headline failures ==="
# Differential failures block.
awk '
    /^--- Differential failures \(/ { in_block=1; print; next }
    /^--- Crashes \(|^=== / { in_block=0 }
    in_block { print }
' "$OUT_RAW" | head -50
# Crashes block.
awk '
    /^--- Crashes \(/ { in_block=1; print; next }
    /^=== / { in_block=0 }
    in_block { print }
' "$OUT_RAW" | head -50

echo ""
echo "=== Fuzz run complete ==="
echo "  programs   : $N (seed=$SEED)"
echo "  pass       : $fz_pass"
echo "  diff fails : $fz_diff"
echo "  crashes    : $fz_crash"
echo "  timeouts   : $fz_tmo"
echo "  compile    : $fz_cfail"
echo "  raw        : $OUT_RAW"
echo "  summary    : $OUT_SUM"

if [[ "$fz_fail" -gt 0 ]]; then
    err "$fz_fail fuzz failures detected (compile=$fz_cfail crash=$fz_crash timeout=$fz_tmo diff=$fz_diff)"
    exit 1
fi
log "All $N fuzz programs passed across all 7 backends"
exit 0
