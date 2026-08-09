#!/usr/bin/env python3
"""Generate W4-sym JSON + TXT reports from the raw run log."""
import json
import os
import sys
from collections import defaultdict

RAW = "/tmp/w4_sym_raw.log"
OUT_JSON = "/home/z/my-project/artifacts/w4_sym_results.json"
OUT_TXT = "/home/z/my-project/artifacts/w4_sym_results.txt"

EXPECTED = {
    "rc4": "bbf316e8d940af0ad3",
    "poly1305": "d795e0af355920ca0404c3da8782904a",
    "aes128": "69c4e0d86a7b0430d8cdb78070b4c55a",
    "aes192": "dda97ca4864cdfe06eaf70a0ec0d7191",
    "aes256": "8ea2b7ca516745bfeafc49904b496089",
    "chacha20": "c6bdf594fa87d094756b8d179a7ba25b816398cc26a334e7f7cf2720335074f1beb85c505d2d6dec471cd7ffaf002e85f3d6207bd9865fc130f6e554067f15bb7e9d9ec4be553c352466ad3fc54f03e4b3b991e755b51c76764786bab0a1023db1f0012369bfdd6661aeb325bbee22cbc13c",
    "des": "85e813540f0ab405",
    "salsa20": "4dfa5e481da23ea09a310a9fe8de6bc8e9d5d57c73f23e6b0b56292c0e9a8b3dd9ce4db7b8d49d81728f24719e6f9d3f0821e50e5cc0d0fa6e9c9a33f62c3d53",
}

REFERENCE_SOURCE = {
    "rc4": "rust_kat_vectors.json",
    "poly1305": "rust_kat_vectors.json",
    "aes128": "rust_kat_vectors.json (aes128_ecb)",
    "aes192": "FIPS-197 App. B (no Rust KAT)",
    "aes256": "FIPS-197 App. C.3 (no Rust KAT)",
    "chacha20": "rust_kat_vectors.json",
    "des": "FIPS-81 App. B (no Rust KAT)",
    "salsa20": "DJB Salsa20 spec (no Rust KAT)",
}

ALGORITHMS = ["rc4", "poly1305", "aes128", "aes192", "aes256",
              "chacha20", "des", "salsa20"]
BACKENDS = ["x86_64", "x86_32", "aarch64", "aarch64_be", "arm32", "armeb",
            "riscv64", "riscv32", "mips64", "mips64be", "ppc64", "ppc64le",
            "loongarch64", "s390x", "sparc64", "alpha", "hppa", "m68k",
            "wasm32"]

# Parse the raw log.
results = {}  # (algo, backend) -> dict
with open(RAW) as f:
    for line in f:
        line = line.strip()
        if not line.startswith("RESULT|"):
            continue
        parts = line.split("|")
        # RESULT|algo|backend|status|hex_len|hex_prefix|info
        if len(parts) < 7:
            continue
        algo = parts[1]
        backend = parts[2]
        status = parts[3]
        hex_len = int(parts[4]) if parts[4].isdigit() else 0
        hex_prefix = parts[5]
        info = parts[6]
        results[(algo, backend)] = {
            "status": status,
            "output_hex_len": hex_len,
            "output_hex_prefix": hex_prefix,
            "info": info,
            "expected_hex": EXPECTED.get(algo, ""),
            "reference_source": REFERENCE_SOURCE.get(algo, ""),
        }

# Build the structured report.
pairs = []
match_count = 0
mismatch_count = 0
qemu_missing = 0
compile_error = 0
for algo in ALGORITHMS:
    for backend in BACKENDS:
        r = results.get((algo, backend), {"status": "NOT_RUN"})
        r["algorithm"] = algo
        r["backend"] = backend
        pairs.append(r)
        s = r["status"]
        if s == "MATCH":
            match_count += 1
        elif s == "MISMATCH":
            mismatch_count += 1
        elif s == "QEMU_MISSING":
            qemu_missing += 1
        elif s == "COMPILE_ERROR":
            compile_error += 1

# Mismatch details grouped by algorithm.
mismatch_details = defaultdict(list)
for p in pairs:
    if p["status"] == "MISMATCH":
        mismatch_details[p["algorithm"]].append({
            "backend": p["backend"],
            "expected": p.get("expected_hex", ""),
            "actual_prefix": p.get("output_hex_prefix", ""),
            "actual_len": p.get("output_hex_len", 0),
            "info": p.get("info", ""),
        })

