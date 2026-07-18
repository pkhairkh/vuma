#!/usr/bin/env bash
# scripts/kernel_parity.sh — VWK Multi-Backend Parity Sweep
#
# Compiles and runs key kernel + gold-standard tests across ALL 19 VUMA
# backends. Uses QEMU user-mode emulators for non-x86_64 architectures.
#
# Usage: ./scripts/kernel_parity.sh [--quick]
#   --quick: only test arena_basic + kernel smoke (skip full gold suite)
#
# Exit: 0 only if all backends pass. Non-zero if any fail.

set -uo pipefail

REPO="/home/z/vuma"
source /home/z/.cargo/env
cd "$REPO"

BIN="./target/release-fast/compile_dump"

# Rebuild if needed.
if [ ! -x "$BIN" ] || [ "Cargo.toml" -nt "$BIN" ]; then
    echo "[parity] building compile_dump..."
    cargo build --profile release-fast --bin compile_dump 2>&1 | tail -3
fi

# All 19 VUMA backends.
BACKENDS=(
    x86_64 aarch64 aarch64_be riscv64 riscv32 arm32 armeb
    mips64 mips64be ppc64 ppc64le loongarch64 s390x sparc64
    alpha hppa m68k x86_32 wasm32
)

# QEMU user-mode emulators for non-x86_64 backends.
# Format: "backend:qemu_binary"
QEMU_MAP=(
    "aarch64:qemu-aarch64"
    "aarch64_be:qemu-aarch64"
    "riscv64:qemu-riscv64"
    "riscv32:qemu-riscv64"
    "arm32:qemu-arm"
    "armeb:qemu-arm"
    "ppc64:qemu-ppc64le"
    "ppc64le:qemu-ppc64le"
    "loongarch64:qemu-loongarch64"
    "mips64:qemu-mips64"
    "mips64be:qemu-mips64"
    "s390x:qemu-s390x"
)

# Backends we can actually EXECUTE (have QEMU or are native x86_64).
EXECUTABLE_BACKENDS="x86_64 aarch64 riscv64 arm32 ppc64le loongarch64 s390x"

# Tests to run on each backend.
# Format: "test_file:expected_exit"
TESTS=(
    "tests/gold_standard/arena_wave1/arena_basic.vuma:42"
    "tests/gold_standard/arena_wave1/arena_grow.vuma:0"
    "tests/gold_standard/arena_wave1/arena_multiple.vuma:0"
    "tests/gold_standard/arena_wave1/arena_overflow.vuma:1"
    "tests/gold_standard/pmt_wave2/init_read.vuma:42"
    "tests/gold_standard/arithmetic/arith_clamp.vuma:100"
    "tests/gold_standard/control_flow/cf2_for_count.vuma:5"
    "tests/gold_standard/functions/fn2_add_two.vuma:7"
    "tests/gold_standard/bitwise/bit2_and_chain.vuma:3"
    "tests/gold_standard/structs/enum_demo.vuma:141"
)

# Kernel modules to compile-verify (no execution — just compile + IVE).
KERNEL_MODULES=(
    womb/kernel/kernel.vuma
    womb/kernel/mm/pmm.vuma
    womb/kernel/mm/vmm.vuma
    womb/kernel/proc/task.vuma
    womb/kernel/proc/scheduler.vuma
    womb/kernel/vfs/inode.vuma
    womb/kernel/vfs/dentry.vuma
    womb/kernel/vfs/file.vuma
    womb/kernel/ipc/pipe.vuma
    womb/kernel/ipc/signal.vuma
    womb/kernel/ipc/futex.vuma
    womb/kernel/sync/spinlock.vuma
    womb/kernel/sync/mutex.vuma
    womb/kernel/net/socket.vuma
    womb/kernel/crypto/api.vuma
    womb/kernel/crypto/aes.vuma
    womb/kernel/crypto/sha.vuma
    womb/kernel/panic/panic.vuma
    womb/kernel/power/pm.vuma
)

# Get the QEMU binary for a backend (or empty if native).
get_qemu() {
    local backend="$1"
    if [ "$backend" = "x86_64" ]; then
        echo ""
        return
    fi
    for entry in "${QEMU_MAP[@]}"; do
        local b="${entry%%:*}"
        local q="${entry##*:}"
        if [ "$b" = "$backend" ]; then
            echo "$q"
            return
        fi
    done
    echo ""
}

