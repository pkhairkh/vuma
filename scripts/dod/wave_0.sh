#!/usr/bin/env bash
# scripts/dod/wave_0.sh — DoD check for wave 0 (environment provisioning).
# Exits 0 on PASS, non-zero on FAIL. Prints structured JSON verdict to stdout.
set -uo pipefail

LOG=/home/z/my-project/scripts/logs/wave0_dod.log
mkdir -p "$(dirname "$LOG")"

# Source env shims so non-login shells see all installed binaries.
for shim in /home/z/my-project/vuma/scripts/env/*.sh; do
  # shellcheck disable=SC1090
  [ -r "$shim" ] && . "$shim"
done
export PATH="$HOME/.cargo/bin:$HOME/.elan/bin:$HOME/.wasmtime/bin:$HOME/.local/bin:$PATH"

declare -A results=()
overall=PASS

check() {
  local name="$1" cmd="$2" expect_regex="$3"
  local out rc
  out="$($cmd 2>&1)"
  rc=$?
  if [ $rc -eq 0 ] && echo "$out" | grep -Eq "$expect_regex"; then
    results["$name"]=PASS
  else
    results["$name"]=FAIL
    overall=FAIL
  fi
}

# --- Z3 ---
check "z3" "pkg-config --modversion z3" "^4\.(1[3-9]|[2-9][0-9])\."

# --- Rust ---
# rustc --version prints e.g. "rustc 1.96.0-nightly (38c0de8dc 2026-02-28)" — the
# channel pin is verified by rustup show active-toolchain instead.
check "rustc-version" "rustc --version" "rustc [0-9].*nightly"
check "rustc-channel" "rustup show active-toolchain" "nightly-2026-03-01"
check "cargo" "cargo --version" "cargo"

rustup_components="$(rustup +nightly-2026-03-01 component list --installed 2>&1)"
for c in rustfmt clippy rust-src; do
  # rustup prints target-qualified names like `rustfmt-x86_64-unknown-linux-gnu`;
  # match either the bare name or any target-qualified variant.
  if echo "$rustup_components" | grep -Eq "^${c}(-.*)?$"; then
    results["rust-$c"]=PASS
  else
    results["rust-$c"]=FAIL
    overall=FAIL
  fi
done

rustup_targets="$(rustup +nightly-2026-03-01 target list --installed 2>&1)"
for t in aarch64-unknown-linux-gnu aarch64-unknown-none; do
  if echo "$rustup_targets" | grep -qx "$t"; then
    results["rust-target-$t"]=PASS
  else
    results["rust-target-$t"]=FAIL
    overall=FAIL
  fi
done

# --- QEMU (18 ISAs) ---
qemu_isas="aarch64 aarch64_be arm armeb alpha hppa i386 loongarch64 m68k mips64 mips64el ppc64 ppc64le riscv32 riscv64 s390x sparc64 x86_64"
qemu_pass=0
qemu_fail=""
for isa in $qemu_isas; do
  bin="qemu-${isa}-static"
  if command -v "$bin" >/dev/null 2>&1 && "$bin" --version 2>&1 | head -1 | grep -Eq 'version (10\.|[1-9][1-9][0-9]?)'; then
    qemu_pass=$((qemu_pass+1))
  else
    qemu_fail="$qemu_fail $bin"
  fi
done
if [ -z "$qemu_fail" ]; then
  results["qemu-18-isas"]="PASS ($qemu_pass/18)"
else
  results["qemu-18-isas"]="FAIL ($qemu_pass/18; missing:$qemu_fail)"
  overall=FAIL
fi

# --- wasmtime ---
check "wasmtime" "wasmtime --version" "^wasmtime (2[9-9]|[3-9][0-9])"

# --- Lean ---
check "lean" "lean --version" "4\.21\.0"
check "lake" "lake --version" "Lean version 4\.21\.0"

# --- Cargo metadata smoke ---
if cargo metadata --manifest-path /home/z/my-project/vuma/Cargo.toml --no-deps >/dev/null 2>&1; then
  results["cargo-metadata"]=PASS
else
  results["cargo-metadata"]=FAIL
  overall=FAIL
fi

# --- Emit JSON verdict ---
{
  echo "{"
  echo "  \"wave\": 0,"
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
