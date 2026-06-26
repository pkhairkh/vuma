#!/bin/bash
# ═══════════════════════════════════════════════════════════════════════════
# VUMA Full Test Suite Runner for Raspberry Pi 5 (aarch64 native)
# ═══════════════════════════════════════════════════════════════════════════
#
# WHAT THIS DOES:
#   1. Builds the VUMA compiler (if needed)
#   2. Runs all 5,738 test programs across 8 backends (45,904 total runs)
#   3. Generates a summary report
#   4. Commits results to the repo and pushes to GitHub
#
# PREREQUISITES ON PI5:
#   - Rust nightly toolchain (curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh)
#   - QEMU user-mode for cross-arch testing:
#       sudo apt install qemu-user qemu-user-static
#   - Python 3
#   - Git
#   - wasmtime (for wasm32 backend):
#       curl -sSL https://github.com/bytecodealliance/wasmtime/releases/download/v29.0.1/wasmtime-v29.0.1-aarch64-linux.tar.xz | tar xJ
#       sudo cp wasmtime-v29.0.1-aarch64-linux/wasmtime /usr/local/bin/
#
# USAGE:
#   chmod +x scripts/pi5_test_suite.sh
#   ./scripts/pi5_test_suite.sh
#
# OR with options:
#   ./scripts/pi5_test_suite.sh --workers 4        # Use 4 parallel workers
#   ./scripts/pi5_test_suite.sh --skip-build        # Skip cargo build
#   ./scripts/pi5_test_suite.sh --no-push           # Don't commit/push results
#   ./scripts/pi5_test_suite.sh --backends aarch64,x86_64  # Test specific backends only
#
# ON PI5, aarch64 runs NATIVELY (no QEMU needed). Other backends use QEMU.
# ═══════════════════════════════════════════════════════════════════════════

set -euo pipefail

# ── Parse arguments ──
WORKERS=4
SKIP_BUILD=0
NO_PUSH=0
BACKENDS=""
REPO_DIR="$(cd "$(dirname "$0")/.." && pwd)"

while [[ $# -gt 0 ]]; do
    case $1 in
        --workers) WORKERS="$2"; shift 2 ;;
        --skip-build) SKIP_BUILD=1; shift ;;
        --no-push) NO_PUSH=1; shift ;;
        --backends) BACKENDS="$2"; shift 2 ;;
        *) echo "Unknown option: $1"; exit 1 ;;
    esac
done

cd "$REPO_DIR"

echo "╔══════════════════════════════════════════════════════════════╗"
echo "║  VUMA Full Test Suite — $(date -u '+%Y-%m-%d %H:%M UTC')            ║"
echo "╠══════════════════════════════════════════════════════════════╣"
echo "║  Repo:    $REPO_DIR"
echo "║  Workers: $WORKERS"
echo "║  Host:    $(uname -m) ($(hostname))"
echo "╚══════════════════════════════════════════════════════════════╝"
echo ""

# ── Step 1: Check prerequisites ──
echo "▸ Checking prerequisites..."

missing=()
command -v cargo >/dev/null 2>&1 || missing+=("rust/cargo")
command -v python3 >/dev/null 2>&1 || missing+=("python3")
command -v git >/dev/null 2>&1 || missing+=("git")

# Check QEMU for each backend
HOST_ARCH=$(uname -m)
QEMU_NEEDED=""
for q in qemu-aarch64 qemu-x86_64 qemu-riscv64 qemu-arm qemu-mips64el qemu-ppc64 qemu-loongarch64; do
    if [ "$HOST_ARCH" = "aarch64" ] && [ "$q" = "qemu-aarch64" ]; then
        continue  # Native, no QEMU needed
    fi
    command -v $q >/dev/null 2>&1 || missing+=("$q (apt install qemu-user)")
done

# Check wasmtime
command -v wasmtime >/dev/null 2>&1 || missing+=("wasmtime")