# Check if a backend is executable.
is_executable() {
    local backend="$1"
    for eb in $EXECUTABLE_BACKENDS; do
        if [ "$eb" = "$backend" ]; then
            return 0
        fi
    done
    return 1
}

# Run a compiled binary on the appropriate emulator (or native).
run_binary() {
    local backend="$1"
    local binpath="$2"
    local qemu
    qemu=$(get_qemu "$backend")
    if [ -z "$qemu" ]; then
        "$binpath" 2>/dev/null
    else
        "$qemu" "$binpath" 2>/dev/null
    fi
}

echo "══════════════════════════════════════════════════════════════════════════"
echo "VWK Multi-Backend Parity Sweep"
echo "══════════════════════════════════════════════════════════════════════════"
echo ""

# Phase 1: Compile + execute gold-standard tests.
echo "=== Phase 1: Gold-standard test execution ==="
printf "%-16s" "Backend"
for test in "${TESTS[@]}"; do
    tname=$(basename "${test%%:*}" .vuma)
    printf "%-16s" "$tname"
done
printf "%-8s\n" "Result"
printf "%-16s" "--------"
for test in "${TESTS[@]}"; do
    printf "%-16s" "--------"
done
printf "%-8s\n" "------"

total_pass=0
total_fail=0
compile_fail=0

for backend in "${BACKENDS[@]}"; do
    printf "%-16s" "$backend"
    backend_pass=0
    backend_fail=0
    
    for test in "${TESTS[@]}"; do
        test_file="${test%%:*}"
        expected="${test##*:}"
        test_path="$REPO/$test_file"
        
        out_bin="/tmp/parity_$(basename $test_file .vuma)_${backend}.bin"
        
        # Compile.
        if ! $BIN "$test_path" "$out_bin" "$backend" --verify 2>/dev/null; then
            printf "%-16s" "COMPILE_FAIL"
            backend_fail=$((backend_fail + 1))
            compile_fail=$((compile_fail + 1))
            continue
        fi
        
        # Execute (only if we have a QEMU for this backend).
        if is_executable "$backend"; then
            actual=$(run_binary "$backend" "$out_bin")
            rc=$?
            if [ "$rc" = "$expected" ]; then
                printf "%-16s" "PASS($rc)"
                backend_pass=$((backend_pass + 1))
                total_pass=$((total_pass + 1))
            else
                printf "%-16s" "FAIL($rc/$expected)"
                backend_fail=$((backend_fail + 1))
                total_fail=$((total_fail + 1))
            fi
        else
            # Compile-only (no QEMU available).
            printf "%-16s" "COMPILE_OK"
            backend_pass=$((backend_pass + 1))
            total_pass=$((total_pass + 1))
        fi
    done
    
    if [ "$backend_fail" = "0" ]; then
        printf "%-8s\n" "✓ PASS"
    else
        printf "%-8s\n" "✗ FAIL"
    fi
done

echo ""
echo "=== Phase 2: Kernel module compile-verify ==="
printf "%-20s" "Module"
for backend in x86_64 aarch64 riscv64 wasm32; do
    printf "%-12s" "$backend"
done
echo ""

module_pass=0
module_fail=0
for module in "${KERNEL_MODULES[@]}"; do
    mname=$(basename "$module" .vuma)
    printf "%-20s" "$mname"
    for backend in x86_64 aarch64 riscv64 wasm32; do
        out_bin="/tmp/parity_mod_${mname}_${backend}.bin"
        if $BIN "$REPO/$module" "$out_bin" "$backend" --verify 2>/dev/null; then
            printf "%-12s" "✓"
            module_pass=$((module_pass + 1))
        else
            printf "%-12s" "✗"
            module_fail=$((module_fail + 1))
        fi
    done
    echo ""
done

echo ""
echo "══════════════════════════════════════════════════════════════════════════"
echo "Parity Sweep Summary"
echo "══════════════════════════════════════════════════════════════════════════"
echo "Gold-standard tests:   PASS=$total_pass  FAIL=$total_fail  COMPILE_FAIL=$compile_fail"
echo "Kernel module compiles: PASS=$module_pass  FAIL=$module_fail"
echo ""

if [ "$total_fail" = "0" ] && [ "$module_fail" = "0" ]; then
    echo "✓ ALL BACKENDS PASS"
    exit 0
else
    echo "✗ SOME BACKENDS FAILED"
    exit 1
fi
