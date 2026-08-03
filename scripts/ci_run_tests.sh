#!/bin/bash
# ============================================================================
# ci_run_tests.sh — VUMA gold-standard CI test runner
# ----------------------------------------------------------------------------
# Main CI entry point. Builds the four test-driver binaries, then runs:
#   1. The original 47 examples across all 7 native QEMU backends.
#   2. The gold-standard test suite on the reference x86_64 backend.
#   3. The O0-vs-O3 optimizer soundness comparison.
#   4. The fuzz_driver with 50 randomly generated programs.
# Output is streamed to stdout and to test_results/ for post-hoc analysis.
# Exit status: 0 if everything compiled; non-zero on hard build failure.
# ============================================================================
set -e
export PATH="$HOME/.cargo/bin:$PATH"

# Resolve repo root from this script's location so the entrypoint works
# regardless of where the caller invoked it from (CI checks out the repo
# to a runner-local path, never assuming a fixed checkout location).
REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$REPO_ROOT"

# Locate QEMU user-mode emulators. CI stages symlinks under
# /tmp/qemu_bins/ via .github/workflows/vuma-tests.yml; on developer
# machines the emulators are typically on PATH (qemu-user package).
# We prefer /tmp/qemu_bins/ when present (matches the path the Rust
# differential_test binary hard-codes) and fall back to `command -v`.
qemu_bin_for() {
    local arch="$1"
    if [ -x "/tmp/qemu_bins/qemu-$arch" ]; then
        printf '%s\n' "/tmp/qemu_bins/qemu-$arch"
    else
        command -v "qemu-$arch" 2>/dev/null || true
    fi
}

echo "=== Building VUMA tools ==="
RUSTUP_TOOLCHAIN=stable cargo build --release --bin compile_dump --bin differential_test --bin opt_level_test --bin fuzz_driver 2>&1 | tail -5

echo "=== Running original 47 examples on all backends ==="
mkdir -p test_results
for be_qemu in "x86_64:" "arm32:$(qemu_bin_for arm)" "mips64:$(qemu_bin_for mips64el)" "aarch64:$(qemu_bin_for aarch64)" "riscv64:$(qemu_bin_for riscv64)" "ppc64:$(qemu_bin_for ppc64)" "loongarch64:$(qemu_bin_for loongarch64)"; do
    be="${be_qemu%%:*}"
    qemu="${be_qemu#*:}"
    echo "--- $be ---"
    if [ -z "$qemu" ]; then
        ./target/release/compile_dump diag $be examples 2>&1 | grep -E "^(Total|Pass|Crashes|Timeouts)"
    else
        ./target/release/compile_dump diag $be examples $qemu 2>&1 | grep -E "^(Total|Pass|Crashes|Timeouts)"
    fi
done

echo "=== Running gold standard tests on x86_64 ==="
# V-NEW-6 fix: the previous pass criterion only checked that the test
# binary didn't crash/timeout (exit codes 124/139/134). Tests that
# exited 0 with the WRONG answer were counted as passing. The fix
# compares the actual exit code against the expected exit code declared
# in each .vuma file's `// Expected exit code: N` header. If the
# header is missing, we fall back to the old "didn't crash" criterion
# (so legacy tests without the header still work).
total=0; pass=0; mismatch=0; missing_header=0
for f in tests/gold_standard/*/*.vuma; do
    total=$((total + 1))
    name=$(basename "$f" .vuma)
    # Extract expected exit code from the test file's header comment.
    # The header is `// Expected exit code: N` where N can be negative
    # (e.g. -1 for tests that expect a signal). We use grep + sed
    # instead of a single regex to handle both signed and unsigned.
    expected=$(grep -m1 -iE '^//\s*Expected exit code:\s*(-?[0-9]+)' "$f" \
        | sed -E 's/^.*Expected exit code:\s*(-?[0-9]+).*/\1/I')
    if [ -z "$expected" ]; then
        # No header — fall back to old "didn't crash" criterion.
        missing_header=$((missing_header + 1))
        ./target/release/compile_dump "$f" /tmp/gs_${name}.bin x86_64 2>/dev/null
        chmod +x /tmp/gs_${name}.bin 2>/dev/null
        timeout 3 /tmp/gs_${name}.bin 2>/dev/null
        code=$?
        if [ $code -ne 124 ] && [ $code -ne 139 ] && [ $code -ne 134 ]; then
            pass=$((pass + 1))
        fi
        continue
    fi
    ./target/release/compile_dump "$f" /tmp/gs_${name}.bin x86_64 2>/dev/null
    chmod +x /tmp/gs_${name}.bin 2>/dev/null
    timeout 3 /tmp/gs_${name}.bin 2>/dev/null
    code=$?
    # V-NEW-6: compare against the expected exit code.
    if [ "$code" -eq "$expected" ]; then
        pass=$((pass + 1))
    else
        mismatch=$((mismatch + 1))
        echo "  MISMATCH: $name expected=$expected got=$code"
    fi
done
echo "Gold standard x86_64: $pass / $total (mismatches: $mismatch, no-header: $missing_header)"

echo "=== Running O0 vs O3 comparison ==="
./target/release/opt_level_test examples 2>&1 | tail -5

echo "=== Running fuzzer (50 programs) ==="
./target/release/fuzz_driver --count 50 --seed 42 2>&1 | tail -10

echo "=== CI Complete ==="
