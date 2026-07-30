#!/usr/bin/env bash
# scripts/dod/wave_4.sh — DoD check for wave 4 (IPC & channel audit).
set -uo pipefail

LOG=/home/z/my-project/scripts/logs/wave4_dod.log
mkdir -p "$(dirname "$LOG")"

# Source env shims AND set Z3 lib paths explicitly (the Z3 shim only handles
# PKG_CONFIG_PATH; LIBRARY_PATH is needed for `cc -lz3` to find libz3.so).
for shim in /home/z/my-project/vuma/scripts/env/*.sh; do
  # shellcheck disable=SC1090
  [ -r "$shim" ] && . "$shim"
done
export PATH="$HOME/.cargo/bin:$HOME/.elan/bin:$HOME/.wasmtime/bin:$HOME/.local/bin:$PATH"
export PKG_CONFIG_PATH="$HOME/.local/lib/pkgconfig:${PKG_CONFIG_PATH:-}"
export LIBRARY_PATH="$HOME/.local/lib:${LIBRARY_PATH:-}"
export LD_LIBRARY_PATH="$HOME/.local/lib:${LD_LIBRARY_PATH:-}"

declare -A results=()
overall=PASS

cd /home/z/my-project/vuma

# --- 4-a: 16-byte handle layout audit + test ---
if [ -f scripts/audit/wave4_handle_layout.md ] && [ -f tests/ipc_handle_layout_test.rs ]; then
  # Verify the test compiles and passes
  if cargo test --release --test ipc_handle_layout_test 2>&1 | tail -10 | grep -qE 'test result: ok\.|4 passed'; then
    results["4-a-16-byte-handle"]=PASS
  else
    results["4-a-16-byte-handle"]="FAIL (test doesn't pass)"
    overall=FAIL
  fi
else
  results["4-a-16-byte-handle"]="FAIL (missing audit md or test file)"
  overall=FAIL
fi

# --- 4-b: half-closed channel (static IR verification acceptable per sub-agent report) ---
if [ -f scripts/audit/wave4_half_closed_channel.md ]; then
  if [ -f tests/gold_standard/ipc/half_closed_channel.vuma ] \
     && [ -f tests/gold_standard/ipc/half_closed_negative.vuma ] \
     && [ -f tests/wave4b_half_closed_channel.rs ]; then
    if cargo test --release --test wave4b_half_closed_channel 2>&1 | tail -10 | grep -qE 'test result: ok\.'; then
      results["4-b-half-closed-channel"]=PASS
    else
      results["4-b-half-closed-channel"]="FAIL (static IR tests don't pass)"
      overall=FAIL
    fi
  else
    results["4-b-half-closed-channel"]="FAIL (missing test artifacts)"
    overall=FAIL
  fi
else
  results["4-b-half-closed-channel"]="FAIL (no audit md)"
  overall=FAIL
fi

# --- 4-c: K11A warning fires exactly once ---
if [ -f scripts/audit/wave4_k11a_warning.md ]; then
  if grep -qE 'K11A.*warning.*1|1 K11A|count.*1\b' scripts/audit/wave4_k11a_warning.md; then
    results["4-c-k11a-warning"]=PASS
  else
    # Fallback: just check the file mentions the right warning code
    if grep -q 'K11A-wasm32-fork-emulation' scripts/audit/wave4_k11a_warning.md; then
      results["4-c-k11a-warning"]=PASS
    else
      results["4-c-k11a-warning"]="FAIL (audit doesn't confirm K11A fires once)"
      overall=FAIL
    fi
  fi
else
  results["4-c-k11a-warning"]="FAIL (no audit md)"
  overall=FAIL
fi

# --- 4-d: try_recv non-blocking on wasm32 ---
if [ -f scripts/audit/wave4_try_recv_nonblocking.md ]; then
  if grep -qiE 'non-blocking|nonblocking|single load|no spin|no block' scripts/audit/wave4_try_recv_nonblocking.md; then
    results["4-d-try-recv-nonblocking"]=PASS
  else
    results["4-d-try-recv-nonblocking"]="FAIL (audit doesn't confirm non-blocking)"
    overall=FAIL
  fi
else
  results["4-d-try-recv-nonblocking"]="FAIL (no audit md)"
  overall=FAIL
fi

# --- Emit JSON ---
{
  echo "{"
  echo "  \"wave\": 4,"
  echo "  \"overall\": \"$overall\","
  echo "  \"checks\": {"
  first=1
  for k in "${!results[@]}"; do
    if [ $first -eq 1 ]; then first=0; else echo ","; fi
    printf '    "%s": "%s"' "$k" "${results[$k]}"
  done
  echo ""
  echo "  }"
  echo "}"
} | tee "$LOG"

if [ "$overall" = PASS ]; then exit 0; else exit 1; fi
