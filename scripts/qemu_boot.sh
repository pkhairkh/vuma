#!/usr/bin/env bash
# scripts/qemu_boot.sh — Boot the VWK kernel under QEMU
#
# Usage:
#   ./scripts/qemu_boot.sh [backend] [interactive]
#
# Examples:
#   ./scripts/qemu_boot.sh x86_64 interactive   # Interactive shell on x86_64
#   ./scripts/qemu_boot.sh aarch64               # Boot aarch64 kernel (pipe input)
#   ./scripts/qemu_boot.sh riscv64 interactive   # Interactive shell on RISC-V
#
# For Pi 5 (aarch64):
#   ./scripts/qemu_boot.sh aarch64 interactive
#
# For hosted mode (no QEMU system, just user-mode):
#   ./scripts/qemu_boot.sh x86_64 interactive

set -uo pipefail

REPO="/home/z/vuma"
source /home/z/.cargo/env
cd "$REPO"

BIN="./target/release-fast/compile_dump"
BACKEND="${1:-x86_64}"
MODE="${2:-pipe}"

# Rebuild if needed
if [ ! -x "$BIN" ] || [ "Cargo.toml" -nt "$BIN" ]; then
    echo "[qemu] building compile_dump..."
    cargo build --profile release-fast --bin compile_dump 2>&1 | tail -3
fi

# Compile the kernel
echo "[qemu] compiling kernel for $BACKEND..."
$BIN womb/kernel/kernel.vuma /tmp/vwk_kernel.bin "$BACKEND" --verify 2>&1 | tail -3

if [ ! -f /tmp/vwk_kernel.bin ]; then
    echo "[qemu] FAIL: kernel binary not produced"
    exit 1
fi

echo "[qemu] kernel compiled: $(ls -la /tmp/vwk_kernel.bin | awk '{print $5}') bytes"
echo ""

# QEMU user-mode binary mapping
get_qemu() {
    case "$1" in
        x86_64)   echo "";;
        aarch64)  echo "/usr/local/bin/qemu-aarch64";;
        aarch64_be) echo "/usr/local/bin/qemu-aarch64_be";;
        riscv64)  echo "/usr/local/bin/qemu-riscv64";;
        riscv32)  echo "/usr/local/bin/qemu-riscv32";;
        arm32)    echo "/usr/local/bin/qemu-arm";;
        armeb)    echo "/usr/local/bin/qemu-armeb";;
        ppc64)    echo "/usr/local/bin/qemu-ppc64";;
        ppc64le)  echo "/usr/local/bin/qemu-ppc64le";;
        loongarch64) echo "/usr/local/bin/qemu-loongarch64";;
        mips64)   echo "/usr/local/bin/qemu-mips64el";;
        mips64be) echo "/usr/local/bin/qemu-mips64";;
        s390x)    echo "/usr/local/bin/qemu-s390x";;
        sparc64)  echo "/usr/local/bin/qemu-sparc64";;
        alpha)    echo "/usr/local/bin/qemu-alpha";;
        hppa)     echo "/usr/local/bin/qemu-hppa";;
        m68k)     echo "/usr/local/bin/qemu-m68k";;
        x86_32)   echo "/usr/local/bin/qemu-i386";;
        wasm32)   echo "";;
        *)        echo "";;
    esac
}

QEMU_BIN=$(get_qemu "$BACKEND")

if [ "$BACKEND" = "x86_64" ]; then
    RUNNER=""
elif [ "$BACKEND" = "wasm32" ]; then
    echo "[qemu] wasm32 backend — use wasmtime to run:"
    echo "  wasmtime /tmp/vwk_kernel.bin"
    exit 0
elif [ -z "$QEMU_BIN" ] || [ ! -x "$QEMU_BIN" ]; then
    echo "[qemu] No QEMU binary for $BACKEND"
    echo "[qemu] Install with: curl -sL ... qemu-user-static"
    exit 1
else
    RUNNER="$QEMU_BIN"
fi

echo "[qemu] booting VWK kernel ($BACKEND)..."
echo "========================================"

if [ "$MODE" = "interactive" ]; then
    # Interactive mode: connect stdin/stdout directly
    if [ -z "$RUNNER" ]; then
        /tmp/vwk_kernel.bin
    else
        $RUNNER /tmp/vwk_kernel.bin
    fi
else
    # Pipe mode: feed a default command sequence
    echo "help
ls
touch hello.txt
ls
cat hello.txt
mkdir docs
ls
pid
ps
ver
memstat
echo VWK is alive
alloc
exit" | {
        if [ -z "$RUNNER" ]; then
            /tmp/vwk_kernel.bin
        else
            $RUNNER /tmp/vwk_kernel.bin
        fi
    }
fi

EXIT_CODE=$?
echo "========================================"
echo "[qemu] kernel exited with code $EXIT_CODE"
