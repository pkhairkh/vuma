#!/usr/bin/env python3
"""VUMA Full Test Suite — runs all .vuma tests across all backends."""
import argparse, os, sys, subprocess, re, time, json, platform
from pathlib import Path
from concurrent.futures import ProcessPoolExecutor, as_completed
from collections import defaultdict

REPO = Path(os.environ.get("REPO_DIR", "."))
GOLD_DIR = REPO / "tests" / "gold_standard"
COMPILE = REPO / "target" / "release" / "compile_dump"
RESULTS = REPO / "test_results"
HOST_ARCH = platform.machine()

# QEMU binary mapping
BACKENDS = {}
# Always use QEMU for all backends (even native aarch64)
# This ensures consistent ELF loading behavior
BACKENDS["aarch64"] = "qemu-aarch64"
BACKENDS["x86_64"] = "qemu-x86_64"
BACKENDS["riscv64"] = "qemu-riscv64"
BACKENDS["arm32"] = "qemu-arm"
BACKENDS["mips64"] = "qemu-mips64el"
BACKENDS["ppc64"] = "qemu-ppc64"
BACKENDS["loongarch64"] = "qemu-loongarch64"
BACKENDS["riscv32"] = "qemu-riscv32"
BACKENDS["x86_32"] = "qemu-i386"

# Check wasmtime
WASMTIME = os.environ.get("WASMTIME_BIN", "")
if WASMTIME and os.path.isfile(WASMTIME):
    BACKENDS["wasm32"] = "WASMTIME"
elif os.path.isfile(str(REPO / "wasmtime")):
    WASMTIME = str(REPO / "wasmtime")
    BACKENDS["wasm32"] = "WASMTIME"
else:
    # Try PATH
    import shutil
    if shutil.which("wasmtime"):
        WASMTIME = "wasmtime"
        BACKENDS["wasm32"] = "WASMTIME"

EXEC_TIMEOUT = 5
EXPECTED_RE = re.compile(rb"//\s*Expected exit code:\s*(-?\d+)")

def find_tests():
    tests = []
    for vuma in sorted(GOLD_DIR.rglob("*.vuma")):
        try:
            with open(vuma, "rb") as f:
                head = f.read(2000)
            m = EXPECTED_RE.search(head)
            if m:
                tests.append((str(vuma), vuma.parent.name, vuma.name, int(m.group(1))))
        except:
            pass
    return tests

