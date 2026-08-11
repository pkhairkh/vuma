#!/usr/bin/env python3
"""Run each scrypt harness on x86_64 and compare to expected."""
import json, os, subprocess, sys

REPO = "/home/z/my-project/vuma"
COMPILE_DUMP = f"{REPO}/target/release/compile_dump"
HARNESS_DIR = f"{REPO}/tests/compact_harnesses"
VECTORS_DIR = f"{REPO}/test_results/standard_vectors"

sys.path.insert(0, f"{REPO}/scripts")
from validate_compact import parse_output

with open(f"{VECTORS_DIR}/scrypt.json") as f:
    vectors = json.load(f)["vectors"]

pass_count = 0
total = len(vectors)
for i, v in enumerate(vectors):
    harness = f"{HARNESS_DIR}/test_scrypt_b{i}.vuma"
    bin_path = f"/tmp/scrypt_b{i}.bin"
    # Compile
    r = subprocess.run([COMPILE_DUMP, harness, bin_path, "x86_64", "--no-verify"],
                       capture_output=True, text=True, timeout=60)
    if r.returncode != 0:
        print(f"  v{i}: COMPILE FAIL: {r.stderr[:200]}")
        continue
    # Run
    r = subprocess.run([bin_path], capture_output=True, timeout=30)
    if r.returncode != 0:
        print(f"  v{i}: RUN FAIL (exit {r.returncode})")
        continue
    actual = parse_output(r.stdout)
    expected = v["expected_hex"]
    act = actual[0] if actual else ""
    if act == expected:
        print(f"  v{i}: PASS ({v['desc'][:60]})")
        pass_count += 1
    else:
        print(f"  v{i}: FAIL ({v['desc'][:60]})")
        print(f"       got      : {act[:64]}...")
        print(f"       expected : {expected[:64]}...")

print(f"\nTotal: {pass_count}/{total}")
