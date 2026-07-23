#!/bin/bash
# Run all key gold-standard IPC tests across ALL 19 backends.
# Reports a pass/fail matrix.
# Usage: bash vuma_test_matrix.sh [test_filter]

set +e

REPO="/home/z/my-project/vuma"
QEMU_DIR="$HOME/.local/bin"
export PATH="$QEMU_DIR:$PATH"

# All 19 backends
BACKENDS="x86_64 aarch64 riscv64 arm32 loongarch64 mips64 mips64be ppc64 ppc64le riscv32 x86_32 sparc64 s390x m68k alpha hppa armeb aarch64_be wasm32"

# Key tests (from TASKS.md Appendix B + extras)
TESTS="${@:-simple_send ping_pong multi_message try_recv recv_timeout match_recv framed_send_recv capability_grant_verify protocol_valid protocol_invalid shared_memory_rw memory_limit stark_proof ffi_basic supervisor driver_isolation fault_tolerance hot_swap distributed aead checkpoint sandbox resource_limit error_containment cap_flow cap_revoke delegation ffi_crash_recovery ffi_isolation linear_valid infoflow_valid session_valid formal_verify}"

# Map backend name to qemu binary
qemu_for() {
  case "$1" in
    x86_64) echo "" ;;
    aarch64) echo "qemu-aarch64-static" ;;
    riscv64) echo "qemu-riscv64-static" ;;
    arm32) echo "qemu-arm-static" ;;
    loongarch64) echo "qemu-loongarch64-static" ;;
    mips64) echo "qemu-mips64el-static" ;;
    mips64be) echo "qemu-mips64-static" ;;
    ppc64) echo "qemu-ppc64-static" ;;
    ppc64le) echo "qemu-ppc64le-static" ;;
    riscv32) echo "qemu-riscv32-static" ;;
    x86_32) echo "qemu-i386-static" ;;
    sparc64) echo "qemu-sparc64-static" ;;
    s390x) echo "qemu-s390x-static" ;;
    m68k) echo "qemu-m68k-static" ;;
    alpha) echo "qemu-alpha-static" ;;
    hppa) echo "qemu-hppa-static" ;;
    armeb) echo "qemu-armeb-static" ;;
    aarch64_be) echo "qemu-aarch64_be-static" ;;
    wasm32) echo "wasm32" ;;
  esac
}

TEST_DIR="$REPO/tests/gold_standard/ipc"
COMPILE_DUMP="$REPO/target/debug/compile_dump"

# Ensure compile_dump exists
if [ ! -f "$COMPILE_DUMP" ]; then
  echo "compile_dump missing - building..."
  . "$HOME/.cargo/env"
  cd "$REPO" && cargo build --workspace 2>&1 | tail -3
fi

# Print header
printf "%-28s" "Test"
for b in $BACKENDS; do
  printf "%-8s" "$b"
done
echo
echo "-------------------------------------------------------------------------------------------------------------------"

# Track stats
declare -A pass_count
declare -A fail_count

for t in $TESTS; do
  vuma="$TEST_DIR/$t.vuma"
  if [ ! -f "$vuma" ]; then
    continue
  fi
  expected=$(grep -m1 -iE 'Expected exit code' "$vuma" | grep -oE '[-]?[0-9]+' | head -1)
  [ -z "$expected" ] && expected="?"

  printf "%-28s" "$t"
  for b in $BACKENDS; do
    bin="/tmp/matrix_${b}_${t}.bin"
    out=$("$COMPILE_DUMP" "$vuma" "$bin" "$b" 2>&1)
    if [ ! -f "$bin" ]; then
      printf "%-8s" "CERR"
      fail_count[$b]=$(( ${fail_count[$b]:-0} + 1 ))
      continue
    fi
    if [ "$b" = "wasm32" ]; then
      printf "%-8s" "wasm"
      continue
    fi
    qb=$(qemu_for "$b")
    if [ -z "$qb" ]; then
      timeout 5 "$bin" >/dev/null 2>&1
      rc=$?
    else
      timeout 5 "$qb" "$bin" >/dev/null 2>&1
      rc=$?
    fi
    if [ "$rc" = "$expected" ]; then
      printf "%-8s" "OK"
      pass_count[$b]=$(( ${pass_count[$b]:-0} + 1 ))
    else
      printf "%-8s" "F:$rc"
      fail_count[$b]=$(( ${fail_count[$b]:-0} + 1 ))
    fi
    rm -f "$bin"
  done
  echo
done

echo
echo "=== Summary ==="
for b in $BACKENDS; do
  printf "%-15s pass=%-3s fail=%-3s\n" "$b" "${pass_count[$b]:-0}" "${fail_count[$b]:-0}"
done
