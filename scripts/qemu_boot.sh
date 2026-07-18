#!/usr/bin/env bash
# scripts/qemu_boot.sh — Build & boot the VWK kernel
#
# Usage:
#   ./scripts/qemu_boot.sh [backend] [interactive|pipe] [--help]
#
# Examples:
#   ./scripts/qemu_boot.sh x86_64 interactive      # Interactive shell (x86_64)
#   ./scripts/qemu_boot.sh aarch64 interactive     # Interactive shell (aarch64)
#   ./scripts/qemu_boot.sh riscv64 interactive     # Interactive shell (RISC-V)
#   ./scripts/qemu_boot.sh aarch64                 # Pipe a default command script
#   ./scripts/qemu_boot.sh                         # Defaults: x86_64 pipe
#
# ── Native vs. QEMU execution ──────────────────────────────────────────
# The VUMA backends emit a normal Linux ELF (ET_EXEC) userspace binary.
# When the requested backend matches the host CPU (e.g. requesting
# `aarch64` on a Raspberry Pi 4/5), the kernel is executed DIRECTLY —
# no QEMU is needed and no emulation overhead is paid.
# When the backend targets a foreign architecture, QEMU user-mode
# (/usr/local/bin/qemu-<arch>) is used if present.
#
# ── sudo note ──────────────────────────────────────────────────────────
# sudo is NOT required and NOT recommended — user-mode execution does
# not need root. If you do run this under `sudo bash`, the script will
# still locate your real user's Rust toolchain via $SUDO_USER so the
# build works. Prefer running it directly:
#     ./scripts/qemu_boot.sh aarch64 interactive
#
# ── Prerequisites ──────────────────────────────────────────────────────
#   * Rust toolchain (rustup) — `curl --proto '=https' --tlsv1.2 -sSf \
#       https://sh.rustup.rs | sh` — the pinned nightly is selected
#       automatically by rust-toolchain.toml.
#   * (only for foreign arches) qemu-user-static binaries in /usr/local/bin.

set -uo pipefail

# ── Help ───────────────────────────────────────────────────────────────
if [ "${1:-}" = "--help" ] || [ "${1:-}" = "-h" ]; then
    sed -n '2,/^set -uo pipefail$/p' "$0" | sed -e 's/^# \?//' -e '/^set -uo pipefail$/d'
    exit 0
fi

# ── Locate the repo root from this script's path (portable) ────────────
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$SCRIPT_DIR/.." && pwd)"
cd "$REPO" || { echo "[qemu] FAIL: cannot cd to repo root $REPO" >&2; exit 1; }

# ── Locate the cargo / rustc toolchain ─────────────────────────────────
#   1. Already on PATH?                            -> use it
#   2. $HOME/.cargo/env exists?                    -> source it
#   3. Running under sudo? Try real user's home    -> source it
#   4. /usr/local/cargo/env ?                      -> source it
#   Otherwise emit a clear install hint.
find_cargo_env() {
    if command -v cargo >/dev/null 2>&1; then
        return 0
    fi
    local -a candidates=( "$HOME/.cargo/env" )
    if [ -n "${SUDO_USER:-}" ]; then
        local real_home
        real_home="$(getent passwd "$SUDO_USER" 2>/dev/null | cut -d: -f6)"
        [ -n "$real_home" ] && candidates+=("$real_home/.cargo/env")
    fi
    candidates+=("/usr/local/cargo/env")
    local tried=()
    for c in "${candidates[@]}"; do
        tried+=("$c")
        if [ -f "$c" ]; then
            # shellcheck disable=SC1090
            source "$c"
            if command -v cargo >/dev/null 2>&1; then
                return 0
            fi
        fi
    done
    echo "[qemu] ERROR: cargo not found on PATH." >&2
    echo "[qemu]        Tried sourcing: ${tried[*]}" >&2
    if [ -n "${SUDO_USER:-}" ]; then
        echo "[qemu]        (running under sudo as user '$SUDO_USER';" >&2
        echo "[qemu]         consider running WITHOUT sudo)" >&2
    fi
    echo "[qemu]        Install Rust via:" >&2
    echo "[qemu]          curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh" >&2
    return 1
}
find_cargo_env || exit 1

# Warn (but do not abort) if running under sudo — it is almost never needed.
if [ "$(id -u)" = "0" ] && [ -z "${VUMA_ALLOW_SUDO:-}" ]; then
    echo "[qemu] NOTE: running as root (sudo). User-mode kernel boot does not" >&2
    echo "[qemu]       need root. Tip: re-run without sudo for a cleaner setup." >&2
    echo "[qemu]       (set VUMA_ALLOW_SUDO=1 to silence this note.)" >&2
fi

BIN="./target/release-fast/compile_dump"
BACKEND="${1:-x86_64}"
MODE="${2:-pipe}"

# ── Build compile_dump if missing or out of date ───────────────────────
needs_build=0
if [ ! -x "$BIN" ]; then
    needs_build=1
else
    # Rebuild if any tracked input is newer than the binary.
    for f in Cargo.toml rust-toolchain.toml build.rs src/bin/compile_dump.rs; do
        if [ -e "$f" ] && [ "$f" -nt "$BIN" ]; then
            needs_build=1; break
        fi
    done
fi

if [ "$needs_build" = "1" ]; then
    echo "[qemu] building compile_dump (profile=release-fast)..."
    BUILD_OUT="$(cargo build --profile release-fast --bin compile_dump 2>&1)"
    BUILD_RC=$?
    if [ "$BUILD_RC" -ne 0 ] || [ ! -x "$BIN" ]; then
        echo "[qemu] FAIL: cargo build exited $BUILD_RC. Full output:" >&2
        echo "$BUILD_OUT" >&2
        exit 1
    fi
    echo "$BUILD_OUT" | tail -n 4