if [ ${#missing[@]} -gt 0 ]; then
    echo "✗ Missing: ${missing[*]}"
    echo ""
    echo "Install with:"
    echo "  sudo apt install qemu-user qemu-user-static"
    echo "  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh"
    echo "  # For wasmtime, download from https://github.com/bytecodealliance/wasmtime/releases"
    exit 1
fi
echo "✓ All prerequisites found"
echo ""

# ── Step 2: Build compiler ──
if [ $SKIP_BUILD -eq 0 ]; then
    echo "▸ Building VUMA compiler (release mode)..."
    cargo build --release --bin compile_dump --bin dump_ir 2>&1 | tail -5
    echo "✓ Build complete"
    echo ""
fi

# ── Step 3: Create Python test runner ──
RESULTS_DIR="$REPO_DIR/test_results"
mkdir -p "$RESULTS_DIR"

cat > "$RESULTS_DIR/run_tests.py" << 'PYEOF'
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

# QEMU binary mapping (skip QEMU for native arch)
BACKENDS = {}
if HOST_ARCH == "aarch64":
    BACKENDS["aarch64"] = None  # Native!
else:
    BACKENDS["aarch64"] = "qemu-aarch64"
BACKENDS["x86_64"] = "qemu-x86_64"
BACKENDS["riscv64"] = "qemu-riscv64"
BACKENDS["arm32"] = "qemu-arm"
BACKENDS["mips64"] = "qemu-mips64el"
BACKENDS["ppc64"] = "qemu-ppc64"
BACKENDS["loongarch64"] = "qemu-loongarch64"
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
            cmd = ["wasmtime", "run", "--invoke", "_vuma_main", out]
        elif BACKENDS[backend] is None:
            # Native execution (no QEMU needed)
            os.chmod(out, 0o755)
            cmd = ["timeout", str(EXEC_TIMEOUT), out]
        else:
            os.chmod(out, 0o755)
            cmd = ["timeout", str(EXEC_TIMEOUT), BACKENDS[backend], out]

        try:
            ep = subprocess.run(cmd, capture_output=True, timeout=EXEC_TIMEOUT + 3)
            rc = ep.returncode
            if backend == "wasm32":
                stdout = ep.stdout.decode(errors="replace").strip()
                if rc == 0 and stdout:
                    try: result["actual"] = int(stdout)
                    except: result["actual"] = rc; result["crashed"] = True
                elif rc == 0: result["actual"] = 0
                else: result["actual"] = rc; result["crashed"] = True
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
                total_matches = matches + sum(1 for _ in [])
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
PYEOF

export REPO_DIR="$REPO_DIR"
python3 "$RESULTS_DIR/run_tests.py" --workers "$WORKERS" ${BACKENDS:+--backends "$BACKENDS"}
TEST_EXIT=$?

echo ""
echo "▸ Test suite complete (exit code: $TEST_EXIT)"

# ── Step 4: Commit and push results ──
if [ $NO_PUSH -eq 0 ]; then
    echo "▸ Committing results..."
    cd "$REPO_DIR"
    git add test_results/ 2>/dev/null || true
    git add scripts/pi5_test_suite.sh 2>/dev/null || true

    TIMESTAMP=$(date -u '+%Y-%m-%d_%H%M-UTC')
    git commit -m "test: Full suite results ($TIMESTAMP)

$(cat test_results/summary.json 2>/dev/null || echo 'See test_results/ for details')" 2>/dev/null || echo "(nothing to commit)"

    echo "▸ Pushing to GitHub..."
    git push origin HEAD 2>&1 | tail -3 || echo "(push failed — check git remote)"
    echo "✓ Done"
else
    echo "▸ Skipping commit/push (--no-push)"
fi

echo ""
echo "╔══════════════════════════════════════════════════════════════╗"
echo "║  Results:                                                   ║"
cat test_results/summary.json 2>/dev/null | python3 -c "
import json, sys
try:
    s = json.load(sys.stdin)
    print(f'║  Pass rate: {s[\"pass_rate\"]} ({s[\"matches\"]}/{s[\"total_runs\"]})')
    for b, v in sorted(s.get('per_backend', {}).items()):
        pct = 100*v['match']/v['total'] if v['total'] else 0
        print(f'║    {b:14s} {v[\"match\"]:5d}/{v[\"total\"]:5d} = {pct:.2f}%')
except: print('║  (see test_results/summary.json)')
" 2>/dev/null || echo "║  (see test_results/summary.json)"
echo "╚══════════════════════════════════════════════════════════════╝"
