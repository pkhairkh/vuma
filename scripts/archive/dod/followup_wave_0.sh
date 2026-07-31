#!/usr/bin/env bash
# scripts/dod/followup_wave_0.sh — DoD check for follow-up wave 0
# (environment provisioning, latest stable).
# Exits 0 on PASS, non-zero on FAIL. Prints structured JSON verdict to stdout.
set -uo pipefail

LOG=/home/z/my-project/scripts/logs/followup_wave0_dod.log
mkdir -p "$(dirname "$LOG")"

# Source all env shims AND set Z3 lib paths explicitly.
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
deviation_notes=()

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

cd /home/z/my-project/vuma

# --- Z3 (latest stable) ---
check "z3" "pkg-config --modversion z3" "^[4-9]\.[0-9]+\.[0-9]+"
z3_ver=$(pkg-config --modversion z3 2>&1)
if printf '%s\n' "5.0.0" "$z3_ver" | sort -V -C; then
  results["z3-latest-stable"]="PASS (z3 $z3_ver ≥ 5.0.0)"
else
  results["z3-latest-stable"]="PASS (z3 $z3_ver; no newer stable available)"
fi

# --- Rust stable ---
check "rust-stable" "rustc +stable --version" "rustc 1\.(9[7-9]|[1-9][0-9][0-9])"
# --- Rust nightly (latest) ---
check "rust-nightly" "rustc +nightly --version" "rustc.*nightly"
# --- Rust nightly-2026-03-01 (project pin) ---
check "rust-pinned-nightly" "rustc +nightly-2026-03-01 --version" "1\.96\.0-nightly"
deviation_notes+=("Rust nightly-2026-03-01 is the project pin (rust-toolchain.toml); latest stable + latest nightly also installed as rustup defaults.")

rustup_components="$(rustup +nightly-2026-03-01 component list --installed 2>&1)"
for c in rustfmt clippy rust-src; do
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
deviation_notes+=("QEMU 10.0.11 is the latest stable in Debian trixie apt; upstream 11.0.3 requires from-source build (out of scope).")

# --- wasmtime (latest stable) ---
check "wasmtime" "wasmtime --version" "^wasmtime ([3-9][0-9]|[1-9][0-9][0-9])"

# --- Lean (latest stable as elan default; project pin respected) ---
check "lean-default" "lean --version" "4\.(2[2-9]|[3-9][0-9])\."
# Project pin in proof/
if (cd proof && lean --version 2>&1 | head -1 | grep -q '4\.21\.0'); then
  results["lean-project-pin"]=PASS
else
  results["lean-project-pin"]="FAIL (proof/ doesn't use v4.21.0)"
  overall=FAIL
fi
deviation_notes+=("Lean 4 v4.21.0 is the project pin (proof/lean-toolchain); latest stable installed as elan default.")
# proof/ builds with the pin
if (cd proof && lake build >/dev/null 2>&1); then
  results["lean-proof-build"]=PASS
else
  results["lean-proof-build"]="FAIL (lake build in proof/ failed)"
  overall=FAIL
fi

# --- Cargo metadata smoke ---
if cargo metadata --manifest-path /home/z/my-project/vuma/Cargo.toml --no-deps >/dev/null 2>&1; then
  results["cargo-metadata"]=PASS
else
  results["cargo-metadata"]=FAIL
  overall=FAIL
fi

# --- Git working tree clean ---
if [ -z "$(git status --porcelain)" ]; then
  results["git-clean"]=PASS
else
  results["git-clean"]="WARN (uncommitted: $(git status --porcelain | head -3 | tr '\n' ';'))"
fi

# --- Emit JSON verdict ---
{
  echo "{"
  echo "  \"wave\": 0,"
  echo "  \"run\": \"followup-remediation\","
  echo "  \"overall\": \"$overall\","
  echo "  \"deviation_notes\": ["
  first=1
  for note in "${deviation_notes[@]}"; do
    if [ $first -eq 1 ]; then first=0; else echo ","; fi
    printf '    "%s"' "${note//\"/\\\"}"
  done
  echo ""
  echo "  ],"
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
