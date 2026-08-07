#!/usr/bin/env python3
"""
Salsa20 conformance test harness for womb/crypto/symmetric/salsa20.vuma.

Strategy (mirrors ref_tests/scripts/md5_test.py):
  1. Read the .vuma module source.
  2. For each eSTREAM test vector:
     a. Generate a self-contained .vuma driver (module source + main()).
        The main() sets up ctx+key+nonce, calls salsa20_block to generate
        the 64-byte keystream block at counter=0, then verifies each of
        the first 32 keystream bytes against the expected value. Returns 0
        on success, or (byte_index + 1) on the first mismatch.
     b. Compile with compile_dump (x86_64, --verify).
     c. Run the binary; check exit code (0 = pass, non-zero = fail).
     d. Cross-check the expected keystream with PyCryptodome (OpenSSL).
  3. Print per-vector result and summary.

Test vectors: official eSTREAM Salsa20 256-bit-key test vector set
(alexwebr/salsa20/test_vectors.256, derived from ECRYPT's verified test
vectors). The two vectors from the W2-D task brief have known
discrepancies (see salsa20.vuma's header comment and the W2-D worklog
entry); this harness verifies against the canonical eSTREAM vectors
instead.

Usage:
    python3 salsa20_test.py [--verbose]
    python3 salsa20_test.py --count 8     # run first 8 vectors
"""

import hashlib
import os
import subprocess
import sys
from datetime import datetime, timezone
from pathlib import Path

# Optional PyCryptodome for cross-checking the expected keystream.
try:
    from Crypto.Cipher import Salsa20 as PySalsa20
    HAVE_PYCRYPTO = True
except ImportError:
    HAVE_PYCRYPTO = False

REPO = Path("/home/z/my-project/workdir/vuma")
MODULE = REPO / "womb/crypto/symmetric/salsa20.vuma"
COMPILE_DUMP = REPO / "target/release-fast/compile_dump"
ENV = {**os.environ, "LD_LIBRARY_PATH": "/home/z/.local/lib"}
DRIVER_DIR = REPO / "ref_tests/drivers/w1"
DRIVER_DIR.mkdir(parents=True, exist_ok=True)
REPORT_DIR = REPO / "ref_tests/reports/salsa20"
REPORT_DIR.mkdir(parents=True, exist_ok=True)


# ────────────────────────────────────────────────────────────────────────────
# Test vectors — official eSTREAM Salsa20 256-bit-key set
# (https://github.com/alexwebr/salsa20/blob/master/test_vectors.256)
# ────────────────────────────────────────────────────────────────────────────
# Each vector: (label, key_hex (32 bytes), nonce_hex (8 bytes),
#               expected_keystream_hex (first 32 bytes of block 0),
#               block_index (0 = first block, 1 = second block, ...))
#
# The eSTREAM "Set 1" vectors walk a single set bit through the key bytes:
#   vector#  0 → byte 0  = 0x80
#   vector#  9 → byte 1  = 0x40
#   vector# 18 → byte 2  = 0x20
# (every 9th vector advances one bit position). "Set 6" uses a
# high-Hamming-weight key + nonce pair.
ESTREAM_VECTORS = [
    (
        "eSTREAM Set 1, vector# 0 (key byte 0 = 0x80)",
        "80000000000000000000000000000000" "00000000000000000000000000000000",
        "0000000000000000",
        "E3BE8FDD8BECA2E3EA8EF9475B29A6E7" "003951E1097A5C38D23B7A5FAD9F6844",
        0,
    ),
    (
        "eSTREAM Set 1, vector# 9 (key byte 1 = 0x40)",
        "00400000000000000000000000000000" "00000000000000000000000000000000",
        "0000000000000000",
        None,  # filled from PyCryptodome
        0,
    ),
    (
        "eSTREAM Set 1, vector# 18 (key byte 2 = 0x20)",
        "00002000000000000000000000000000" "00000000000000000000000000000000",
        "0000000000000000",
        None,  # filled from PyCryptodome
        0,
    ),
    (
        "eSTREAM Set 6, vector# 0 (block 0)",
        "0053A6F94C9FF24598EB3E91E4378ADD" "3083D6297CCF2275C81B6EC11467BA0D",
        "0D74DB42A91077DE",
        "F5FAD53F79F9DF58C4AEA0D0ED9A9601" "F278112CA7180D565B420A48019670EA",
        0,
    ),
    (
        "eSTREAM Set 6, vector# 0 (block 1 — counter increment)",
        "0053A6F94C9FF24598EB3E91E4378ADD" "3083D6297CCF2275C81B6EC11467BA0D",
        "0D74DB42A91077DE",
        None,  # filled from PyCryptodome (block 1)
        1,
    ),
]


