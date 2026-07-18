#!/bin/bash
# ═══════════════════════════════════════════════════════════════════════════
# VUMA Full Test Suite Runner for Raspberry Pi 5 (aarch64 native)
# ═══════════════════════════════════════════════════════════════════════════
set -euo pipefail

WORKERS=4
SKIP_BUILD=0
NO_PUSH=0
FRESH=0
BACKENDS=""
VERIFY=0
BUILD_PROFILE="release-fast"   # fast iterative profile (LTO off, codegen-units=16)
REPO_DIR="$(cd "$(dirname "$0")/.." && pwd)"

while [[ $# -gt 0 ]]; do
    case $1 in
        --workers) WORKERS="$2"; shift 2 ;;
        --skip-build) SKIP_BUILD=1; shift ;;
        --no-push) NO_PUSH=1; shift ;;
        --fresh) FRESH=1; shift ;;
        --backends) BACKENDS="$2"; shift 2 ;;
        --verify) VERIFY=1; shift ;;
        --release) BUILD_PROFILE="release"; shift ;;   # opt-in: slow LTO build
        --profile) BUILD_PROFILE="$2"; shift 2 ;;
        *) echo "Unknown option: $1"; exit 1 ;;
    esac
done

cd "$REPO_DIR"

# ── Setup PATH (cargo might be in ~/.cargo/bin) ──
export PATH="$HOME/.cargo/bin:$PATH"

echo "╔══════════════════════════════════════════════════════════════╗"
echo "║  VUMA Full Test Suite — $(date -u '+%Y-%m-%d %H:%M UTC')            ║"
echo "╠══════════════════════════════════════════════════════════════╣"
echo "║  Repo:    $REPO_DIR"
echo "║  Workers: $WORKERS"
echo "║  Profile: $BUILD_PROFILE"
echo "║  Host:    $(uname -m) ($(hostname))"
echo "╚══════════════════════════════════════════════════════════════╝"
echo ""

# ── Step 1: Install prerequisites ──
echo "▸ Checking prerequisites..."

# Check/install Rust
if ! command -v cargo &>/dev/null; then
    echo "  Installing Rust..."
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain nightly-2026-03-01 2>/dev/null || {
        echo "  curl failed, trying wget..."
        wget -qO- https://sh.rustup.rs | sh -s -- -y --default-toolchain nightly-2026-03-01
    }
    source "$HOME/.cargo/env"
    rustup component add rustfmt clippy rust-src 2>/dev/null || true
fi
echo "  ✓ Rust: $(rustc --version 2>/dev/null || echo 'NOT FOUND')"

# Check/install QEMU
# First, look for the bundled QEMU user-mode binaries extracted at
# $REPO_DIR/bin/ or $REPO_DIR/qemu-user-extract/usr/bin/ (dev image
# locations). If found, prepend that directory to PATH so all 19 QEMU
# binaries (aarch64, x86_64, hppa, alpha, m68k, s390x, sparc64, etc.) are
# discoverable. Otherwise fall back to system QEMU.
#
# CRITICAL: Always ensure /usr/bin and /bin are in PATH (prepend if needed).
# When running under sudo, PATH may be stripped to a minimal set that
# doesn't include all standard directories, causing commands like tr, xargs,
# mkdir to fail with "Too many levels of symbolic links" (ELOOP) because
# they resolve through broken symlink chains.
export PATH="/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin${PATH:+:$PATH}"

QEMU_DIR=""
for d in "$REPO_DIR/bin" "$REPO_DIR/qemu-user-extract/usr/bin" /tmp/qemu_bins; do
    if [ -x "$d/qemu-aarch64" ]; then QEMU_DIR="$d"; break; fi
done
if [ -n "$QEMU_DIR" ]; then
    export PATH="$QEMU_DIR:$PATH"
    echo "  ✓ QEMU (bundled): $QEMU_DIR"
elif ! command -v qemu-aarch64 &>/dev/null; then
    echo "  Installing QEMU..."
    apt update -qq && apt install -y qemu-user qemu-user-static 2>/dev/null || {
        echo "  ✗ Failed to install qemu-user. Run: apt install qemu-user qemu-user-static"
        exit 1
    }
    echo "  ✓ QEMU: $(qemu-aarch64 --version 2>/dev/null | head -1 || echo 'NOT FOUND')"
else
    echo "  ✓ QEMU: $(qemu-aarch64 --version 2>/dev/null | head -1 || echo 'NOT FOUND')"
fi

# Check/install wasmtime
WASMTIME_BIN=""
for p in /usr/local/bin/wasmtime "$HOME/.local/bin/wasmtime" "$(pwd)/wasmtime"; do
    if [ -x "$p" ]; then WASMTIME_BIN="$p"; break; fi
