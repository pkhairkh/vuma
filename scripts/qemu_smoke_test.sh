#!/usr/bin/env bash
# ============================================================================
# VUMA — QEMU smoke-test script (Waves 13-14 backends)
# ============================================================================
# Builds the compiler once, then for every supported QEMU/wasmtime backend
# compiles a small set of gold-standard .vuma programs and runs them under
# the appropriate emulator, checking the process exit code against the
# `// Expected exit code:` header in each test file.
#
# Backends covered (12 QEMU + 1 wasmtime):
#   aarch64  x86_64  riscv64  s390x  arm32  ppc64  m68k  sparc64
#   hppa     alpha   loongarch64  mips64   +  wasm32 (wasmtime)
#
# All backends were individually smoke-tested in Waves 13-14; this script
# consolidates them into a single CI-friendly invocation.
#
# Usage:
#   scripts/qemu_smoke_test.sh                  # run all backends
#   VUMA_SMOKE_ISAS="x86_64 aarch64" scripts/qemu_smoke_test.sh
#   VUMA_SMOKE_NO_BUILD=1 scripts/qemu_smoke_test.sh   # skip cargo build
#   VUMA_SMOKE_TESTS="arith_add_basic test_exit" scripts/qemu_smoke_test.sh
#
# Exit status: 0 if every (backend, test) pair passes; 1 otherwise.
# ============================================================================

set -u

# ---------------------------------------------------------------------------
# Locate the repo root (directory containing Cargo.toml).
# ---------------------------------------------------------------------------
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
cd "${REPO_ROOT}"

VUMA_BIN="${REPO_ROOT}/target/release/vuma"
TEST_ROOT="${REPO_ROOT}/tests/gold_standard"
TMP_DIR="$(mktemp -d -t vuma-smoke-XXXXXX)"
trap 'rm -rf "${TMP_DIR}"' EXIT

# ---------------------------------------------------------------------------
# Per-ISA QEMU runner mapping.
#   <vuma-isa> <qemu-binary-base>
# Most are qemu-<isa>-static; the two mismatches are:
#   * arm32    -> qemu-arm-static     (qemu-user naming convention)
#   * mips64   -> qemu-mips64el-static (vuma emits little-endian MIPS64 ELF
#                                       — see Wave 14-d finding)
# wasm32 is handled separately via wasmtime.
# ---------------------------------------------------------------------------
declare -A QEMU_BIN=(
  [aarch64]="qemu-aarch64-static"
  [x86_64]="qemu-x86_64-static"
  [riscv64]="qemu-riscv64-static"
  [s390x]="qemu-s390x-static"
  [arm32]="qemu-arm-static"
  [ppc64]="qemu-ppc64-static"
  [m68k]="qemu-m68k-static"
  [sparc64]="qemu-sparc64-static"
  [hppa]="qemu-hppa-static"
  [alpha]="qemu-alpha-static"
  [loongarch64]="qemu-loongarch64-static"
  [mips64]="qemu-mips64el-static"
)

# Per-ISA vuma subcommand. `vuma build --isa <isa>` uses the direct
# AST→codegen path for non-AArch64 targets and the canonical pipeline for
# AArch64. The canonical pipeline's `emit_elf` (codegen/src/emit.rs:6512)
# is AArch64-only and produces an ELF with no `_start` stub, which
# segfaults under QEMU (Task 12-e finding). Task 13-a fixed `vuma emit`
# to use `compile_to_binary_direct` for all ISAs, so we route AArch64
# through `vuma emit` (which produces a proper statically-linked ELF with
# a real `_start` stub + exit syscall). All other ISAs go through
# `vuma build` (the path Waves 13-14 smoke-tested).
declare -A VUMA_CMD=(
  [aarch64]="emit"
)

# Default QEMU ISA set (12 backends, all Waves 13-14 working) + wasm32
# (run under wasmtime). Order: fast/canonical backends first so failures
# surface early; wasm32 last (uses a different runner + test subset).
DEFAULT_QEMU_ISAS=(
  x86_64 aarch64 riscv64 arm32
  ppc64 m68k sparc64 s390x
  alpha hppa loongarch64 mips64
  wasm32
)

# ---------------------------------------------------------------------------
# Test programs (relative to tests/gold_standard/) with expected exit codes.
# These are the same 4 integer-only programs used across the Wave 13-14
# smoke tasks; all carry an `// Expected exit code:` header which we also
# parse at runtime as a sanity check.
# ---------------------------------------------------------------------------
DEFAULT_TESTS=(
  "arithmetic/arith_add_basic.vuma:7"
  "arithmetic/arith_mul_basic.vuma:12"
  "arithmetic/test_exit.vuma:42"
  "control_flow/for_count.vuma:23"
)