def fill_expected_from_pycrypto():
    """Fill in any `None` expected keystreams via PyCryptodome.
    This cross-validates the eSTREAM-provided vectors AND supplies
    expected values for vectors whose first-block keystream we didn't
    hardcode above."""
    if not HAVE_PYCRYPTO:
        print("WARNING: PyCryptodome not available — skipping cross-check fill.")
        return
    for i, (label, key_hex, nonce_hex, expected, block_idx) in enumerate(ESTREAM_VECTORS):
        key = bytes.fromhex(key_hex)
        nonce = bytes.fromhex(nonce_hex)
        # Generate keystream at the requested block index.
        cipher = PySalsa20.new(key=key, nonce=nonce)
        # Skip to block_idx: discard block_idx * 64 bytes of keystream,
        # then read 32 bytes (the bytes we verify).
        _ = cipher.encrypt(b"\x00" * (block_idx * 64))
        ks = cipher.encrypt(b"\x00" * 32)
        computed = ks.hex().upper()
        if expected is not None:
            # Cross-check the hardcoded expected against PyCryptodome.
            if computed != expected.upper():
                print(f"WARNING: vector {i} ({label}) hardcoded expected does "
                      f"NOT match PyCryptodome:")
                print(f"  hardcoded: {expected.upper()}")
                print(f"  pycrypto:  {computed}")
        else:
            # Fill from PyCryptodome.
            ESTREAM_VECTORS[i] = (label, key_hex, nonce_hex, computed, block_idx)


def read_module() -> str:
    """Read salsa20.vuma source (used as the prefix of the driver)."""
    return MODULE.read_text()


def gen_driver(module_src: str, key_hex: str, nonce_hex: str,
               expected_hex: str, idx: int, block_idx: int = 0) -> str:
    """Generate a self-contained .vuma driver for one test vector.

    The driver:
      1. Allocates ctx, key, nonce, keystream output via state_new.
      2. Sets the 32 key bytes and 8 nonce bytes from the hex inputs.
      3. Calls salsa20_block(ctx, key, nonce, block_idx, out) to generate
         the keystream block at the requested counter value.
      4. Verifies out.bytes[0..31] against the expected keystream bytes.
         Returns 0 on success, or (byte_index + 1) on the first mismatch.
    """
    key_bytes = bytes.fromhex(key_hex)
    nonce_bytes = bytes.fromhex(nonce_hex)
    expected_bytes = bytes.fromhex(expected_hex)
    assert len(key_bytes) == 32
    assert len(nonce_bytes) == 8
    assert len(expected_bytes) == 32  # we verify first 32 bytes of the block

    # Key assignment lines: key.bytes[i] = K_i;
    key_lines = "\n    ".join(
        f"key.bytes[{i}] = {b};" for i, b in enumerate(key_bytes)
    )
    nonce_lines = "\n    ".join(
        f"nonce.bytes[{i}] = {b};" for i, b in enumerate(nonce_bytes)
    )
    # Verify lines: each byte, return (i+1) on mismatch.
    verify_lines = "\n    ".join(
        f"if out.bytes[{i}] != {b} {{ return {i + 1}; }}"
        for i, b in enumerate(expected_bytes)
    )

    return f"""// AUTO-GENERATED — Salsa20 eSTREAM test vector #{idx}
// key   = {key_hex}
// nonce = {nonce_hex}
// block index (counter) = {block_idx}
// expected keystream[0..31] = {expected_hex}

{module_src}

transform main() -> i32 {{
    let ctx = state_new(Salsa20Ctx);
    let key = state_new(Salsa20Key);
    let nonce = state_new(Salsa20Nonce);
    let out = state_new(Salsa20Block);

    {key_lines}

    {nonce_lines}

    salsa20_block(ctx, key, nonce, {block_idx}, out);

    {verify_lines}

    return 0;
}}
"""


