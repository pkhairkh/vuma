#!/usr/bin/env bash
# Run all 5754 gold standard tests across all 7 native backends in parallel.
# Wasm32 is excluded (no native execution available).
#
# Usage: ./scripts/run_all_gold.sh [jobs]
#   jobs  - number of parallel workers per backend (default: 8)
#
# Output:
#   /tmp/vuma_results/<backend>.tsv  - per-program results
#   /tmp/vuma_results/summary.txt    - aggregated stats

set -uo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
COMPILE_DUMP="$REPO_ROOT/target/release/compile_dump"
GOLD_DIR="$REPO_ROOT/tests/gold_standard"
QEMU_DIR="/tmp/qemu/extracted/usr/bin"
OUT_DIR="/tmp/vuma_results"
JOBS="${1:-8}"

mkdir -p "$OUT_DIR"

# Backend -> QEMU binary map (x86_64 runs natively)
declare -A QEMU
QEMU[x86_64]=""
QEMU[aarch64]="$QEMU_DIR/qemu-aarch64"
QEMU[riscv64]="$QEMU_DIR/qemu-riscv64"
QEMU[arm32]="$QEMU_DIR/qemu-arm"
QEMU[mips64]="$QEMU_DIR/qemu-mips64el"
QEMU[ppc64]="$QEMU_DIR/qemu-ppc64"
QEMU[loongarch64]="$QEMU_DIR/qemu-loongarch64"

BACKENDS=(x86_64 aarch64 riscv64 arm32 mips64 ppc64 loongarch64)

# Collect all .vuma files
VUMA_FILES=()
while IFS= read -r f; do
    VUMA_FILES+=("$f")
done < <(find "$GOLD_DIR" -name '*.vuma' | sort)

TOTAL=${#VUMA_FILES[@]}
echo "Total programs: $TOTAL"
echo "Backends: ${BACKENDS[*]}"
echo "Parallel jobs per backend: $JOBS"
echo "Output: $OUT_DIR"
echo ""

run_one() {
    local backend="$1"
    local qemu="${QEMU[$backend]}"
    local vuma_file="$2"
    local out_bin
    out_bin=$(mktemp /tmp/vuma_bin_XXXXXX.bin)

    # Extract expected exit code from header
    local expected=""
    expected=$(grep -m1 -E "^// *[Ee]xpected exit code: *([0-9]+)" "$vuma_file" \
               | sed -E 's/^.*: *([0-9]+).*$/\1/')

    # Compile
    local compile_out
    compile_out=$("$COMPILE_DUMP" "$vuma_file" "$out_bin" "$backend" 2>&1)
    if [ ! -s "$out_bin" ]; then
        rm -f "$out_bin"
        printf "%s\t%s\t%s\t%s\t%s\n" "$vuma_file" "compile_fail" "-" "$expected" "-"
        return
    fi
    chmod +x "$out_bin"

    # Execute
    local code=-
    local status=run_ok
    if [ -z "$qemu" ]; then
        # Native x86_64
        local res
        res=$(timeout 3 "$out_bin" 2>/dev/null; echo "EXIT:$?")
        code=$(echo "$res" | grep -oE 'EXIT:[0-9-]+' | cut -d: -f2)
    else
        local res
        res=$(timeout 5 "$qemu" "$out_bin" 2>/dev/null; echo "EXIT:$?")
        code=$(echo "$res" | grep -oE 'EXIT:[0-9-]+' | cut -d: -f2)
    fi

    # Classify
    case "$code" in
        124) status=timeout ;;
        139|134|136|131|137) status=crash ;;
        -1) status=exec_fail ;;
        *)  status=run_ok ;;
    esac

    # Strict pass?
    if [ "$status" = "run_ok" ] && [ -n "$expected" ]; then
        if [ "$code" = "$expected" ]; then
            status=strict_pass
        else
            status=wrong_exit
        fi
    elif [ "$status" = "run_ok" ] && [ -z "$expected" ]; then
        status=pass_any  # ran cleanly, no expected code to compare
    fi

    rm -f "$out_bin"
    # Strip absolute path prefix for readability
    local rel
    rel="${vuma_file#$GOLD_DIR/}"
    printf "%s\t%s\t%s\t%s\t%s\n" "$rel" "$status" "$code" "$expected" "$backend"
}
export -f run_one
export COMPILE_DUMP GOLD_DIR QEMU_DIR

echo "=== Starting parallel runs ==="
for backend in "${BACKENDS[@]}"; do
    echo "[$(date +%H:%M:%S)] Launching $backend ..."
done
echo ""

# Run all backends in parallel; each backend processes its files in parallel
for backend in "${BACKENDS[@]}"; do
    (
        for f in "${VUMA_FILES[@]}"; do
            echo "$f"
        done | xargs -I{} -P "$JOBS" bash -c "run_one '$backend' '{}'" \
            > "$OUT_DIR/$backend.tsv" 2>/dev/null
        echo "[$(date +%H:%M:%S)] $backend done: $(wc -l < $OUT_DIR/$backend.tsv) rows"
    ) &
done
wait

echo ""
echo "=== All backends finished ==="
ls -la "$OUT_DIR/"
