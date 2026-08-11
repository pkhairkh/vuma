#!/usr/bin/env python3
"""Validate bignum2048 vectors on x86_64."""
import json, os, subprocess, sys

REPO = "/home/z/my-project/vuma"
COMPILE_DUMP = f"{REPO}/target/release/compile_dump"
HARNESS_DIR = f"{REPO}/tests/compact_harnesses"
VECTORS_DIR = f"{REPO}/test_results/standard_vectors"
RESULTS_FILE = f"{REPO}/test_results/compact_results.json"

sys.path.insert(0, f"{REPO}/scripts")
from validate_compact import parse_output

MODULE = "bignum2048"
BACKEND = "x86_64"

with open(f"{VECTORS_DIR}/{MODULE}.json") as f:
    vectors = json.load(f)["vectors"]

total_pass = 0
total_vecs = 0
status = "PASS"

for i, v in enumerate(vectors):
    harness = f"{HARNESS_DIR}/test_{MODULE}_b{i}.vuma"
    bin_path = f"/tmp/{MODULE}_b{i}_{BACKEND}.bin"
    r = subprocess.run([COMPILE_DUMP, harness, bin_path, BACKEND, "--no-verify"],
                       capture_output=True, text=True, timeout=120)
    if r.returncode != 0:
        print(f"  v{i}: COMPILE FAIL", flush=True)
        total_vecs += 1; status = "CERR"; continue
    r = subprocess.run([bin_path], capture_output=True, timeout=30)
    if r.returncode != 0:
        print(f"  v{i}: RUN FAIL exit={r.returncode}", flush=True)
        total_vecs += 1; status = "PARTIAL"; continue
    actual = parse_output(r.stdout)
    act = actual[0] if actual else ""
    total_vecs += 1
    if act == v["expected_hex"]:
        total_pass += 1
        print(f"  v{i}: PASS", flush=True)
    else:
        status = "PARTIAL"
        print(f"  v{i}: FAIL got={act[:32]} exp={v['expected_hex'][:32]}", flush=True)

with open(RESULTS_FILE) as f:
    results = json.load(f)
results[f"{MODULE}|{BACKEND}"] = {"status": status, "pass": total_pass, "total": total_vecs}
with open(RESULTS_FILE, "w") as f:
    json.dump(results, f, indent=2)
print(f"\n{MODULE}|{BACKEND}: {total_pass}/{total_vecs} ({status})", flush=True)