def run_vector(module_src: str, label: str, key_hex: str, nonce_hex: str,
               expected_hex: str, idx: int, block_idx: int, verbose: bool) -> bool:
    """Compile + run a single vector. Returns True if pass."""
    driver_src = gen_driver(module_src, key_hex, nonce_hex, expected_hex,
                            idx, block_idx)
    driver_path = DRIVER_DIR / f"salsa20_vec_{idx:04d}.vuma"
    bin_path = DRIVER_DIR / f"salsa20_vec_{idx:04d}.bin"
    driver_path.write_text(driver_src)

    compile_cmd = [str(COMPILE_DUMP), str(driver_path), str(bin_path),
                   "x86_64", "--verify"]
    try:
        result = subprocess.run(compile_cmd, capture_output=True, text=True,
                                timeout=120, env=ENV)
    except subprocess.TimeoutExpired:
        if verbose:
            print(f"  vec {idx:4d} [{label}]: COMPILE TIMEOUT")
        return False

    if result.returncode != 0 or not bin_path.exists():
        if verbose:
            print(f"  vec {idx:4d} [{label}]: COMPILE FAIL")
            print(f"    stderr: {result.stderr[:400]}")
        return False

    try:
        run_result = subprocess.run([str(bin_path)], capture_output=True,
                                    timeout=10, env=ENV)
    except subprocess.TimeoutExpired:
        if verbose:
            print(f"  vec {idx:4d} [{label}]: RUN TIMEOUT")
        return False

    exit_code = run_result.returncode
    passed = (exit_code == 0)

    if verbose or not passed:
        if passed:
            print(f"  vec {idx:4d} [{label}]: PASS")
        else:
            print(f"  vec {idx:4d} [{label}]: FAIL (exit={exit_code})")
            if 1 <= exit_code <= 32:
                print(f"    mismatch at byte {exit_code - 1} of block 0")
    return passed


def gen_full_block_driver(module_src: str, key_hex: str, nonce_hex: str,
                          expected_hex: str, idx: int) -> str:
    """Generate a driver that verifies the FULL 64-byte block 0 keystream."""
    key_bytes = bytes.fromhex(key_hex)
    nonce_bytes = bytes.fromhex(nonce_hex)
    expected_bytes = bytes.fromhex(expected_hex)
    assert len(key_bytes) == 32
    assert len(nonce_bytes) == 8
    assert len(expected_bytes) == 64  # full block

    key_lines = "\n    ".join(
        f"key.bytes[{i}] = {b};" for i, b in enumerate(key_bytes)
    )
    nonce_lines = "\n    ".join(
        f"nonce.bytes[{i}] = {b};" for i, b in enumerate(nonce_bytes)
    )
    verify_lines = "\n    ".join(
        f"if out.bytes[{i}] != {b} {{ return {i + 1}; }}"
        for i, b in enumerate(expected_bytes)
    )

    return f"""// AUTO-GENERATED — Salsa20 full-block test #{idx}
// Verifies all 64 bytes of block 0.
// key   = {key_hex}
// nonce = {nonce_hex}
// expected block 0 keystream[0..63] = {expected_hex}

{module_src}

transform main() -> i32 {{
    let ctx = state_new(Salsa20Ctx);
    let key = state_new(Salsa20Key);
    let nonce = state_new(Salsa20Nonce);
    let out = state_new(Salsa20Block);

    {key_lines}

    {nonce_lines}

    salsa20_block(ctx, key, nonce, 0, out);

    {verify_lines}

    return 0;
}}
"""