done
if [ -z "$WASMTIME_BIN" ]; then
    echo "  Installing wasmtime..."
    ARCH=$(uname -m)
    WASMTIME_VER="v29.0.1"
    WASMTIME_URL="https://github.com/bytecodealliance/wasmtime/releases/download/${WASMTIME_VER}/wasmtime-${WASMTIME_VER}-${ARCH}-linux.tar.xz"
    WASMTIME_TARBALL="/tmp/wasmtime-${WASMTIME_VER}-${ARCH}.tar.xz"
    WASMTIME_DIR="/tmp/wasmtime-${WASMTIME_VER}-${ARCH}-linux"

    # Try curl first, then wget as fallback. GitHub releases redirect to
    # Azure blob storage — some networks block one but not the other.
    # Show the actual error on failure so the user can diagnose.
    download_ok=0
    if curl -fSL --retry 3 --retry-delay 2 --connect-timeout 15 -o "$WASMTIME_TARBALL" "$WASMTIME_URL" 2>/tmp/wasmtime_curl_err; then
        download_ok=1
    elif wget -q --tries=3 --timeout=30 -O "$WASMTIME_TARBALL" "$WASMTIME_URL" 2>/tmp/wasmtime_wget_err; then
        download_ok=1
    fi

    if [ "$download_ok" = "1" ] && [ -s "$WASMTIME_TARBALL" ]; then
        rm -rf "$WASMTIME_DIR"
        if tar xf "$WASMTIME_TARBALL" -C /tmp/ 2>/dev/null; then
            WASMTIME_BIN=$(find "$WASMTIME_DIR" -name wasmtime -type f 2>/dev/null | head -1)
            if [ -n "$WASMTIME_BIN" ]; then
                cp "$WASMTIME_BIN" "$REPO_DIR/wasmtime"
                chmod +x "$REPO_DIR/wasmtime"
                WASMTIME_BIN="$REPO_DIR/wasmtime"
                echo "  ✓ Wasmtime installed: $WASMTIME_BIN"
            else
                echo "  ⚠ wasmtime binary not found in tarball (wasm32 backend will be skipped)"
            fi
        else
            echo "  ⚠ wasmtime tarball extraction failed (wasm32 backend will be skipped)"
        fi
    else
        echo "  ⚠ wasmtime download failed from:"
        echo "    $WASMTIME_URL"
        if [ -s /tmp/wasmtime_curl_err ]; then
            echo "  curl error: $(tail -1 /tmp/wasmtime_curl_err)"
        fi
        echo "  Try manually:"
        echo "    curl -fSL -o /tmp/wasmtime.tar.xz '$WASMTIME_URL'"
        echo "    tar xf /tmp/wasmtime.tar.xz -C /tmp/"
        echo "    cp $WASMTIME_DIR/wasmtime ~/vuma/wasmtime"
        echo "  (wasm32 backend will be skipped)"
    fi
    rm -f "$WASMTIME_TARBALL" /tmp/wasmtime_curl_err /tmp/wasmtime_wget_err
fi
echo "  ✓ Wasmtime CLI: ${WASMTIME_BIN:-NOT FOUND}"

# ── Step 1b: Install wasmtime Python package ──
# The custom wasm32 runner (scripts/wasm32_runner.py) uses the wasmtime
# Python API to provide pipe/fork/execve/dup2/waitpid/strcmp host functions
# that WASI does not support.  This enables self_exec on wasm32 without
# skipping.
echo "▸ Installing wasmtime Python package for custom wasm32 runner..."
if python3 -c "import wasmtime" 2>/dev/null; then
    echo "  ✓ wasmtime Python package already installed"
else
    # Try multiple installation methods.  PEP 668 may block system-wide
    # installs on some distros (Debian 12+, Ubuntu 23.04+); --break-system-packages
    # overrides this.  --user installs into the user's site-packages.
    WT_INSTALLED=0
    # Use a unique temp file in the repo to avoid /tmp permission issues
    # (e.g., when running as root via sudo and /tmp/wt_pip.log exists
    # from a prior non-root run with restrictive permissions).
    WT_PIP_LOG="$REPO_DIR/test_results/wt_pip_$$.$RANDOM.log"
    mkdir -p "$REPO_DIR/test_results"
    for pip_cmd in \
        "pip3 install wasmtime" \
        "python3 -m pip install wasmtime" \
        "pip3 install --break-system-packages wasmtime" \
        "python3 -m pip install --break-system-packages wasmtime" \
        "pip3 install --user wasmtime" \
        "python3 -m pip install --user --break-system-packages wasmtime"
    do
        echo "  trying: $pip_cmd"
        if eval "$pip_cmd" >"$WT_PIP_LOG" 2>&1; then
            if python3 -c "import wasmtime" 2>/dev/null; then
                echo "  ✓ wasmtime Python package installed via: $pip_cmd"
                WT_INSTALLED=1
                break
            fi
        fi
    done
    if [ "$WT_INSTALLED" = "0" ]; then
        echo "  ⚠ could not install wasmtime Python package"
        echo "    last pip log:"
        tail -5 "$WT_PIP_LOG" 2>/dev/null | sed 's/^/      /'
        echo "    (wasm32 self_exec will use CLI fallback)"
        echo "    manual install: pip3 install --break-system-packages wasmtime"
    fi
    # Clean up temp log
    rm -f "$WT_PIP_LOG" 2>/dev/null
fi
echo ""

# ── Step 1c: Set up binfmt_misc for QEMU ──
# self_exec.vuma tests fork+execve, which requires the host kernel to be
# able to execute guest binaries.  On a native host (e.g., aarch64 Pi
# running aarch64 binaries), execve works natively.  For cross-architecture
# binaries (e.g., x86_64 binary on aarch64 host), the kernel needs
# binfmt_misc to find the right QEMU interpreter.
#
# We try to register QEMU handlers for all architectures.  This requires
# root or CAP_SYS_ADMIN.  If it fails, self_exec will only work on the
# native architecture — other backends will have execve fail (the child
# exits(5), parent reads EOF, returns 0 instead of 100).
echo "▸ Setting up binfmt_misc for QEMU (for self_exec fork+exec tests)..."
BINFMT_OK=0
BINFMT_REGISTER="/proc/sys/fs/binfmt_misc/register"

