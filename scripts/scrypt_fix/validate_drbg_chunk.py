#!/usr/bin/env python3
"""Validate drbg vectors — chunked to avoid long-running process issues."""
import json, os, subprocess, sys

REPO = "/home/z/my-project/vuma"
COMPILE_DUMP = f"{REPO}/target/release/compile_dump"
HARNESS_DIR = f"{REPO}/tests/compact_harnesses"
VECTORS_DIR = f"{REPO}/test_results/standard_vectors"
RESULTS_FILE = f"{REPO}/test_results/compact_results.json"

sys.path.insert(0, f"{REPO}/scripts")
from validate_compact import parse_output

start = int(sys.argv[1]) if len(sys.argv) > 1 else 0
end = int(sys.argv[2]) if len(sys.argv) > 2 else 20

with open(f"{VECTORS_DIR}/drbg.json") as f:
    vectors = json.load(f)["vectors"]

passes = 0
total = 0
for i in range(start, min(end, len(vectors))):
    v = vectors[i]
    harness = f"{HARNESS_DIR}/test_drbg_b{i}.vuma"
    bin_path = f"/tmp/drbg_b{i}_x86_64.bin"
    r = subprocess.run([COMPILE_DUMP, harness, bin_path, "x86_64", "--no-verify"],
                       capture_output=True, text=True, timeout=120)
    if r.returncode != 0:
        print(f"v{i}: COMPILE FAIL", flush=True)
        total += 1
        continue
    r = subprocess.run([bin_path], capture_output=True, timeout=30)
    if r.returncode != 0:
        print(f"v{i}: RUN FAIL exit={r.returncode}", flush=True)
        total += 1
        continue
    actual = parse_output(r.stdout)
    act = actual[0] if actual else ""
    total += 1
    if act == v["expected_hex"]:
        passes += 1
        print(f"v{i}: PASS", flush=True)
    else:
        print(f"v{i}: FAIL got={act[:32]} exp={v['expected_hex'][:32]}", flush=True)

print(f"\nChunk {start}-{end-1}: {passes}/{total}", flush=True)

# Save chunk result to a temp file for aggregation
with open(f"/tmp/drbg_chunk_{start}_{end}.json", "w") as f:
    json.dump({"pass": passes, "total": total}, f)