fi

# ── Compile the kernel ─────────────────────────────────────────────────
echo "[qemu] compiling kernel for $BACKEND..."
COMPILE_OUT="$("$BIN" womb/kernel/kernel.vuma /tmp/vwk_kernel.bin "$BACKEND" --verify 2>&1)"
COMPILE_RC=$?
if [ "$COMPILE_RC" -ne 0 ] || [ ! -f /tmp/vwk_kernel.bin ]; then
    echo "[qemu] FAIL: kernel compile exited $COMPILE_RC. Full output:" >&2
    echo "$COMPILE_OUT" >&2
    exit 1
fi
echo "$COMPILE_OUT" | tail -n 4

# File size (Linux stat -c%s, BSD/macOS stat -f%z)
KSIZE="$(stat -c%s /tmp/vwk_kernel.bin 2>/dev/null || stat -f%z /tmp/vwk_kernel.bin 2>/dev/null || echo '?')"
echo "[qemu] kernel compiled: $KSIZE bytes -> /tmp/vwk_kernel.bin"
echo ""

# ── Map backend → (qemu-user binary | native uname -m) ──────────────────
# native_arch: if `uname -m` equals this, run the kernel DIRECTLY (no QEMU).
get_qemu_user() {
    case "$1" in
        x86_64)        echo "";;
        aarch64)       echo "/usr/local/bin/qemu-aarch64";;
        aarch64_be)    echo "/usr/local/bin/qemu-aarch64_be";;
        riscv64)       echo "/usr/local/bin/qemu-riscv64";;
        riscv32)       echo "/usr/local/bin/qemu-riscv32";;
        arm32|arm)     echo "/usr/local/bin/qemu-arm";;
        armeb)         echo "/usr/local/bin/qemu-armeb";;
        ppc64)         echo "/usr/local/bin/qemu-ppc64";;
        ppc64le)       echo "/usr/local/bin/qemu-ppc64le";;
        loongarch64)   echo "/usr/local/bin/qemu-loongarch64";;
        mips64)        echo "/usr/local/bin/qemu-mips64el";;
        mips64be)      echo "/usr/local/bin/qemu-mips64";;
        s390x)         echo "/usr/local/bin/qemu-s390x";;
        sparc64)       echo "/usr/local/bin/qemu-sparc64";;
        alpha)         echo "/usr/local/bin/qemu-alpha";;
        hppa)          echo "/usr/local/bin/qemu-hppa";;
        m68k)          echo "/usr/local/bin/qemu-m68k";;
        x86_32|i386)   echo "/usr/local/bin/qemu-i386";;
        wasm32|wasm)   echo "";;
        *)             echo "";;
    esac
}
# Host uname -m values that can run a given backend's ELF natively.
get_native_arch() {
    case "$1" in
        x86_64)        echo "x86_64";;
        aarch64)       echo "aarch64";;
        riscv64)       echo "riscv64";;
        ppc64le)       echo "ppc64le";;
        ppc64)         echo "ppc64";;
        s390x)         echo "s390x";;
        arm32|arm)     echo "armv7l";;
        *)             echo "";;
    esac
}

HOST_ARCH="$(uname -m)"
QEMU_BIN="$(get_qemu_user "$BACKEND")"
NATIVE_ARCH="$(get_native_arch "$BACKEND")"

if [ "$BACKEND" = "wasm32" ] || [ "$BACKEND" = "wasm" ]; then
    echo "[qemu] wasm32 backend — run the output with:" >&2
    echo "  wasmtime /tmp/vwk_kernel.bin" >&2
    exit 0
fi

# Decide how to run the kernel.
RUNNER=""
if [ -n "$NATIVE_ARCH" ] && [ "$HOST_ARCH" = "$NATIVE_ARCH" ]; then
    # Native execution — no QEMU needed (e.g. aarch64 on a Raspberry Pi).
    RUNNER=""
    echo "[qemu] native execution (host=$HOST_ARCH, target=$BACKEND) — no QEMU needed"
elif [ -n "$QEMU_BIN" ] && [ -x "$QEMU_BIN" ]; then
    RUNNER="$QEMU_BIN"
    echo "[qemu] using QEMU user-mode: $QEMU_BIN"
else
    echo "[qemu] No QEMU binary for '$BACKEND' and not a native arch (host=$HOST_ARCH)." >&2
    echo "[qemu] Install qemu-user-static, or run on a matching host." >&2
    exit 1
fi

# Best-effort ELF sanity check (optional; `file` may be absent).
if command -v file >/dev/null 2>&1; then
    case "$(file -b /tmp/vwk_kernel.bin 2>/dev/null)" in
        *ELF*) : ;;  # looks like an ELF — fine
        *)
            echo "[qemu] WARN: /tmp/vwk_kernel.bin does not look like an ELF:" >&2
            file /tmp/vwk_kernel.bin >&2
            ;;
    esac
fi

echo "[qemu] booting VWK kernel ($BACKEND)..."
echo "========================================"

run_kernel() {
    if [ -z "$RUNNER" ]; then
        /tmp/vwk_kernel.bin
    else
        "$RUNNER" /tmp/vwk_kernel.bin
    fi
}

if [ "$MODE" = "interactive" ]; then
    # Interactive mode: connect the terminal's stdin/stdout directly.
    run_kernel
else
    # Pipe mode: feed a default command sequence.
    printf '%s\n' \
        "help" "ls" "touch hello.txt" "ls" "cat hello.txt" \
        "mkdir docs" "ls" "pid" "ps" "ver" "memstat" \
        "echo VWK is alive" "alloc" "exit" \
    | run_kernel
fi

EXIT_CODE=$?
echo "========================================"
echo "[qemu] kernel exited with code $EXIT_CODE"