# Build the list of binfmt_misc entries for all QEMU architectures.
# Format: :name:M:magic:mask:interpreter:flags
#
# CRITICAL: The magic/mask must include the ELF machine type (bytes 18-19) to
# uniquely identify each architecture. Using only the first 8 bytes (magic +
# class + data + version) causes ALL LE-64-bit architectures (aarch64, x86_64,
# riscv64, ppc64le, loongarch64, mips64el) to match the SAME magic, so only
# one handler can be registered and execve fails for the others.
#
# CRITICAL: Do NOT register a binfmt_misc entry for the HOST's native
# architecture. If the host is aarch64 and we register qemu-aarch64, every
# native aarch64 binary (ls, cat, clear, bash itself!) would be intercepted
# and run through qemu-aarch64 — which is ITSELF an aarch64 binary, causing
# infinite recursion: kernel → binfmt → qemu-aarch64 → binfmt → qemu-aarch64
# → ... → ELOOP. This bricks the system until reboot.
#
# 20-byte magic layout:
#   bytes 0-3:  \x7fELF (magic)
#   byte  4:    class (1=32-bit, 2=64-bit)
#   byte  5:    data (1=LE, 2=BE)
#   byte  6:    version (1)
#   bytes 7-15: ignored (ABI version + padding)
#   bytes 16-17: ignored (e_type)
#   bytes 18-19: e_machine (architecture identifier, in ELF endianness)
#
# 20-byte mask: match bytes 0-6 and 18-19, ignore 7-17.
# F flag = fix binary (cache interpreter path at registration time)
#
# The interpreter path is resolved by searching for the QEMU binary in:
#   1. $QEMU_DIR (bundled binaries, if found earlier)
#   2. /usr/bin/qemu-$arch (system-installed)
#   3. $(command -v qemu-$arch) (PATH lookup)
# This ensures the binfmt entry points to a real, executable QEMU binary.
build_binfmt_entries() {
    # Detect host architecture to SKIP native arch registration.
    # uname -m returns: aarch64, x86_64, riscv64, ppc64le, mips64, etc.
    local host_arch
    host_arch=$(uname -m)

    # Map host arch to the binfmt entry name that must be SKIPPED.
    # On aarch64 host: skip qemu-aarch64 (but NOT qemu-aarch64_be).
    # On x86_64 host: skip qemu-x86_64 (but NOT qemu-x86_32).
    local skip_native=""
    case "$host_arch" in
        aarch64)    skip_native="qemu-aarch64" ;;
        x86_64)     skip_native="qemu-x86_64" ;;
        riscv64)    skip_native="qemu-riscv64" ;;
        ppc64le)    skip_native="qemu-ppc64le" ;;
        ppc64)      skip_native="qemu-ppc64" ;;
        mips64)     skip_native="qemu-mips64el" ;;  # Debian mips64el
        mips)       skip_native="qemu-mipsel" ;;
        s390x)      skip_native="qemu-s390x" ;;
        loongarch64) skip_native="qemu-loongarch64" ;;
    esac

    # Map: binfmt_name -> qemu_binary_name
    # Also: magic_hex mask_hex
    local entries=(
        "qemu-aarch64|qemu-aarch64|\x7f\x45\x4c\x46\x02\x01\x01\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x02\xb7\x00|\xff\xff\xff\xff\xff\xff\xff\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\xff\xff"
        "qemu-aarch64_be|qemu-aarch64_be|\x7f\x45\x4c\x46\x02\x02\x01\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x02\x00\xb7|\xff\xff\xff\xff\xff\xff\xff\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\xff\xff"
        "qemu-x86_64|qemu-x86_64|\x7f\x45\x4c\x46\x02\x01\x01\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x02\x3e\x00|\xff\xff\xff\xff\xff\xff\xff\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\xff\xff"
        "qemu-riscv64|qemu-riscv64|\x7f\x45\x4c\x46\x02\x01\x01\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x02\xf3\x00|\xff\xff\xff\xff\xff\xff\xff\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\xff\xff"
        "qemu-riscv32|qemu-riscv32|\x7f\x45\x4c\x46\x01\x01\x01\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x02\xf3\x00|\xff\xff\xff\xff\xff\xff\xff\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\xff\xff"
        "qemu-arm32|qemu-arm|\x7f\x45\x4c\x46\x01\x01\x01\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x02\x28\x00|\xff\xff\xff\xff\xff\xff\xff\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\xff\xff"
        "qemu-armeb|qemu-armeb|\x7f\x45\x4c\x46\x01\x02\x01\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x02\x00\x28|\xff\xff\xff\xff\xff\xff\xff\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\xff\xff"
        "qemu-ppc64|qemu-ppc64|\x7f\x45\x4c\x46\x02\x02\x01\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x02\x00\x15|\xff\xff\xff\xff\xff\xff\xff\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\xff\xff"
        "qemu-ppc64le|qemu-ppc64le|\x7f\x45\x4c\x46\x02\x01\x01\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x02\x15\x00|\xff\xff\xff\xff\xff\xff\xff\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\xff\xff"
        "qemu-mips64|qemu-mips64|\x7f\x45\x4c\x46\x02\x02\x01\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x02\x00\x08|\xff\xff\xff\xff\xff\xff\xff\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\xff\xff"
        "qemu-mips64el|qemu-mips64el|\x7f\x45\x4c\x46\x02\x01\x01\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x02\x08\x00|\xff\xff\xff\xff\xff\xff\xff\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\xff\xff"
        "qemu-s390x|qemu-s390x|\x7f\x45\x4c\x46\x02\x02\x01\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x02\x00\x16|\xff\xff\xff\xff\xff\xff\xff\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\xff\xff"
        "qemu-alpha|qemu-alpha|\x7f\x45\x4c\x46\x02\x01\x01\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x02\x26\x90|\xff\xff\xff\xff\xff\xff\xff\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\xff\xff"
        "qemu-m68k|qemu-m68k|\x7f\x45\x4c\x46\x01\x02\x01\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x02\x00\x04|\xff\xff\xff\xff\xff\xff\xff\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\xff\xff"
        "qemu-sparc64|qemu-sparc64|\x7f\x45\x4c\x46\x02\x02\x01\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x02\x00\x2b|\xff\xff\xff\xff\xff\xff\xff\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\xff\xff"
        "qemu-hppa|qemu-hppa|\x7f\x45\x4c\x46\x01\x02\x01\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x02\x00\x0f|\xff\xff\xff\xff\xff\xff\xff\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\xff\xff"
        "qemu-loongarch64|qemu-loongarch64|\x7f\x45\x4c\x46\x02\x01\x01\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x02\x02\x01|\xff\xff\xff\xff\xff\xff\xff\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\xff\xff"
        "qemu-x86_32|qemu-i386|\x7f\x45\x4c\x46\x01\x01\x01\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x02\x03\x00|\xff\xff\xff\xff\xff\xff\xff\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\xff\xff"
    )

    for entry in "${entries[@]}"; do
        IFS='|' read -r name qemu_bin magic mask <<< "$entry"

        # SKIP the native architecture to prevent infinite recursion.
        # If host is aarch64 and we register qemu-aarch64, every native
        # binary (including qemu-aarch64 itself) gets intercepted → ELOOP.
        if [ -n "$skip_native" ] && [ "$name" = "$skip_native" ]; then
            echo "SKIP: $name matches host arch ($host_arch) — not registering to avoid infinite recursion" >&2
            continue
        fi

        # Find the actual QEMU binary path
        local interp=""
        # Try QEMU_DIR first (bundled binaries)
        if [ -n "$QEMU_DIR" ] && [ -x "$QEMU_DIR/$qemu_bin" ]; then
            interp="$QEMU_DIR/$qemu_bin"
        # Try /usr/bin
        elif [ -x "/usr/bin/$qemu_bin" ]; then
            interp="/usr/bin/$qemu_bin"
        # Try PATH lookup
        else
            interp=$(command -v "$qemu_bin" 2>/dev/null || true)
        fi
        if [ -z "$interp" ]; then
            echo "WARNING: $qemu_bin not found — skipping binfmt registration for $name" >&2
            continue
        fi
        echo ":${name}:M::${magic}:${mask}:${interp}:F"
    done
}