def run_one(args):
    test_path, category, test_name, expected, backend = args
    result = {
        "test": test_name, "category": category, "path": test_path,
        "backend": backend, "expected": expected, "actual": None,
        "compile_ok": False, "crashed": False, "timed_out": False, "match": False,
    }
    out = f"/tmp/vuma_{os.getpid()}_{backend}_{test_name}.bin"
    try:
        r = subprocess.run([str(COMPILE), test_path, out, backend], capture_output=True, timeout=15)
        if r.returncode != 0:
            return result
        result["compile_ok"] = True

        if backend == "wasm32":
            os.chmod(out, 0o644)
            cmd = [WASMTIME, "run", out]
        elif BACKENDS[backend] is None:
            os.chmod(out, 0o755)
            cmd = ["timeout", str(EXEC_TIMEOUT), out]
        else:
            os.chmod(out, 0o755)
            cmd = ["timeout", str(EXEC_TIMEOUT), BACKENDS[backend], out]

        try:
            ep = subprocess.run(cmd, capture_output=True, timeout=EXEC_TIMEOUT + 3)
            rc = ep.returncode
            if backend == "wasm32":
                # Use proc_exit exit code (same as other backends)
                # This fixes test_print where --invoke mixed stdout with return value
                crashed = rc < 0 or rc > 128
                result["actual"] = rc; result["crashed"] = crashed
            elif rc == 124:
                result["timed_out"] = True; result["actual"] = 124
            else:
                stderr = ep.stderr.decode(errors="replace")
                crashed = "Segmentation fault" in stderr or "uncaught target signal" in stderr or rc == 139 or rc == 134 or rc < 0
                result["actual"] = rc; result["crashed"] = crashed
        except subprocess.TimeoutExpired:
            result["timed_out"] = True; result["actual"] = 124
    except:
        pass
    finally:
        try: os.remove(out)
        except: pass

    if result["actual"] is not None:
        a = result["actual"] & 0xFF if result["actual"] >= 0 else result["actual"]
        e = expected & 0xFF if expected >= 0 else expected
        result["match"] = (a == e)
    return result

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--workers", type=int, default=4)
    ap.add_argument("--backends", default=None)
    ap.add_argument("--checkpoint", default=str(RESULTS / "checkpoint.jsonl"))
    args = ap.parse_args()

    RESULTS.mkdir(parents=True, exist_ok=True)
    tests = find_tests()
    bl = args.backends.split(",") if args.backends else list(BACKENDS.keys())
    bl = [b for b in bl if b in BACKENDS]
    tasks = [(*t, b) for t in tests for b in bl]
    total = len(tasks)

    # Resume support
    done = set()
    if os.path.exists(args.checkpoint):
        with open(args.checkpoint) as f:
            for line in f:
                try:
                    r = json.loads(line)
                    done.add((r["path"], r["backend"]))
                except: pass

    remaining = [t for t in tasks if (t[0], t[4]) not in done]
    print(f"Tests: {len(tests)} × Backends: {len(bl)} = {total} runs")
    print(f"Already done: {len(done)}, Remaining: {len(remaining)}")
    print(f"Backends: {bl}")
    print()

    ckpt = open(args.checkpoint, "a", buffering=1)
    matches = 0
    t0 = time.monotonic()

    with ProcessPoolExecutor(max_workers=args.workers) as pool:
        futures = {pool.submit(run_one, t): t for t in remaining}
        for i, fut in enumerate(as_completed(futures), 1):
            try: r = fut.result()
            except: r = {"path": "", "backend": "", "match": False, "actual": None,
                        "expected": 0, "test": "", "category": "", "compile_ok": False,
                        "crashed": False, "timed_out": False}
            ckpt.write(json.dumps(r) + "\n")
            if r.get("match"): matches += 1
            if i % 200 == 0 or i == len(remaining):
                elapsed = time.monotonic() - t0
                rate = i / elapsed if elapsed > 0 else 0
                eta = (len(remaining) - i) / rate / 60 if rate > 0 else 0
                print(f"  [{i}/{len(remaining)}] {rate:.0f}/s ETA {eta:.1f}min | matches={matches} ({100*matches/i:.1f}%)", flush=True)

    ckpt.close()
    elapsed = time.monotonic() - t0
    print(f"\n{'='*60}")
    print(f"Completed {len(remaining)} runs in {elapsed/60:.1f} minutes")

    # Generate final report
    latest = {}
    with open(args.checkpoint) as f:
        for line in f:
            try:
                r = json.loads(line)
                latest[(r["path"], r["backend"])] = r
            except: pass

    total = len(latest)
    matches = sum(1 for r in latest.values() if r.get("match"))
    print(f"Total: {matches}/{total} = {100*matches/total:.2f}%")
    print()

    by_backend = defaultdict(lambda: {"total": 0, "match": 0})
    for r in latest.values():
        by_backend[r["backend"]]["total"] += 1
        if r.get("match"): by_backend[r["backend"]]["match"] += 1

    print("Per-backend:")
    for b in sorted(by_backend):
        s = by_backend[b]
        pct = 100 * s["match"] / s["total"] if s["total"] else 0
        print(f"  {b:14s} {s['match']:5d}/{s['total']:5d} = {pct:.2f}%")

    # Save summary
    summary = {
        "timestamp": time.strftime("%Y-%m-%d %H:%M:%S UTC", time.gmtime()),
        "host": platform.node(),
        "arch": HOST_ARCH,
        "total_runs": total,
        "matches": matches,
        "pass_rate": f"{100*matches/total:.2f}%",
        "per_backend": {b: dict(s) for b, s in by_backend.items()},
    }
    with open(RESULTS / "summary.json", "w") as f:
        json.dump(summary, f, indent=2)

    # List failures
    failures = [r for r in latest.values() if not r.get("match")]
    by_test = defaultdict(list)
    for r in failures:
        by_test[(r["category"], r["test"])].append(r)

    with open(RESULTS / "failures.txt", "w") as f:
        f.write(f"VUMA Test Failures — {summary['timestamp']}\n")
        f.write(f"Total: {len(failures)} failures across {len(by_test)} tests\n\n")
        for (cat, test), rs in sorted(by_test.items()):
            backends = [(r["backend"], r.get("actual"), "TO" if r.get("timed_out") else ("CR" if r.get("crashed") else "MM")) for r in rs]
            f.write(f"  {cat:20s} {test:45s} exp={rs[0]['expected']:4} {backends}\n")

    print(f"\nFailures: {len(failures)} across {len(by_test)} tests")
    print(f"Results saved to {RESULTS}/")

if __name__ == "__main__":
    main()