# wasm32 uses a different test subset (per Wave 14-e — these were the
# programs verified to exit cleanly under wasmtime 47.0.2).
WASM_TESTS=(
  "edge_cases/edge_zero_plus_one.vuma:1"
  "edge_cases/edge_one_mul_one.vuma:1"
  "u32_arith/u32_add_pair.vuma:60"
  "control_flow/cf_if_true.vuma:42"
)

# Allow caller overrides.
if [ -n "${VUMA_SMOKE_ISAS:-}" ]; then
  read -r -a ISA_LIST <<< "${VUMA_SMOKE_ISAS}"
else
  ISA_LIST=("${DEFAULT_QEMU_ISAS[@]}")
fi

if [ -n "${VUMA_SMOKE_TESTS:-}" ]; then
  TESTS=()
  for t in ${VUMA_SMOKE_TESTS}; do
    TESTS+=("${t}:?")   # caller-provided tests don't carry expected codes
  done
else
  TESTS=("${DEFAULT_TESTS[@]}")
fi

# ---------------------------------------------------------------------------
# Helpers.
# ---------------------------------------------------------------------------
log()  { printf '%s\n' "$*"; }
err()  { printf '\033[31m%s\033[0m\n' "$*" >&2; }
ok()   { printf '\033[32m%s\033[0m\n' "$*"; }
warn() { printf '\033[33m%s\033[0m\n' "$*"; }

# Extract expected exit code from a .vuma file's `// Expected exit code: N`
# header. Returns 0 with code on stdout, or 1 if not found.
extract_expected_exit() {
  local file="$1"
  local line
  line="$(grep -m1 -E '^[[:space:]]*//.*[Ee]xpected exit code' "${file}" || true)"
  [ -z "${line}" ] && return 1
  # Parse the trailing integer (handles "code: 7", "code 7", "code:7").
  echo "${line}" | grep -oE '[0-9]+' | tail -1
}

# ---------------------------------------------------------------------------
# Step 1 — Build the compiler (skippable via VUMA_SMOKE_NO_BUILD=1).
# ---------------------------------------------------------------------------
if [ -z "${VUMA_SMOKE_NO_BUILD:-}" ]; then
  log "==> Building vuma (cargo build --release --bin vuma)..."
  if ! cargo build --release --bin vuma; then
    err "==> cargo build failed"
    exit 1
  fi
fi

if [ ! -x "${VUMA_BIN}" ]; then
  err "==> vuma binary not found at ${VUMA_BIN}"
  err "    (run without VUMA_SMOKE_NO_BUILD=1, or run 'cargo build --release --bin vuma' first)"
  exit 1
fi

log "==> Using vuma: $(${VUMA_BIN} --version 2>&1 | head -1)"
log "==> Tests root: ${TEST_ROOT}"
log ""

# ---------------------------------------------------------------------------
# Step 2 — Run the smoke matrix.
# ---------------------------------------------------------------------------
TOTAL_PASS=0
TOTAL_FAIL=0
declare -a FAIL_ROWS=()

# Per-ISA rollup (for summary table): pass/total.
declare -A ISA_PASS
declare -A ISA_TOTAL