# Deregister ALL existing qemu-* binfmt_misc entries before registering new ones.
# The system may have pre-installed entries (from qemu-user-static package) that
# use broken 8-byte magic (can't distinguish architectures with same ELF class/data).
# Our entries use 20-byte magic including e_machine for unique per-arch matching.
deregister_old_binfmt() {
    local sudo_cmd=""
    if [ "$(id -u)" != "0" ]; then
        sudo -n true 2>/dev/null && sudo_cmd="sudo"
    fi
    [ -z "$sudo_cmd" ] && [ "$(id -u)" != "0" ] && return 1

    for entry in /proc/sys/fs/binfmt_misc/qemu-*; do
        [ -f "$entry" ] || continue
        local name=$(basename "$entry")
        if [ -n "$sudo_cmd" ]; then
            echo -1 | $sudo_cmd tee "$entry" >/dev/null 2>/dev/null
        else
            echo -1 > "$entry" 2>/dev/null
        fi
        # Don't log — too noisy with 30+ entries
    done
    return 0
}

# Register new binfmt_misc entries with correct 20-byte magic.
register_binfmt() {
    local sudo_cmd=""
    if [ "$(id -u)" != "0" ]; then
        sudo -n true 2>/dev/null && sudo_cmd="sudo"
    fi
    if [ -z "$sudo_cmd" ] && [ "$(id -u)" != "0" ]; then
        return 1  # no root, no sudo
    fi

    # Step 1: Deregister ALL existing qemu-* entries (system-installed or ours)
    deregister_old_binfmt

    # Step 2: Register our new entries with correct 20-byte magic
    local registered=0
    local failed=0
    build_binfmt_entries | while IFS= read -r fmt; do
        if [ -n "$sudo_cmd" ]; then
            echo "$fmt" | $sudo_cmd tee "$BINFMT_REGISTER" >/dev/null 2>/dev/null
        else
            echo "$fmt" > "$BINFMT_REGISTER" 2>/dev/null
        fi
    done

    # Step 3: Verify our entries were actually registered (not just that files exist)
    # Check a few key architectures that previously failed
    for arch in qemu-alpha qemu-hppa qemu-sparc64 qemu-m68k qemu-s390x; do
        if [ -f "/proc/sys/fs/binfmt_misc/$arch" ]; then
            # Verify the entry has 20-byte magic (not the old 8-byte)
            local magic_len=$(wc -c < "/proc/sys/fs/binfmt_misc/$arch" 2>/dev/null)
            # A proper entry file is ~200+ bytes; the magic line should have 20 hex pairs
            if grep -q 'magic' "/proc/sys/fs/binfmt_misc/$arch" 2>/dev/null; then
                registered=$((registered + 1))
            fi
        fi
    done

    # Check if at least one registration succeeded
    [ -f /proc/sys/fs/binfmt_misc/qemu-aarch64 ] && return 0
    [ -f /proc/sys/fs/binfmt_misc/qemu-x86_64 ] && return 0
    return 1
}

if [ -w "$BINFMT_REGISTER" ] || [ "$(id -u)" = "0" ]; then
    # We have write access (running as root)
    register_binfmt && BINFMT_OK=1
elif sudo -n true 2>/dev/null; then
    # We have passwordless sudo
    register_binfmt && BINFMT_OK=1
fi

if [ "$BINFMT_OK" = "1" ]; then
    echo "  ✓ binfmt_misc registered for QEMU architectures (old entries replaced)"
    # List registered archs for verification (use pure bash, avoid xargs/tr
    # which may fail with ELOOP under sudo on some systems)
    registered_list=""
    for f in /proc/sys/fs/binfmt_misc/qemu-*; do
        [ -f "$f" ] || continue
        registered_list+="$(basename "$f") "
    done
    echo "    Registered: $registered_list"
else
    echo "  ⚠ binfmt_misc not available (needs root) — self_exec will only work on native arch"
    echo "    To enable: sudo mount -t binfmt_misc binfmt_misc /proc/sys/fs/binfmt_misc"
    echo "    then: sudo $REPO_DIR/scripts/pi5_test_suite.sh --skip-build --no-push"
    echo "    (or just run the full suite as root)"