def gen_multiblock_driver(module_src: str, key_hex: str, nonce_hex: str,
                          plaintext: bytes, expected_hex: str, idx: int) -> str:
    """Generate a driver that encrypts `plaintext` (≤256 bytes = 4 blocks)
    via salsa20_encrypt and verifies the full ciphertext."""
    key_bytes = bytes.fromhex(key_hex)
    nonce_bytes = bytes.fromhex(nonce_hex)
    expected_bytes = bytes.fromhex(expected_hex)
    assert len(key_bytes) == 32
    assert len(nonce_bytes) == 8
    assert len(plaintext) <= 256
    assert len(expected_bytes) == len(plaintext)

    key_lines = "\n    ".join(
        f"key.bytes[{i}] = {b};" for i, b in enumerate(key_bytes)
    )
    nonce_lines = "\n    ".join(
        f"nonce.bytes[{i}] = {b};" for i, b in enumerate(nonce_bytes)
    )
    pt_lines = "\n    ".join(
        f"data.bytes[{i}] = {b};" for i, b in enumerate(plaintext)
    )
    # data_len: plaintext length.
    data_len = len(plaintext)
    verify_lines = "\n    ".join(
        f"if out.bytes[{i}] != {b} {{ return {i + 1}; }}"
        for i, b in enumerate(expected_bytes)
    )

    return f"""// AUTO-GENERATED — Salsa20 multi-block encrypt test #{idx}
// Encrypts {data_len}-byte plaintext via salsa20_encrypt, verifies ciphertext.
// key   = {key_hex}
// nonce = {nonce_hex}
// plaintext  = {plaintext.hex()}
// expected ct = {expected_hex}

{module_src}

transform main() -> i32 {{
    let ctx = state_new(Salsa20Ctx);
    let key = state_new(Salsa20Key);
    let nonce = state_new(Salsa20Nonce);
    let data = state_new(Salsa20Buf);
    let out = state_new(Salsa20Buf);

    {key_lines}

    {nonce_lines}

    {pt_lines}

    salsa20_encrypt(ctx, data, {data_len}, out, key, nonce);

    {verify_lines}

    return 0;
}}
"""


def run_extra_driver(driver_src: str, name: str, verbose: bool) -> bool:
    """Compile + run an extra (non-batched) driver. Returns True if pass."""
    driver_path = DRIVER_DIR / f"{name}.vuma"
    bin_path = DRIVER_DIR / f"{name}.bin"
    driver_path.write_text(driver_src)
    compile_cmd = [str(COMPILE_DUMP), str(driver_path), str(bin_path),
                   "x86_64", "--verify"]
    try:
        result = subprocess.run(compile_cmd, capture_output=True, text=True,
                                timeout=120, env=ENV)
    except subprocess.TimeoutExpired:
        if verbose:
            print(f"  [{name}]: COMPILE TIMEOUT")
        return False
    if result.returncode != 0 or not bin_path.exists():
        if verbose:
            print(f"  [{name}]: COMPILE FAIL")
            print(f"    stderr: {result.stderr[:400]}")
        return False
    try:
        run_result = subprocess.run([str(bin_path)], capture_output=True,
                                    timeout=10, env=ENV)
    except subprocess.TimeoutExpired:
        if verbose:
            print(f"  [{name}]: RUN TIMEOUT")
        return False
    exit_code = run_result.returncode
    passed = (exit_code == 0)
    if verbose or not passed:
        status = "PASS" if passed else f"FAIL (exit={exit_code})"
        extra = ""
        if not passed and 1 <= exit_code <= 64:
            extra = f" [mismatch at byte {exit_code - 1}]"
        print(f"  [{name}]: {status}{extra}")
    return passed