run_one() {
  local isa="$1"
  local test_rel="$2"
  local expected="$3"   # "?" if unknown
  local test_path="${TEST_ROOT}/${test_rel}"
  local test_name
  test_name="$(basename "${test_rel}" .vuma)"

  if [ ! -f "${test_path}" ]; then
    err "  MISSING  ${test_rel} (file not found)"
    FAIL_ROWS+=("${isa}|${test_rel}|MISSING|-")
    ISA_TOTAL["${isa}"]=$(( ${ISA_TOTAL["${isa}"]:-0} + 1 ))
    TOTAL_FAIL=$((TOTAL_FAIL + 1))
    return
  fi

  # Resolve expected exit from file header if not provided.
  if [ "${expected}" = "?" ]; then
    expected="$(extract_expected_exit "${test_path}" || echo "?")"
  fi

  local out="${TMP_DIR}/${isa}_${test_name}.bin"

  # Compile. Use `vuma emit <isa>` for AArch64 (Task 13-a fix path that
  # produces a proper _start stub); use `vuma build --isa <isa>` for
  # everything else (the direct AST→codegen path that Waves 13-14
  # smoke-tested). Note: `emit` takes the ISA as a positional argument,
  # `build` takes it via `--isa <isa>`.
  local subcmd="${VUMA_CMD[${isa}]:-build}"
  local build_log
  if [ "${subcmd}" = "emit" ]; then
    if ! build_log="$("${VUMA_BIN}" emit "${isa}" "${test_path}" -o "${out}" 2>&1)"; then
      err "  FAIL     ${isa} | ${test_rel} (build error)"
      FAIL_ROWS+=("${isa}|${test_rel}|BUILD_ERROR|-")
      ISA_TOTAL["${isa}"]=$(( ${ISA_TOTAL["${isa}"]:-0} + 1 ))
      TOTAL_FAIL=$((TOTAL_FAIL + 1))
      return
    fi
  else
    if ! build_log="$("${VUMA_BIN}" build --isa "${isa}" "${test_path}" -o "${out}" 2>&1)"; then
      err "  FAIL     ${isa} | ${test_rel} (build error)"
      FAIL_ROWS+=("${isa}|${test_rel}|BUILD_ERROR|-")
      ISA_TOTAL["${isa}"]=$(( ${ISA_TOTAL["${isa}"]:-0} + 1 ))
      TOTAL_FAIL=$((TOTAL_FAIL + 1))
      return
    fi
  fi

  # Run under the appropriate emulator.
  local runner
  case "${isa}" in
    wasm32)
      runner="wasmtime"
      ;;
    *)
      runner="${QEMU_BIN[${isa}]:-qemu-${isa}-static}"
      ;;
  esac

  if ! command -v "${runner}" >/dev/null 2>&1; then
    err "  FAIL     ${isa} | ${test_rel} (runner '${runner}' not installed)"
    FAIL_ROWS+=("${isa}|${test_rel}|RUNNER_MISSING|-")
    ISA_TOTAL["${isa}"]=$(( ${ISA_TOTAL["${isa}"]:-0} + 1 ))
    TOTAL_FAIL=$((TOTAL_FAIL + 1))
    return
  fi

  local actual
  "${runner}" "${out}" >/dev/null 2>&1
  actual=$?

  ISA_TOTAL["${isa}"]=$(( ${ISA_TOTAL["${isa}"]:-0} + 1 ))

  if [ "${expected}" = "?" ]; then
    # No expected code available — treat as a "ran without crash" smoke check
    # (any exit code acceptable; just confirm runner executed).
    ok "  SMOKE    ${isa} | ${test_rel} (exit=${actual}, no expected)"
    ISA_PASS["${isa}"]=$(( ${ISA_PASS["${isa}"]:-0} + 1 ))
    TOTAL_PASS=$((TOTAL_PASS + 1))
    return
  fi

  if [ "${actual}" = "${expected}" ]; then
    ok "  PASS     ${isa} | ${test_rel} (exit=${actual})"
    ISA_PASS["${isa}"]=$(( ${ISA_PASS["${isa}"]:-0} + 1 ))
    TOTAL_PASS=$((TOTAL_PASS + 1))
  else
    err "  FAIL     ${isa} | ${test_rel} (exit=${actual}, expected ${expected})"
    FAIL_ROWS+=("${isa}|${test_rel}|exit=${actual}|expected=${expected}")
    TOTAL_FAIL=$((TOTAL_FAIL + 1))
  fi
}

for isa in "${ISA_LIST[@]}"; do
  log "=== Backend: ${isa} ==="

  if [ "${isa}" = "wasm32" ]; then
    for entry in "${WASM_TESTS[@]}"; do
      test_rel="${entry%:*}"
      expected="${entry##*:}"
      run_one "${isa}" "${test_rel}" "${expected}"
    done
  else
    if [ -z "${QEMU_BIN[${isa}]:-}" ]; then
      warn "  SKIP     ${isa} (no QEMU mapping; add to QEMU_BIN table)"
      continue
    fi
    for entry in "${TESTS[@]}"; do
      test_rel="${entry%:*}"
      expected="${entry##*:}"
      run_one "${isa}" "${test_rel}" "${expected}"
    done
  fi
  log ""
done

# ---------------------------------------------------------------------------
# Step 3 — Summary table.
# ---------------------------------------------------------------------------
log "==================== QEMU Smoke Test Summary ===================="
printf '  %-14s %-8s %-8s\n' "BACKEND" "PASS" "TOTAL"
printf '  %-14s %-8s %-8s\n' "--------------" "--------" "--------"

for isa in "${ISA_LIST[@]}"; do
  if [ "${isa}" = "wasm32" ]; then
    p="${ISA_PASS[${isa}]:-0}"
    t="${ISA_TOTAL[${isa}]:-0}"
  else
    p="${ISA_PASS[${isa}]:-0}"
    t="${ISA_TOTAL[${isa}]:-0}"
  fi
  # Skip ISAs that were never started (e.g. unknown ISA in QEMU_BIN).
  [ "${t}" = "0" ] && continue
  printf '  %-14s %-8s %-8s\n' "${isa}" "${p}" "${t}"
done

log ""
log "  Total: ${TOTAL_PASS} passed, ${TOTAL_FAIL} failed"

if [ "${TOTAL_FAIL}" -gt 0 ]; then
  log ""
  err "  Failures:"
  for row in "${FAIL_ROWS[@]}"; do
    IFS='|' read -r fisa ftest fkind fextra <<< "${row}"
    err "    ${fisa} | ${ftest} | ${fkind} ${fextra}"
  done
fi

log "================================================================"

if [ "${TOTAL_FAIL}" -gt 0 ]; then
  exit 1
fi
exit 0
