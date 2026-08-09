#!/usr/bin/env bash
# W4-sym: compile + run each symmetric-cipher harness on 19 backends,
# capture stdout, parse decimal-per-line bytes, convert to hex, compare
# against expected Rust reference / standard test vectors.
set -uo pipefail
export LD_LIBRARY_PATH="${LD_LIBRARY_PATH:-}"
export LIBRARY_PATH="${LIBRARY_PATH:-}"
export C_INCLUDE_PATH="${C_INCLUDE_PATH:-}"
export CPLUS_INCLUDE_PATH="${CPLUS_INCLUDE_PATH:-}"
export PKG_CONFIG_PATH="${PKG_CONFIG_PATH:-}"
. "$HOME/.cargo/env"
. "$HOME/.local/z3-env.sh"

REPO=/home/z/my-project/vuma
TESTDIR="$REPO/tests/rust_differential"
OUTDIR=/tmp/w4_sym
mkdir -p "$OUTDIR"

BACKENDS="x86_64 x86_32 aarch64 aarch64_be arm32 armeb riscv64 riscv32 mips64 mips64be ppc64 ppc64le loongarch64 s390x sparc64 alpha hppa m68k wasm32"

# Expected output hex per algorithm (lowercase, no separators).
declare -A EXP
EXP[rc4]="bbf316e8d940af0ad3"
EXP[poly1305]="d795e0af355920ca0404c3da8782904a"
EXP[aes128]="69c4e0d86a7b0430d8cdb78070b4c55a"
EXP[aes192]="dda97ca4864cdfe06eaf70a0ec0d7191"
EXP[aes256]="8ea2b7ca516745bfeafc49904b496089"
EXP[chacha20]="c6bdf594fa87d094756b8d179a7ba25b816398cc26a334e7f7cf2720335074f1beb85c505d2d6dec471cd7ffaf002e85f3d6207bd9865fc130f6e554067f15bb7e9d9ec4be553c352466ad3fc54f03e4b3b991e755b51c76764786bab0a1023db1f0012369bfdd6661aeb325bbee22cbc13c"
EXP[des]="85e813540f0ab405"
EXP[salsa20]="4dfa5e481da23ea09a310a9fe8de6bc8e9d5d57c73f23e6b0b56292c0e9a8b3dd9ce4db7b8d49d81728f24719e6f9d3f0821e50e5cc0d0fa6e9c9a33f62c3d53"

# Rust KAT source label per algorithm.
declare -A SRC
SRC[rc4]="rust_kat_vectors.json"
SRC[poly1305]="rust_kat_vectors.json"
SRC[aes128]="rust_kat_vectors.json (aes128_ecb)"
SRC[aes192]="FIPS-197 Appendix B (no Rust KAT)"
SRC[aes256]="FIPS-197 Appendix C.3 (no Rust KAT)"
SRC[chacha20]="rust_kat_vectors.json"
SRC[des]="FIPS-81 Appendix B (no Rust KAT)"
SRC[salsa20]="DJB Salsa20 spec (no Rust KAT)"

run_one() {
    local algo="$1" backend="$2"
    local src="$TESTDIR/test_${algo}_basic.vuma"
    local bin="$OUTDIR/${algo}_${backend}.bin"
    local out="$OUTDIR/${algo}_${backend}.out"

    local compile_log
    compile_log=$("$REPO/target/release/compile_dump" "$src" "$bin" "$backend" --no-verify 2>&1)
    local compile_rc=$?
    if [ $compile_rc -ne 0 ]; then
        echo "RESULT|${algo}|${backend}|COMPILE_ERROR|0||$(echo "$compile_log" | tr '\n' ' ' | head -c 200)"
        return
    fi

    local actual_hex=""
    local exit_code=0
    if [ "$backend" = "wasm32" ]; then
        python3 "$REPO/scripts/wasm32_runner.py" "$bin" >"$out" 2>/dev/null
        exit_code=$?
    elif [ "$backend" = "x86_64" ]; then
        timeout 10 "$bin" >"$out" 2>/dev/null
        exit_code=$?
    else
        # Map VUMA backend names to QEMU user-mode binary names.
        local q
        case "$backend" in
            x86_32)       q="qemu-i386-static" ;;
            arm32)        q="qemu-arm-static" ;;
            armeb)        q="qemu-armeb-static" ;;
            mips64)       q="qemu-mips64el-static" ;;
            mips64be)     q="qemu-mips64-static" ;;
            *)            q="qemu-${backend}-static" ;;
        esac
        if ! command -v "$q" >/dev/null 2>&1; then
            echo "RESULT|${algo}|${backend}|QEMU_MISSING|0||$q"
            return
        fi
        timeout 10 "$q" "$bin" >"$out" 2>/dev/null
        exit_code=$?
    fi

    # Parse decimal output into a hex string.
    # Each byte is emitted as print_int(byte + 1000), producing a 4-digit
    # decimal (1000-1255). Newlines may or may not be present depending on
    # the backend's print_int stub. We strip ALL whitespace, then split into
    # 4-character chunks.
    actual_hex=$(python3 -c '
import sys, os
path = "'"$out"'"
data = open(path, "rb").read() if os.path.exists(path) else b""
text = data.decode("utf-8", "replace")
text = "".join(text.split())  # strip ALL whitespace
hex_str = ""
for i in range(0, len(text) - 3, 4):
    chunk = text[i:i+4]
    try:
        v = int(chunk)
    except ValueError:
        continue
    hex_str += "%02x" % ((v - 1000) & 0xFF)
print(hex_str)
')

    local expected="${EXP[$algo]}"
    local status="MISMATCH"
    if [ "$actual_hex" = "$expected" ]; then
        status="MATCH"
    fi
    local run_info="exit=$exit_code len=${#actual_hex}"
    echo "RESULT|${algo}|${backend}|${status}|${#actual_hex}|${actual_hex:0:64}|${run_info}"
}

echo "META|start|$(date -u +%Y-%m-%dT%H:%M:%SZ)"
for algo in rc4 poly1305 aes128 aes192 aes256 chacha20 des salsa20; do
    for backend in $BACKENDS; do
        run_one "$algo" "$backend"
    done
done
echo "META|end|$(date -u +%Y-%m-%dT%H:%M:%SZ)"