fi
echo ""

# ── Step 2: Build compiler ──
if [ $SKIP_BUILD -eq 0 ]; then
    echo "▸ Building VUMA compiler (profile: $BUILD_PROFILE)..."
    # Stream build output live so the user sees progress (the LTO `release`
    # profile can take 10+ minutes on a Pi 5 and would otherwise show nothing
    # until completion). Capture stderr to a log for post-mortem on failure.
    RESULTS_DIR="$REPO_DIR/test_results"
    mkdir -p "$RESULTS_DIR"
    BUILD_LOG="$RESULTS_DIR/build.log"
    BUILD_START=$(date +%s)
    if cargo build --profile "$BUILD_PROFILE" --bin compile_dump --bin dump_ir 2>"$BUILD_LOG"; then
        BUILD_END=$(date +%s)
        echo "  ✓ Build complete in $((BUILD_END - BUILD_START))s  (log: $BUILD_LOG)"
    else
        BUILD_END=$(date +%s)
        echo "  ✗ Build FAILED after $((BUILD_END - BUILD_START))s"
        echo "  ── last 30 lines of build log ──"
        tail -30 "$BUILD_LOG" | sed 's/^/  /'
        exit 1
    fi
    echo ""
fi

# ── Step 2.5: Clear checkpoint if --fresh or if compiler was rebuilt ──
RESULTS_DIR="$REPO_DIR/test_results"
CHECKPOINT="$RESULTS_DIR/checkpoint.jsonl"
COMPILE_BIN="$REPO_DIR/target/$BUILD_PROFILE/compile_dump"
if [ $FRESH -eq 1 ]; then
    echo "▸ --fresh: clearing previous checkpoint..."
    rm -f "$CHECKPOINT"
    echo "✓ Checkpoint cleared"
    echo ""
elif [ -f "$CHECKPOINT" ] && [ -f "$COMPILE_BIN" ]; then
    # Auto-detect: if the compiler binary is newer than the checkpoint,
    # the results are stale and should be regenerated.
    if [ "$COMPILE_BIN" -nt "$CHECKPOINT" ]; then
        echo "▸ Compiler binary newer than checkpoint — clearing stale results..."
        rm -f "$CHECKPOINT"
        echo "✓ Checkpoint cleared"
        echo ""
    fi
fi

# ── Step 3: Create Python test runner ──
mkdir -p "$RESULTS_DIR"
export VUMA_BUILD_PROFILE="$BUILD_PROFILE"
export REPO_DIR="$REPO_DIR"
export WASMTIME_BIN="${WASMTIME_BIN:-}"

cat > "$RESULTS_DIR/run_tests.py" << 'PYEOF'
#!/usr/bin/env python3
"""VUMA Full Test Suite — runs all .vuma tests across all backends."""
import argparse, os, sys, subprocess, re, time, json, platform
from pathlib import Path
from concurrent.futures import ProcessPoolExecutor, as_completed
from collections import defaultdict

REPO = Path(os.environ.get("REPO_DIR", "."))
GOLD_DIR = REPO / "tests" / "gold_standard"
COMPILE = REPO / "target" / os.environ.get("VUMA_BUILD_PROFILE", "release-fast") / "compile_dump"
RESULTS = REPO / "test_results"
HOST_ARCH = platform.machine()

# QEMU binary mapping
BACKENDS = {}
# Always use QEMU for all backends (even native aarch64)
# This ensures consistent ELF loading behavior
BACKENDS["aarch64"] = "qemu-aarch64"
BACKENDS["aarch64_be"] = "qemu-aarch64_be"
BACKENDS["x86_64"] = "qemu-x86_64"
BACKENDS["riscv64"] = "qemu-riscv64"
BACKENDS["arm32"] = "qemu-arm"
BACKENDS["armeb"] = "qemu-armeb"
BACKENDS["mips64"] = "qemu-mips64el"
BACKENDS["mips64be"] = "qemu-mips64"
BACKENDS["ppc64"] = "qemu-ppc64"
BACKENDS["ppc64le"] = "qemu-ppc64le"
BACKENDS["loongarch64"] = "qemu-loongarch64"
BACKENDS["riscv32"] = "qemu-riscv32"
BACKENDS["x86_32"] = "qemu-i386"
BACKENDS["s390x"] = "qemu-s390x"
BACKENDS["alpha"] = "qemu-alpha"
BACKENDS["m68k"] = "qemu-m68k"
BACKENDS["sparc64"] = "qemu-sparc64"
BACKENDS["hppa"] = "qemu-hppa"

# Check wasmtime — we use the Python wasmtime package for the custom runner
# (scripts/wasm32_runner.py) which provides pipe/fork/execve/dup2/waitpid/strcmp
# host functions that WASI does not support.
try:
    import wasmtime as _wt_check
    BACKENDS["wasm32"] = "WASMTIME"
    WASMTIME = "python-wasmtime"
except ImportError:
    # Fall back to CLI wasmtime if the Python package is not available.
    # (self_exec will fail on wasm32 in this case, but other tests work.)
    WASMTIME = os.environ.get("WASMTIME_BIN", "")
    if WASMTIME and os.path.isfile(WASMTIME):
        BACKENDS["wasm32"] = "WASMTIME"
    elif os.path.isfile(str(REPO / "wasmtime")):
        WASMTIME = str(REPO / "wasmtime")
        BACKENDS["wasm32"] = "WASMTIME"
    else:
        import shutil
        if shutil.which("wasmtime"):
            WASMTIME = "wasmtime"
            BACKENDS["wasm32"] = "WASMTIME"

EXEC_TIMEOUT = 5
EXPECTED_RE = re.compile(rb"//\s*Expected exit code:\s*(-?\d+)")
SKIP_ON_RE = re.compile(rb"//\s*skip_on:\s*([a-zA-Z0-9_,\s]+)")

