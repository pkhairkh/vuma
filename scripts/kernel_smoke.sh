#!/usr/bin/env bash
#
# scripts/kernel_smoke.sh — VWK kernel smoke-test harness (hosted x86_64).
#
# WHAT THIS SCRIPT DOES
# ---------------------
# 1. Builds the `compile_dump` binary (release-fast profile) if it is
#    missing or older than Cargo.toml.
# 2. Compiles womb/kernel/kernel.vuma for the x86_64 backend with PMT
#    verification (--verify) enabled.
# 3. Runs the resulting ELF binary as a normal Linux process.
# 4. Greps the combined stdout+stderr for the banner text
#    "VWK kernel booted".  The kernel emits this with ANSI color
#    escapes interspersed (\x1b[36m\x1b[1mVWK\x1b[0m kernel booted),
#    so we match with a regex that tolerates the escape bytes
#    between "VWK" and "kernel booted".
# 5. Verifies the process exit code is 0.
# 6. Prints "PASS: ..." on success or "FAIL: ..." on any failure, and
#    exits 0 (PASS) or 1 (FAIL) accordingly.
#
# HOSTED-MODE ONLY (FOR NOW)
# --------------------------
# This harness executes the kernel as a regular x86_64 Linux process.
# The kernel's `host_*` abstraction (see womb/kernel/hosted/host.vuma)
# is wired to the host's libc syscalls (write/read/exit/mmap/...), so
# the kernel can boot, print, and exit like any other userspace
# program.  This is the only execution environment available in the
# current sandbox: there is no QEMU (no sudo, no system/user emulator).
#
# BARE-METAL QEMU IS A K11 TASK
# -----------------------------
# Wave K11 (parity sweep) will install QEMU and add a second harness
# that boots the same kernel.vuma under a bare-metal x86_64 QEMU
# target with a real trampoline + MMIO console.  Until then, this
# hosted-mode smoke test is the gate for the "kernel boots, prints
# banner, exits 0" DoD item.

set -euo pipefail

# Always operate from the repo root regardless of where the script is
# invoked from.  This makes the script safe to call as
#   ./scripts/kernel_smoke.sh
# or
#   bash /path/to/vuma/scripts/kernel_smoke.sh
REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$REPO_ROOT"

# Put cargo + the nightly toolchain on PATH (needed for the rebuild
# step below; harmless if compile_dump is already up to date). Skip
# if absent (e.g., minimal CI runners that pre-install cargo on PATH).
# shellcheck disable=SC1091
[ -f "$HOME/.cargo/env" ] && source "$HOME/.cargo/env"

BIN=./target/release-fast/compile_dump
KERNEL_SRC=womb/kernel/kernel.vuma
KERNEL_BIN=/tmp/kernel_smoke.bin
KERNEL_OUT=/tmp/kernel_smoke.out

# ---------------------------------------------------------------------------
# Step 1: ensure compile_dump exists and is newer than Cargo.toml.
# ---------------------------------------------------------------------------
need_build=0
if [[ ! -x "$BIN" ]]; then
    need_build=1
elif [[ Cargo.toml -nt "$BIN" ]]; then
    need_build=1
fi

if [[ "$need_build" -eq 1 ]]; then
    echo "[smoke] building compile_dump (release-fast)..."
    CARGO_BUILD_JOBS=4 cargo build --profile release-fast --bin compile_dump
fi

# ---------------------------------------------------------------------------
# Step 2: compile the kernel with --verify, capturing IVE output.
# ---------------------------------------------------------------------------
echo "[smoke] compiling $KERNEL_SRC -> $KERNEL_BIN (x86_64, --verify)"
compile_log=$(mktemp)
if ! "$BIN" "$KERNEL_SRC" "$KERNEL_BIN" x86_64 --verify >"$compile_log" 2>&1; then
    echo "FAIL: compile/verify error"
    echo "----- compile_dump output -----"
    cat "$compile_log"
    echo "-------------------------------"
    rm -f "$compile_log"
    exit 1
fi

# IVE is expected to print either "IVE: Pass" or "IVE: Fail".  Treat
# any "IVE: Fail" line (or absence of an "IVE:" line) as a failure.
if ! grep -q "IVE: Pass" "$compile_log"; then
    echo "FAIL: compile/verify error"
    echo "----- compile_dump output -----"
    cat "$compile_log"
    echo "-------------------------------"
    rm -f "$compile_log"
    exit 1
fi
if grep -q "IVE: Fail" "$compile_log"; then
    echo "FAIL: compile/verify error"
    echo "----- compile_dump output -----"
    cat "$compile_log"
    echo "-------------------------------"
    rm -f "$compile_log"
    exit 1
fi
rm -f "$compile_log"

# Sanity: the binary file itself must exist on disk.
if [[ ! -x "$KERNEL_BIN" ]]; then
    echo "FAIL: compile/verify error (binary not produced: $KERNEL_BIN)"
    exit 1
fi

# ---------------------------------------------------------------------------
# Step 3: run the binary, capturing stdout+stderr and the exit code.
#
# `set +e` is required because the kernel might exit non-zero (that is
# exactly one of the failure modes we are testing for) and we want to
# inspect $rc rather than have the script abort on the spot.  We
# re-enable `set -e` immediately after.
#
# stdin is redirected from /dev/null so the kernel's shell reads EOF
# immediately and exits cleanly.  Without this, when the smoke test is
# invoked from a context whose stdin is a blocking pipe (e.g. CI or
# `bash scripts/kernel_smoke.sh | tail`), the kernel would block
# forever in read() waiting for interactive input.
# ---------------------------------------------------------------------------
echo "[smoke] running $KERNEL_BIN"
set +e
"$KERNEL_BIN" < /dev/null >"$KERNEL_OUT" 2>&1
rc=$?
set -e

# ---------------------------------------------------------------------------
# Step 4: check exit code.
# ---------------------------------------------------------------------------
if [[ $rc -ne 0 ]]; then
    echo "FAIL: exit code $rc (expected 0)"
    echo "----- kernel output -----"
    cat "$KERNEL_OUT"
    echo "-------------------------"
    exit 1
fi

# ---------------------------------------------------------------------------
# Step 5: grep the output for the exact banner.
# ---------------------------------------------------------------------------
if ! grep -q "VWK.*kernel booted" "$KERNEL_OUT"; then
    echo "FAIL: banner not found in output"
    cat "$KERNEL_OUT"
    exit 1
fi

# ---------------------------------------------------------------------------
# Step 6: success.
# ---------------------------------------------------------------------------
echo "PASS: kernel boots, prints banner, exits 0"
exit 0
