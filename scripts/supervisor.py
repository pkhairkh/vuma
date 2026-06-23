#!/usr/bin/env python3
"""
Supervisor: launches run_one_batch.py for each backend in chunks of N files.
Survives crashes by relaunching. Runs backends in parallel (one process per
backend, processing files in chunks of 500).

Usage: python3 supervisor.py [chunk_size] [parallel_backends]
"""

import os
import subprocess
import sys
import time
from pathlib import Path

REPO_ROOT = Path("/home/z/my-project/vuma")
GOLD_DIR = REPO_ROOT / "tests/gold_standard"
OUT_DIR = Path("/home/z/my-project/download/gold_standard_results")
LOG_DIR = Path("/tmp/vuma_logs")
LOG_DIR.mkdir(parents=True, exist_ok=True)

BACKENDS = ["aarch64", "riscv64", "arm32", "mips64", "ppc64", "loongarch64"]
# x86_64 already done

CHUNK = int(sys.argv[1]) if len(sys.argv) > 1 else 500
PARALLEL = int(sys.argv[2]) if len(sys.argv) > 2 else 6  # all 6 QEMU backends in parallel


def count_files():
    return len(sorted(GOLD_DIR.rglob("*.vuma")))


def count_done(backend):
    p = OUT_DIR / f"{backend}.tsv"
    if not p.exists():
        return 0
    with p.open() as f:
        return sum(1 for _ in f) - 1  # minus header


def main():
    total = count_files()
    print(f"Total files per backend: {total}")
    print(f"Chunk size: {CHUNK}")
    print(f"Parallel backends: {PARALLEL}")
    print(f"Backends: {BACKENDS[:PARALLEL]}")
    print()

    # Run ONE round: launch one chunk per backend in parallel, wait, exit.
    # Caller (shell loop) relaunches us until all backends reach `total`.
    active = []
    for b in BACKENDS[:PARALLEL]:
        done = count_done(b)
        if done >= total:
            print(f"  {b}: ALREADY DONE ({done}/{total})")
            continue
        start = done
        count = min(CHUNK, total - start)
        log_path = LOG_DIR / f"{b}_chunk_{start}.log"
        log_f = log_path.open("w")
        p = subprocess.Popen(
            ["python3", str(REPO_ROOT / "scripts/run_one_batch.py"),
             b, str(start), str(count)],
            stdout=log_f, stderr=subprocess.STDOUT,
        )
        active.append((b, start, count, p, log_f))
        print(f"  {b}: launching chunk start={start} count={count}")

    print()
    print("=== Waiting for round to finish ===")
    for b, start, count, p, log_f in active:
        p.wait()
        log_f.close()
        done_now = count_done(b)
        print(f"[{time.strftime('%H:%M:%S')}] {b} ({start}+{count}) done — "
              f"total {done_now}/{total}", flush=True)

    print()
    print("=== Round complete ===")
    for b in BACKENDS[:PARALLEL]:
        print(f"  {b}: {count_done(b)}/{total}")


if __name__ == "__main__":
    main()