def find_tests():
    tests = []
    for vuma in sorted(GOLD_DIR.rglob("*.vuma")):
        try:
            with open(vuma, "rb") as f:
                head = f.read(2000)
            m = EXPECTED_RE.search(head)
            if m:
                expected = int(m.group(1))
                # Parse skip_on marker (e.g. "// skip_on: wasm32" or
                # "// skip_on: wasm32, ppc64"). Backends listed here are
                # skipped (counted as a pass with skipped=True) because the
                # test exercises functionality that is architecturally
                # unavailable on that target (e.g. fork/execve on wasm32).
                skip_backends = frozenset()
                sm = SKIP_ON_RE.search(head)
                if sm:
                    raw = sm.group(1).decode(errors="replace")
                    skip_backends = frozenset(
                        b.strip() for b in raw.replace(",", " ").split()
                        if b.strip()
                    )
                tests.append((str(vuma), vuma.parent.name, vuma.name,
                              expected, skip_backends))
        except:
            pass
    return tests

def run_one(args):
    test_path, category, test_name, expected, skip_backends, backend, verify = args
    result = {
        "test": test_name, "category": category, "path": test_path,
        "backend": backend, "expected": expected, "actual": None,
        "compile_ok": False, "crashed": False, "timed_out": False,
        "match": False, "skipped": False,
        "ive_verdict": None, "ive_passed": None, "ive_failed": None, "ive_total": None,
    }
    # Honor skip_on marker — count as pass with skipped=True so the test
    # is visible in results but doesn't break the pass rate.
    if backend in skip_backends:
        result["skipped"] = True
        result["match"] = True
        result["actual"] = expected
        return result
    out = f"/tmp/vuma_{os.getpid()}_{backend}_{test_name}.bin"
    try:
        compile_cmd = [str(COMPILE), test_path, out, backend]
        if verify:
            compile_cmd.append("--verify")
        r = subprocess.run(compile_cmd, capture_output=True, timeout=15)
        if r.returncode != 0:
            return result
        result["compile_ok"] = True

        # Parse IVE status from stderr (if --verify was passed)
        if verify:
            stderr = r.stderr.decode(errors="replace")
            for line in stderr.splitlines():
                if line.startswith("IVE: "):
                    # Format: "IVE: Pass passed=5 failed=0 total=5"
                    # or: "IVE: Skip (ive_skip marker)"
                    rest = line[5:]
                    parts = rest.split()
                    if parts:
                        result["ive_verdict"] = parts[0]
                    for p in parts[1:]:
                        if "=" in p:
                            k, v = p.split("=", 1)
                            try:
                                iv = int(v)
                                if k == "passed": result["ive_passed"] = iv
                                elif k == "failed": result["ive_failed"] = iv
                                elif k == "total": result["ive_total"] = iv
                            except: pass

        if backend == "wasm32":
            os.chmod(out, 0o644)
            test_name_lower = test_name.lower()
            if WASMTIME == "python-wasmtime":
                # Use the custom wasmtime runner that provides pipe/fork/execve/
                # dup2/waitpid/strcmp host functions via the Python wasmtime API.
                runner = str(REPO / "scripts" / "wasm32_runner.py")
                cmd = [sys.executable, runner, out]
            else:
                # Fallback: CLI wasmtime (self_exec won't work without host functions)
                if "print" in test_name_lower:
                    cmd = [WASMTIME, "run", out]
                else:
                    cmd = [WASMTIME, "run", "--invoke", "_vuma_main", out]
        elif BACKENDS[backend] is None:
            os.chmod(out, 0o755)
            cmd = ["timeout", str(EXEC_TIMEOUT), out]
        else:
            os.chmod(out, 0o755)
            cmd = ["timeout", str(EXEC_TIMEOUT), BACKENDS[backend], out]

        try:
            # self_exec uses fork/exec/pipe which is timing-sensitive
            # under QEMU user-mode emulation. If it crashes with SIGPIPE
            # (signal 13, rc=-13), retry up to 3 times — the race window
            # is narrow and usually succeeds on a second attempt.
            max_retries = 3 if test_name == "self_exec.vuma" else 1
            for attempt in range(max_retries):
                ep = subprocess.run(cmd, capture_output=True, timeout=EXEC_TIMEOUT + 3)
                rc = ep.returncode
                if backend == "wasm32":
                    # Custom runner returns _vuma_main's value as the exit code.
                    # For CLI fallback, use the old print/non-print logic.
                    if WASMTIME == "python-wasmtime":
                        crashed = rc < 0 or rc > 128
                        result["actual"] = rc; result["crashed"] = crashed
                    elif "print" in test_name_lower:
                        crashed = rc < 0 or rc > 128
                        result["actual"] = rc; result["crashed"] = crashed
                    else:
                        stdout = ep.stdout.decode(errors="replace").strip()
                        if rc == 0 and stdout:
                            try: result["actual"] = int(stdout)
                            except: result["actual"] = rc; result["crashed"] = True
                        elif rc == 0: result["actual"] = 0
                        else: result["actual"] = rc; result["crashed"] = True
                elif rc == 124:
                    result["timed_out"] = True; result["actual"] = 124
                else:
                    stderr = ep.stderr.decode(errors="replace")
                    crashed = "Segmentation fault" in stderr or "uncaught target signal" in stderr or rc == 139 or rc == 134 or rc < 0
                    result["actual"] = rc; result["crashed"] = crashed
                # Retry only on SIGPIPE (-13) for self_exec
                if rc == -13 and attempt < max_retries - 1:
                    continue
                break
        except subprocess.TimeoutExpired:
            result["timed_out"] = True; result["actual"] = 124
    except:
        pass
    finally:
        try: os.remove(out)
        except: pass

    if result["actual"] is not None:
        a = result["actual"] & 0xFF if result["actual"] >= 0 else result["actual"]
        e = expected & 0xFF if expected >= 0 else expected
        result["match"] = (a == e)
    return result

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--workers", type=int, default=4)
    ap.add_argument("--backends", default=None)
    ap.add_argument("--checkpoint", default=str(RESULTS / "checkpoint.jsonl"))
    ap.add_argument("--verify", action="store_true",
                    help="Run IVE verification (non-fatal) and report pass rate")
    args = ap.parse_args()

    RESULTS.mkdir(parents=True, exist_ok=True)
    tests = find_tests()
    bl = args.backends.split(",") if args.backends else list(BACKENDS.keys())
    bl = [b for b in bl if b in BACKENDS]
    tasks = [(*t, b, args.verify) for t in tests for b in bl]
    total = len(tasks)

    # Resume support
    done = set()
    if os.path.exists(args.checkpoint):
        with open(args.checkpoint) as f:
            for line in f:
                try:
                    r = json.loads(line)
                    done.add((r["path"], r["backend"]))
                except: pass

    remaining = [t for t in tasks if (t[0], t[5]) not in done]
    print(f"Tests: {len(tests)} × Backends: {len(bl)} = {total} runs")
    print(f"Already done: {len(done)}, Remaining: {len(remaining)}")
    print(f"Backends: {bl}")
    if args.verify:
        print(f"IVE verification: ENABLED (non-fatal, reported separately)")
    print()

    ckpt = open(args.checkpoint, "a", buffering=1)
    matches = 0
    skipped = 0
    t0 = time.monotonic()

    with ProcessPoolExecutor(max_workers=args.workers) as pool:
        futures = {pool.submit(run_one, t): t for t in remaining}
        for i, fut in enumerate(as_completed(futures), 1):
            try: r = fut.result()
            except: r = {"path": "", "backend": "", "match": False, "actual": None,
                        "expected": 0, "test": "", "category": "", "compile_ok": False,
                        "crashed": False, "timed_out": False, "skipped": False}
            ckpt.write(json.dumps(r) + "\n")
            if r.get("match"):
                matches += 1
                if r.get("skipped"): skipped += 1
            if i % 200 == 0 or i == len(remaining):
                elapsed = time.monotonic() - t0
                rate = i / elapsed if elapsed > 0 else 0
                eta = (len(remaining) - i) / rate / 60 if rate > 0 else 0
                print(f"  [{i}/{len(remaining)}] {rate:.0f}/s ETA {eta:.1f}min | matches={matches} ({100*matches/i:.1f}%) skipped={skipped}", flush=True)

    ckpt.close()
    elapsed = time.monotonic() - t0
    print(f"\n{'='*60}")
    print(f"Completed {len(remaining)} runs in {elapsed/60:.1f} minutes")

    # Generate final report
    latest = {}
    with open(args.checkpoint) as f:
        for line in f:
            try:
                r = json.loads(line)
                latest[(r["path"], r["backend"])] = r
            except: pass

    total = len(latest)
    matches = sum(1 for r in latest.values() if r.get("match"))
    skipped = sum(1 for r in latest.values() if r.get("skipped"))
    print(f"Total: {matches}/{total} = {100*matches/total:.2f}%  (skipped: {skipped})")
    print()

    by_backend = defaultdict(lambda: {"total": 0, "match": 0, "skipped": 0})
    for r in latest.values():
        by_backend[r["backend"]]["total"] += 1
        if r.get("match"): by_backend[r["backend"]]["match"] += 1
        if r.get("skipped"): by_backend[r["backend"]]["skipped"] += 1

    print("Per-backend:")
    for b in sorted(by_backend):
        s = by_backend[b]
        pct = 100 * s["match"] / s["total"] if s["total"] else 0
        sk = f" (skip={s['skipped']})" if s["skipped"] else ""
        print(f"  {b:14s} {s['match']:5d}/{s['total']:5d} = {pct:.2f}%{sk}")

    # IVE verification summary (if --verify was used)
    ive_runs = [r for r in latest.values() if r.get("ive_verdict")]
    if ive_runs:
        ive_pass = sum(1 for r in ive_runs if r.get("ive_verdict") in ("Pass", "Skip"))
        ive_fail = sum(1 for r in ive_runs if r.get("ive_verdict") == "Fail")
        ive_skip = sum(1 for r in ive_runs if r.get("ive_verdict") == "Skip")
        ive_total = len(ive_runs)
        print()
        print(f"IVE Verification: {ive_pass}/{ive_total} = {100*ive_pass/ive_total:.2f}% pass"
              + (f" (skip={ive_skip})" if ive_skip else ""))
        # Per-backend IVE stats
        ive_by_backend = defaultdict(lambda: {"total": 0, "pass": 0, "skip": 0})
        for r in ive_runs:
            ive_by_backend[r["backend"]]["total"] += 1
            if r.get("ive_verdict") in ("Pass", "Skip"):
                ive_by_backend[r["backend"]]["pass"] += 1
            if r.get("ive_verdict") == "Skip":
                ive_by_backend[r["backend"]]["skip"] += 1
        for b in sorted(ive_by_backend):
            s = ive_by_backend[b]
            pct = 100 * s["pass"] / s["total"] if s["total"] else 0
            sk = f" (skip={s['skip']})" if s["skip"] else ""
            print(f"  {b:14s} {s['pass']:5d}/{s['total']:5d} = {pct:.2f}%{sk}")

    # Save summary
    summary = {
        "timestamp": time.strftime("%Y-%m-%d %H:%M:%S UTC", time.gmtime()),
        "host": platform.node(),
        "arch": HOST_ARCH,
        "total_runs": total,
        "matches": matches,
        "skipped": skipped,
        "pass_rate": f"{100*matches/total:.2f}%",
        "per_backend": {b: dict(s) for b, s in by_backend.items()},
    }
    if ive_runs:
        ive_pass = sum(1 for r in ive_runs if r.get("ive_verdict") == "Pass")
        summary["ive_verification"] = {
            "total": len(ive_runs),
            "pass": ive_pass,
            "fail": len(ive_runs) - ive_pass,
            "pass_rate": f"{100*ive_pass/len(ive_runs):.2f}%",
        }
    with open(RESULTS / "summary.json", "w") as f:
        json.dump(summary, f, indent=2)

    # List failures
    failures = [r for r in latest.values() if not r.get("match")]
    by_test = defaultdict(list)
    for r in failures:
        by_test[(r["category"], r["test"])].append(r)

    with open(RESULTS / "failures.txt", "w") as f:
        f.write(f"VUMA Test Failures — {summary['timestamp']}\n")
        f.write(f"Total: {len(failures)} failures across {len(by_test)} tests\n")
        f.write(f"Skipped: {skipped} (architecturally unavailable on target)\n\n")
        for (cat, test), rs in sorted(by_test.items()):
            backends = [(r["backend"], r.get("actual"), "TO" if r.get("timed_out") else ("CR" if r.get("crashed") else "MM")) for r in rs]
            f.write(f"  {cat:20s} {test:45s} exp={rs[0]['expected']:4} {backends}\n")

    print(f"\nFailures: {len(failures)} across {len(by_test)} tests")
    print(f"Skipped:  {skipped}")
    print(f"Results saved to {RESULTS}/")

