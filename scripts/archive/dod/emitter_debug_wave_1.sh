#!/usr/bin/env bash
# scripts/dod/emitter_debug_wave_1.sh — DoD for Wave 1 (prologue/epilogue fix).
# Exits 0 on PASS, non-zero on FAIL. Prints JSON verdict to stdout.
set -uo pipefail

LOG=/home/z/my-project/vuma/scripts/logs/emitter_debug_wave1_dod.log
mkdir -p "$(dirname "$LOG")"

# Source env.
for shim in /home/z/my-project/vuma/scripts/env/*.sh; do
  [ -r "$shim" ] && . "$shim"
done
export PATH="$HOME/.cargo/bin:$HOME/.local/bin:$PATH"
export LD_LIBRARY_PATH="$HOME/.local/lib:${LD_LIBRARY_PATH:-}"
export LIBRARY_PATH="$HOME/.local/lib:${LIBRARY_PATH:-}"

cd /home/z/my-project/vuma

declare -A results=()
overall=PASS

# Rebuild.
cargo build --release --bin compile_dump >"$LOG" 2>&1
if [ $? -ne 0 ]; then
  echo '{"overall":"FAIL","reason":"build failed","log":"'"$LOG"'"}'
  exit 1
fi

# Compile u32_add with the gate ON.
VUMA_REAL_REGALLOC_X86_64=1 ./target/release/compile_dump \
  tests/gold_standard/u32_arith/u32_add.vuma /tmp/w1_u32_add.bin x86_64 --no-verify >>"$LOG" 2>&1

if [ ! -s /tmp/w1_u32_add.bin ]; then
  echo '{"overall":"FAIL","reason":"compile produced no binary"}'
  exit 1
fi

chmod +x /tmp/w1_u32_add.bin
/tmp/w1_u32_add.bin
exit_code=$?

# Save disassembly for inspection.
objdump -d /tmp/w1_u32_add.bin > scripts/audit/w1_u32_add_disasm.txt 2>&1

# DoD: exit code is 100 (the expected value of u32_add), OR at minimum
# not 139 (SIGSEGV) — meaning the prologue/epilogue no longer crashes.
if [ $exit_code -eq 100 ]; then
  verdict="PASS-exact"
  results["u32_add"]="PASS (exit=$exit_code, expected 100)"
elif [ $exit_code -ne 139 ]; then
  verdict="PARTIAL-non-139"
  results["u32_add"]="PARTIAL (exit=$exit_code, not 139 but not 100 either)"
  overall=PARTIAL
else
  verdict="FAIL-sigsegv"
  results["u32_add"]="FAIL (exit=$exit_code SIGSEGV)"
  overall=FAIL
fi

# Emit JSON.
python3 -c "
import json, sys
results = {'overall': '$overall', 'verdict': '$verdict', 'exit_code': $exit_code, 'results': {}}
for k, v in {'u32_add': '${results[u32_add]:-}'} .items():
    results['results'][k] = v
print(json.dumps(results, indent=2))
"
exit 0
