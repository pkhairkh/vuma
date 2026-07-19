#!/bin/bash
# Boot VWK kernel under QEMU system mode (bare metal)
# Requires: qemu-system-x86_64 installed
# Usage: bash scripts/qemu_system_boot.sh

set -e
cd /home/z/vuma-review

# Check if QEMU system is installed
if ! command -v qemu-system-x86_64 &>/dev/null; then
    echo "ERROR: qemu-system-x86_64 is not installed"
    echo "Install with: apt-get install qemu-system-x86"
    echo "Or: pacman -S qemu-system-x86"
    exit 1
fi

# Build compiler if needed
if [ ! -f target/release-fast/compile_dump ]; then
    echo "[qemu] Building compiler..."
    . "$HOME/.cargo/env"
    cargo build --profile release-fast --bin compile_dump 2>&1 | tail -3
fi

# Compile kernel for bare-metal x86_64
echo "[qemu] Compiling kernel.vuma for x86_64..."
./target/release-fast/compile_dump womb/kernel/kernel.vuma /tmp/vwk_kernel.bin x86_64 --verify 2>&1 | tail -3

# Boot under QEMU system mode
# -kernel: load the kernel ELF
# -m 128: 128MB RAM
# -nographic: serial console only (no VGA window)
# -no-reboot: don't reboot on exit
# -serial mon:stdio: serial output to terminal
echo "[qemu] Booting under QEMU system mode..."
echo "[qemu] Press Ctrl-A X to exit QEMU"
qemu-system-x86_64 \
    -kernel /tmp/vwk_kernel.bin \
    -m 128 \
    -nographic \
    -no-reboot \
    -serial mon:stdio \
    2>&1 | head -50

echo "[qemu] Boot attempt complete"