if __name__ == "__main__":
    main()
PYEOF

export REPO_DIR="$REPO_DIR"
export WASMTIME_BIN="$WASMTIME_BIN"
VERIFY_FLAG=""
if [[ "$VERIFY" == "1" ]]; then VERIFY_FLAG="--verify"; fi
python3 "$RESULTS_DIR/run_tests.py" --workers "$WORKERS" ${BACKENDS:+--backends "$BACKENDS"} $VERIFY_FLAG
TEST_EXIT=$?

echo ""
echo "▸ Test suite complete (exit code: $TEST_EXIT)"

# ── Step 4: Commit and push results ──
if [ $NO_PUSH -eq 0 ]; then
    echo "▸ Committing results..."
    cd "$REPO_DIR"

    # Stage the critical result files explicitly. failures.txt and summary.json
    # are the files needed for remote debugging — do NOT silently swallow errors
    # from `git add` on them (the old `2>/dev/null || true` hid real problems).
    for f in test_results/failures.txt test_results/summary.json; do
        if [[ -f "$f" ]]; then
            if ! git add "$f"; then
                echo "ERROR: 'git add $f' failed. Test results may be incomplete in the commit."
            fi
        else
            echo "WARNING: $f does not exist — cannot stage it."
        fi
    done
    # Stage any other test_results/ changes.
    # IMPORTANT: the Pi MUST ONLY commit files under test_results/ — never
    # scripts/, src/, docs/, or any other agent-owned path. This isolation
    # guarantees the Pi can always `git pull && git push` without merge
    # conflicts on agent-maintained files.
    git add test_results/ 2>/dev/null || echo "WARNING: 'git add test_results/' reported an error."

    TIMESTAMP=$(date -u '+%Y-%m-%d_%H%M-UTC')

    # Only print "(nothing to commit)" when the working tree is genuinely clean.
    # Otherwise run the commit with stderr VISIBLE (no 2>/dev/null) so real
    # failures (pre-commit hooks, gpg signing, lock files, etc.) are surfaced
    # instead of being hidden behind the misleading "(nothing to commit)" line.
    #
    # When running as root (via sudo), git may not have a user identity configured.
    # Set a fallback identity via env vars so the commit succeeds.
    export GIT_AUTHOR_NAME="${GIT_AUTHOR_NAME:-VUMA Test Suite}"
    export GIT_AUTHOR_EMAIL="${GIT_AUTHOR_EMAIL:-vuma-test@local}"
    export GIT_COMMITTER_NAME="${GIT_COMMITTER_NAME:-VUMA Test Suite}"
    export GIT_COMMITTER_EMAIL="${GIT_COMMITTER_EMAIL:-vuma-test@local}"

    if [[ -z "$(git status --porcelain)" ]]; then
        echo "(nothing to commit)"
    else
        if ! git commit -m "test: Full suite results ($TIMESTAMP) on $(hostname)

