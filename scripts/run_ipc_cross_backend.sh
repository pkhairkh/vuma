#!/bin/bash
# Cross-backend IPC test runner — Wave 16c
# Runs all tests/gold_standard/ipc/*.vuma tests on all 5 target backends
# via QEMU and reports a pass/fail matrix.
#
# Usage: scripts/run_ipc_cross_backend.sh [backend1] [backend2] ...
# If no backends specified, runs all 5: x86_64 aarch64 riscv64 arm32 loongarch64

set -e

REPO="/home/z/vuma-review"
QEMU_DIR="$HOME/.local/bin"
BACKENDS="${@:-x86_64 aarch64 riscv64 arm32 loongarch64}"
TEST_DIR="$REPO/tests/gold_standard/ipc"
COMPILE_DUMP="$REPO/target/debug/compile_dump"

# Ensure compiled
if [ ! -f "$COMPILE_DUMP" ]; then
    echo "compile_dump not found — building..."
    cd "$REPO" && . "$HOME/.cargo/env" && cargo build --workspace 2>/dev/null
fi

# Print header
printf "%-30s" "Test"
for b in $BACKENDS; do
    printf "%-12s" "$b"
done
echo
printf "%-30s" "------------------------------"
for b in $BACKENDS; do
    printf "%-12s" "------------"
done
echo

total_pass=0
total_fail=0

for vuma in "$TEST_DIR"/*.vuma; do
    name=$(basename "$vuma" .vuma)
    expected=$(grep -m1 'Expected exit code' "$vuma" | sed 's/.*Expected exit code[: ]*//' | grep -oE '^[0-9]+' | head -1)
    [ -z "$expected" ] && continue

    printf "%-30s" "$name"
    for b in $BACKENDS; do
        bin="/tmp/cb_${b}_${name}.bin"
        "$COMPILE_DUMP" "$vuma" "$bin" "$b" >/dev/null 2>&1
        if [ $? -ne 0 ]; then
            printf "%-12s" "CERR"
            total_fail=$((total_fail+1))
            continue
        fi
        case $b in
            x86_64)      timeout 10 "$bin" </dev/null >/dev/null 2>&1 ;;
            aarch64)     timeout 10 "$QEMU_DIR/qemu-aarch64-static" "$bin" </dev/null >/dev/null 2>&1 ;;
            riscv64)     timeout 10 "$QEMU_DIR/qemu-riscv64-static" "$bin" </dev/null >/dev/null 2>&1 ;;
            arm32)       timeout 10 "$QEMU_DIR/qemu-arm-static" "$bin" </dev/null >/dev/null 2>&1 ;;
            loongarch64) timeout 10 "$QEMU_DIR/qemu-loongarch64-static" "$bin" </dev/null >/dev/null 2>&1 ;;
        esac
        rc=$?
        if [ "$rc" = "$expected" ]; then
            printf "%-12s" "PASS"
            total_pass=$((total_pass+1))
        else
            printf "%-12s" "FAIL($rc)"
            total_fail=$((total_fail+1))
        fi
    done
    echo
done

echo
echo "=== Summary: $total_pass passed, $total_fail failed ==="
[ $total_fail -gt 0 ] && exit 1 || exit 0