report = {
    "task_id": "W4-sym",
    "agent": "sub-agent (general-purpose)",
    "description": "VUMA harnesses for symmetric ciphers validated against Rust reference / standard vectors across 19 backends",
    "harnesses_written": len(ALGORITHMS),
    "harness_list": [
        f"tests/rust_differential/test_{a}_basic.vuma" for a in ALGORITHMS
    ],
    "backends_tested": BACKENDS,
    "total_pairs": len(pairs),
    "matches": match_count,
    "mismatches": mismatch_count,
    "qemu_missing": qemu_missing,
    "compile_errors": compile_error,
    "expected_vectors": EXPECTED,
    "reference_sources": REFERENCE_SOURCE,
    "results": pairs,
    "mismatch_details": dict(mismatch_details),
    "notes": [
        "Output mechanism: each output byte emitted as print_int(byte + 1000) "
        "to produce fixed-width 4-digit decimal, parsed by stripping whitespace "
        "and splitting into 4-char chunks. The state-as-Address FFI cast for "
        "write(2) truncates the buffer pointer to 32 bits on 64-bit backends "
        "(observed in x86_64 disassembly: 'mov %eax,%eax'), so direct binary "
        "write via FFI is broken on 64-bit backends. The print_int builtin "
        "approach sidesteps this.",
        "poly1305: the womb/crypto/symmetric/poly1305.vuma module has a TODO "
        "in poly1305_block — the acc*r mod p multiplication is not implemented "
        "(only the addition path runs). All backends produce identical but "
        "wrong output — this is an algorithmic gap, not a codegen bug.",
        "chacha20: chacha20_encrypt does not increment ctx.counter between "
        "64-byte blocks, so bytes 64-113 of the 114-byte RFC 7539 test "
        "vector use the wrong counter. Additionally the first block's "
        "keystream does not match RFC 7539 even with counter=1 set via "
        "chacha20_set_counter — suggesting a deeper algorithmic issue in "
        "the QR function or sigma constants.",
        "salsa20: no Rust KAT vector exists. The DJB Salsa20/20 keystream "
        "test vector (key=00..1f, nonce=00..00,4a, counter=0) was used as "
        "the reference; output does not match — likely a nonce/counter "
        "endianness mismatch in the VUMA module's setup_state.",
        "Backends with partial/no output (arm32, armeb, mips64, mips64be, "
        "loongarch64, sparc64, hppa) have a print_int stub that emits "
        "binary or partial data instead of decimal strings — a codegen "
        "gap in the print_int runtime helper for those backends.",
        "des on ppc64/ppc64le/m68k: produces correct-length but wrong-value "
        "output — a real codegen differential (likely endianness or "
        "64-bit-int handling in the DES permutation tables).",
        "aes128/192/256 on wasm32: produces correct-length but wrong-value "
        "output — a real codegen differential specific to the AES key "
        "schedule or round function on wasm32.",
    ],
}

os.makedirs(os.path.dirname(OUT_JSON), exist_ok=True)
with open(OUT_JSON, "w") as f:
    json.dump(report, f, indent=2)

# Human-readable TXT report.
lines = []
lines.append("=" * 78)
lines.append("W4-sym: Symmetric Cipher VUMA Harnesses — Cross-Backend Validation")
lines.append("=" * 78)
lines.append("")
lines.append(f"Harnesses written: {len(ALGORITHMS)}")
lines.append(f"Backends tested:   {len(BACKENDS)}")
lines.append(f"Total (algo, backend) pairs: {len(pairs)}")
lines.append(f"  MATCH:       {match_count}")
lines.append(f"  MISMATCH:    {mismatch_count}")
lines.append(f"  QEMU_MISSING: {qemu_missing}")
lines.append(f"  COMPILE_ERROR: {compile_error}")
lines.append("")
lines.append("-" * 78)
lines.append("Expected output hex per algorithm:")
lines.append("-" * 78)
for a in ALGORITHMS:
    lines.append(f"  {a:12s} ref={REFERENCE_SOURCE[a]}")
    lines.append(f"               exp={EXPECTED[a]}")
lines.append("")
lines.append("-" * 78)
lines.append("Per-algorithm match matrix (M = match, . = mismatch, - = missing)")
lines.append("-" * 78)
header = f"{'algorithm':12s} " + " ".join(f"{b[:4]:>4s}" for b in BACKENDS) + f" {'pass':>4s}/19"
lines.append(header)
for a in ALGORITHMS:
    cells = []
    m = 0
    for b in BACKENDS:
        s = results.get((a, b), {}).get("status", "NOT_RUN")
        if s == "MATCH":
            cells.append("  M ")
            m += 1
        elif s == "MISMATCH":
            cells.append("  . ")
        elif s == "QEMU_MISSING":
            cells.append("  - ")
        else:
            cells.append("  ? ")
    lines.append(f"{a:12s} " + "".join(cells) + f" {m:>3d}/19")
lines.append("")
lines.append("-" * 78)
lines.append("Mismatch details (algorithm -> failing backends)")
lines.append("-" * 78)
for a in ALGORITHMS:
    fails = mismatch_details.get(a, [])
    if not fails:
        continue
    lines.append(f"  {a} ({len(fails)} backends):")
    for f in fails:
        lines.append(f"    {f['backend']:14s} exp_prefix={f['expected'][:32]}...")
        lines.append(f"                   act_prefix={f['actual_prefix'][:32]}... (len={f['actual_len']} hex chars)")
        lines.append(f"                   info={f['info']}")
lines.append("")
lines.append("-" * 78)
lines.append("Harness files:")
lines.append("-" * 78)
for a in ALGORITHMS:
    lines.append(f"  tests/rust_differential/test_{a}_basic.vuma")
lines.append("")
lines.append(f"Full JSON report: {OUT_JSON}")
lines.append(f"Raw run log:      /tmp/w4_sym_raw.log")

with open(OUT_TXT, "w") as f:
    f.write("\n".join(lines) + "\n")

print(f"Wrote {OUT_JSON}")
print(f"Wrote {OUT_TXT}")
print(f"Matches: {match_count}/{len(pairs)}, Mismatches: {mismatch_count}")