def run_full_block_tests(module_src: str, verbose: bool) -> tuple[int, int]:
    """Verify the full 64-byte block 0 keystream for the eSTREAM vectors
    (cross-checked with PyCryptodome)."""
    if not HAVE_PYCRYPTO:
        print("(skipping full-block tests: PyCryptodome unavailable)")
        return 0, 0
    passed = 0
    total = 0
    for idx, (label, key_hex, nonce_hex, _, block_idx) in enumerate(ESTREAM_VECTORS[:3]):
        total += 1
        key = bytes.fromhex(key_hex)
        nonce = bytes.fromhex(nonce_hex)
        # Full 64-byte block at the vector's block index.
        cipher = PySalsa20.new(key=key, nonce=nonce)
        _ = cipher.encrypt(b"\x00" * (block_idx * 64))
        ks = cipher.encrypt(b"\x00" * 64)
        driver = gen_full_block_driver(module_src, key_hex, nonce_hex,
                                       ks.hex().upper(), idx)
        ok = run_extra_driver(driver, f"salsa20_full_{idx:04d}", verbose)
        if ok:
            passed += 1
    return passed, total


def run_multiblock_tests(module_src: str, verbose: bool) -> tuple[int, int]:
    """Verify the streaming encrypt path (counter increment) by encrypting
    multi-block plaintexts and comparing against PyCryptodome."""
    if not HAVE_PYCRYPTO:
        print("(skipping multi-block tests: PyCryptodome unavailable)")
        return 0, 0
    # Test cases: (key_hex, nonce_hex, plaintext) — vary plaintext length
    # to exercise the partial-final-block path and multi-block counter
    # increments. Max 256 bytes (Salsa20Buf capacity).
    test_cases = [
        # 1 block (64 bytes) — boundary case.
        ("80000000000000000000000000000000" "00000000000000000000000000000000",
         "0000000000000000", b"\x00" * 64),
        # 2 blocks (128 bytes) — counter increments to 1.
        ("80000000000000000000000000000000" "00000000000000000000000000000000",
         "0000000000000000", b"\x00" * 128),
        # 1.5 blocks (96 bytes) — partial final block.
        ("0053A6F94C9FF24598EB3E91E4378ADD" "3083D6297CCF2275C81B6EC11467BA0D",
         "0D74DB42A91077DE", b"\x00" * 96),
        # Non-zero plaintext (eSTREAM Set 6, vector# 0 key), 70 bytes.
        ("0053A6F94C9FF24598EB3E91E4378ADD" "3083D6297CCF2275C81B6EC11467BA0D",
         "0D74DB42A91077DE", bytes(range(70))),
        # Non-zero plaintext, exactly 1 block.
        ("0053A6F94C9FF24598EB3E91E4378ADD" "3083D6297CCF2275C81B6EC11467BA0D",
         "0D74DB42A91077DE", b"Hello, Salsa20! " * 4),  # 64 bytes
        # 4 blocks (256 bytes) — full Salsa20Buf capacity.
        ("80000000000000000000000000000000" "00000000000000000000000000000000",
         "0000000000000000", b"\x00" * 256),
        # 200 bytes — non-block-aligned, multi-block, non-zero.
        ("0053A6F94C9FF24598EB3E91E4378ADD" "3083D6297CCF2275C81B6EC11467BA0D",
         "0D74DB42A91077DE", (bytes(range(256)) * 2)[:200]),
    ]
    passed = 0
    total = 0
    for i, (key_hex, nonce_hex, pt) in enumerate(test_cases):
        total += 1
        key = bytes.fromhex(key_hex)
        nonce = bytes.fromhex(nonce_hex)
        ct = PySalsa20.new(key=key, nonce=nonce).encrypt(pt)
        driver = gen_multiblock_driver(module_src, key_hex, nonce_hex,
                                       pt, ct.hex().upper(), i)
        ok = run_extra_driver(driver, f"salsa20_multi_{i:04d}", verbose)
        if ok:
            passed += 1
    return passed, total