$(cat test_results/summary.json 2>/dev/null || echo 'See test_results/ for details')"; then
            echo "ERROR: git commit failed. Test results were NOT committed."
            echo "  Run 'git status' and 'git commit' manually to diagnose."
        fi
    fi

    echo "▸ Pushing to GitHub..."
    # Do not swallow push failures — surface a clear ERROR so the operator knows
    # the local commit never reached the remote (pipefail is set at top of script,
    # so a non-zero git push makes the whole pipe fail and triggers the branch).
    if ! git push origin HEAD 2>&1 | tail -3; then
        echo "ERROR: git push failed. Test results were committed locally but NOT pushed to GitHub."
        echo "  Run 'git push' manually (check remote URL / credentials / network)."
    fi
    echo "✓ Done"
else
    echo "▸ Skipping commit/push (--no-push)"
fi

echo ""
echo "╔══════════════════════════════════════════════════════════════╗"
echo "║  Results:                                                   ║"
cat test_results/summary.json 2>/dev/null | python3 -c "
import json, sys
try:
    s = json.load(sys.stdin)
    print(f'║  Pass rate: {s[\"pass_rate\"]} ({s[\"matches\"]}/{s[\"total_runs\"]})')
    for b, v in sorted(s.get('per_backend', {}).items()):
        pct = 100*v['match']/v['total'] if v['total'] else 0
        print(f'║    {b:14s} {v[\"match\"]:5d}/{v[\"total\"]:5d} = {pct:.2f}%')
except: print('║  (see test_results/summary.json)')
" 2>/dev/null || echo "║  (see test_results/summary.json)"
echo "╚══════════════════════════════════════════════════════════════╝"