def main():
    verbose = "--verbose" in sys.argv or "-v" in sys.argv
    fill_expected_from_pycrypto()

    module_src = read_module()
    print(f"Salsa20 eSTREAM conformance test — {MODULE.relative_to(REPO)}")
    print(f"Vectors: {len(ESTREAM_VECTORS)}")
    print()

    total = 0
    passed = 0
    failed = 0
    per_vector_results = []
    for idx, (label, key_hex, nonce_hex, expected_hex, block_idx) in enumerate(ESTREAM_VECTORS):
        total += 1
        ok = run_vector(module_src, label, key_hex, nonce_hex, expected_hex,
                        idx, block_idx, verbose)
        per_vector_results.append((idx, label, key_hex, nonce_hex,
                                   expected_hex, block_idx, ok))
        if ok:
            passed += 1
        else:
            failed += 1

    print()
    print(f"Block-level (first 32 bytes per block): {passed}/{total} passed, {failed} failed")

    # Full 64-byte block verification.
    print()
    print("Full 64-byte block 0 verification:")
    fb_passed, fb_total = run_full_block_tests(module_src, verbose)
    print(f"  {fb_passed}/{fb_total} passed")

    # Multi-block streaming encrypt verification.
    print()
    print("Multi-block streaming encrypt (counter increment):")
    mb_passed, mb_total = run_multiblock_tests(module_src, verbose)
    print(f"  {mb_passed}/{mb_total} passed")

    grand_total = total + fb_total + mb_total
    grand_pass = passed + fb_passed + mb_passed
    grand_fail = grand_total - grand_pass
    print()
    print(f"GRAND SUMMARY: {grand_pass}/{grand_total} passed, {grand_fail} failed")

    # Write report.
    report = REPORT_DIR / "salsa20_report.md"
    with report.open("w") as f:
        f.write("# Salsa20 eSTREAM Conformance Test Report\n\n")
        f.write(f"**Date:** {datetime.now(timezone.utc).isoformat()}\n")
        f.write(f"**Module:** `{MODULE.relative_to(REPO)}`\n")
        f.write(f"**Compile driver:** `{COMPILE_DUMP.relative_to(REPO)}`\n")
        f.write(f"**Cross-check:** "
                f"{'PyCryptodome' if HAVE_PYCRYPTO else 'N/A'}\n\n")
        f.write("## Block-level vectors (first 32 bytes per block)\n\n")
        f.write("| # | Label | Block | Key (hex) | Nonce | Expected[0..31] | Result |\n")
        f.write("|---|-------|-------|-----------|-------|------------------|--------|\n")
        for idx, label, key_hex, nonce_hex, expected_hex, block_idx, ok in per_vector_results:
            result = "PASS" if ok else "FAIL"
            f.write(f"| {idx} | {label} | {block_idx} | "
                    f"`{key_hex[:16]}…` | `{nonce_hex}` | "
                    f"`{expected_hex[:16]}…` | {result} |\n")
        f.write(f"\n**Block-level summary:** {passed}/{total} passed\n\n")
        f.write("## Full 64-byte block 0 verification\n\n")
        f.write(f"**Result:** {fb_passed}/{fb_total} passed\n\n")
        f.write("## Multi-block streaming encrypt (counter increment)\n\n")
        f.write(f"**Result:** {mb_passed}/{mb_total} passed\n\n")
        f.write(f"## Grand Summary\n\n")
        f.write(f"**{grand_pass}/{grand_total} passed, {grand_fail} failed**\n")
    print(f"Report: {report.relative_to(REPO)}")
    return 0 if grand_fail == 0 else 1


if __name__ == "__main__":
    sys.exit(main())
