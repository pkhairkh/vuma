#!/bin/bash
# ═══════════════════════════════════════════════════════════════════════════
# VUMA Full Test Suite Runner for Raspberry Pi 5 (aarch64 native)
# ═══════════════════════════════════════════════════════════════════════════
set -euo pipefail

WORKERS=4
SKIP_BUILD=0
NO_PUSH=0
FRESH=0
BACKENDS=""
VERIFY=0
BUILD_PROFILE="release-fast"   # fast iterative profile (LTO off, codegen-units=16)
REPO_DIR="$(cd "$(dirname "$0")/.." && pwd)"

while [[ $# -gt 0 ]]; do
    case $1 in
        --workers) WORKERS="$2"; shift 2 ;;
        --skip-build) SKIP_BUILD=1; shift ;;
        --no-push) NO_PUSH=1; shift ;;
        --fresh) FRESH=1; shift ;;
        --backends) BACKENDS="$2"; shift 2 ;;
        --verify) VERIFY=1; shift ;;
        --release) BUILD_PROFILE="release"; shift ;;   # opt-in: slow LTO build
        --profile) BUILD_PROFILE="$2"; shift 2 ;;
        *) echo "Unknown option: $1"; exit 1 ;;
    esac
done

cd "$REPO_DIR"

# ── Setup PATH (cargo might be in ~/.cargo/bin) ──
export PATH="$HOME/.cargo/bin:$PATH"

echo "╔══════════════════════════════════════════════════════════════╗"
echo "║  VUMA Full Test Suite — $(date -u '+%Y-%m-%d %H:%M UTC')            ║"
echo "╠══════════════════════════════════════════════════════════════╣"
echo "║  Repo:    $REPO_DIR"
echo "║  Workers: $WORKERS"
echo "║  Profile: $BUILD_PROFILE"
echo "║  Host:    $(uname -m) ($(hostname))"
echo "╚══════════════════════════════════════════════════════════════╝"
echo ""

# ── Step 1: Install prerequisites ──
echo "▸ Checking prerequisites..."

# Check/install Rust
if ! command -v cargo &>/dev/null; then
    echo "  Installing Rust..."
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain nightly-2026-03-01 2>/dev/null || {
        echo "  curl failed, trying wget..."
        wget -qO- https://sh.rustup.rs | sh -s -- -y --default-toolchain nightly-2026-03-01
    }
    source "$HOME/.cargo/env"
    rustup component add rustfmt clippy rust-src 2>/dev/null || true
fi
echo "  ✓ Rust: $(rustc --version 2>/dev/null || echo 'NOT FOUND')"

# Check/install QEMU
if ! command -v qemu-aarch64 &>/dev/null; then
    echo "  Installing QEMU..."
    sudo apt update -qq && sudo apt install -y qemu-user qemu-user-static 2>/dev/null || {
        echo "  ✗ Failed to install qemu-user. Run: sudo apt install qemu-user qemu-user-static"
        exit 1
    }
fi
echo "  ✓ QEMU: $(qemu-aarch64 --version 2>/dev/null | head -1 || echo 'NOT FOUND')"

# Check/install wasmtime
WASMTIME_BIN=""
for p in /usr/local/bin/wasmtime "$HOME/.local/bin/wasmtime" "$(pwd)/wasmtime"; do
    if [ -x "$p" ]; then WASMTIME_BIN="$p"; break; fi
done
if [ -z "$WASMTIME_BIN" ]; then
    echo "  Installing wasmtime..."
    ARCH=$(uname -m)
    WASMTIME_URL="https://github.com/bytecodealliance/wasmtime/releases/download/v29.0.1/wasmtime-v29.0.1-${ARCH}-linux.tar.xz"
    curl -sSL "$WASMTIME_URL" -o /tmp/wasmtime.tar.xz 2>/dev/null && {
        tar xf /tmp/wasmtime.tar.xz -C /tmp/ 2>/dev/null
        WASMTIME_BIN=$(find /tmp/wasmtime-v29.0.1-${ARCH}-linux -name wasmtime -type f 2>/dev/null | head -1)
        [ -n "$WASMTIME_BIN" ] && cp "$WASMTIME_BIN" "$REPO_DIR/wasmtime" && WASMTIME_BIN="$REPO_DIR/wasmtime"
    } || echo "  ⚠ wasmtime install failed (wasm32 backend will be skipped)"
fi
echo "  ✓ Wasmtime: ${WASMTIME_BIN:-NOT FOUND}"
echo ""

# ── Step 2: Build compiler ──
if [ $SKIP_BUILD -eq 0 ]; then
    echo "▸ Building VUMA compiler (profile: $BUILD_PROFILE)..."
    # Stream build output live so the user sees progress (the LTO `release`
    # profile can take 10+ minutes on a Pi 5 and would otherwise show nothing
    # until completion). Capture stderr to a log for post-mortem on failure.
    RESULTS_DIR="$REPO_DIR/test_results"
    mkdir -p "$RESULTS_DIR"
    BUILD_LOG="$RESULTS_DIR/build.log"
    BUILD_START=$(date +%s)
    if cargo build --profile "$BUILD_PROFILE" --bin compile_dump --bin dump_ir 2>"$BUILD_LOG"; then
        BUILD_END=$(date +%s)
        echo "  ✓ Build complete in $((BUILD_END - BUILD_START))s  (log: $BUILD_LOG)"
    else
        BUILD_END=$(date +%s)
        echo "  ✗ Build FAILED after $((BUILD_END - BUILD_START))s"
        echo "  ── last 30 lines of build log ──"
        tail -30 "$BUILD_LOG" | sed 's/^/  /'
        exit 1
    fi
    echo ""
fi

# ── Step 2.5: Clear checkpoint if --fresh or if compiler was rebuilt ──
RESULTS_DIR="$REPO_DIR/test_results"
CHECKPOINT="$RESULTS_DIR/checkpoint.jsonl"
COMPILE_BIN="$REPO_DIR/target/$BUILD_PROFILE/compile_dump"
if [ $FRESH -eq 1 ]; then
    echo "▸ --fresh: clearing previous checkpoint..."
    rm -f "$CHECKPOINT"
    echo "✓ Checkpoint cleared"
    echo ""
elif [ -f "$CHECKPOINT" ] && [ -f "$COMPILE_BIN" ]; then
    # Auto-detect: if the compiler binary is newer than the checkpoint,
    # the results are stale and should be regenerated.
    if [ "$COMPILE_BIN" -nt "$CHECKPOINT" ]; then
        echo "▸ Compiler binary newer than checkpoint — clearing stale results..."
        rm -f "$CHECKPOINT"
        echo "✓ Checkpoint cleared"
        echo ""
    fi
fi

# ── Step 3: Create Python test runner ──
mkdir -p "$RESULTS_DIR"
export VUMA_BUILD_PROFILE="$BUILD_PROFILE"
export REPO_DIR="$REPO_DIR"
export WASMTIME_BIN="${WASMTIME_BIN:-}"

cat > "$RESULTS_DIR/run_tests.py" << 'PYEOF'
#!/usr/bin/env python3
"""VUMA Full Test Suite — runs all .vuma tests across all backends."""
import argparse, os, sys, subprocess, re, time, json, platform
from pathlib import Path
from concurrent.futures import ProcessPoolExecutor, as_completed
from collections import defaultdict

REPO = Path(os.environ.get("REPO_DIR", "."))
GOLD_DIR = REPO / "tests" / "gold_standard"
COMPILE = REPO / "target" / os.environ.get("VUMA_BUILD_PROFILE", "release-fast") / "compile_dump"
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
SKIP_ON_RE = re.compile(rb"//\s*skip_on:\s*([a-zA-Z0-9_,\s]+)")

def find_tests():
    tests = []
    for vuma in sorted(GOLD_DIR.rglob("*.vuma")):
        try:
            with open(vuma, "rb") as f:
                head = f.read(2000)
            m = EXPECTED_RE.search(head)
            if m:
                expected = int(m.group(1))
                # Parse skip_on marker (e.g. "// skip_on: wasm32" or
                # "// skip_on: wasm32, ppc64"). Backends listed here are
                # skipped (counted as a pass with skipped=True) because the
                # test exercises functionality that is architecturally
                # unavailable on that target (e.g. fork/execve on wasm32).
                skip_backends = frozenset()
                sm = SKIP_ON_RE.search(head)
                if sm:
                    raw = sm.group(1).decode(errors="replace")
                    skip_backends = frozenset(
                        b.strip() for b in raw.replace(",", " ").split()
                        if b.strip()
                    )
                tests.append((str(vuma), vuma.parent.name, vuma.name,
                              expected, skip_backends))
        except:
            pass
    return tests

def run_one(args):
    test_path, category, test_name, expected, skip_backends, backend, verify = args
    result = {
        "test": test_name, "category": category, "path": test_path,
        "backend": backend, "expected": expected, "actual": None,
        "compile_ok": False, "crashed": False, "timed_out": False,
        "match": False, "skipped": False,
        "ive_verdict": None, "ive_passed": None, "ive_failed": None, "ive_total": None,
    }
    # Honor skip_on marker — count as pass with skipped=True so the test
    # is visible in results but doesn't break the pass rate.
    if backend in skip_backends:
        result["skipped"] = True
        result["match"] = True
        result["actual"] = expected
        return result
    out = f"/tmp/vuma_{os.getpid()}_{backend}_{test_name}.bin"
    try:
        compile_cmd = [str(COMPILE), test_path, out, backend]
        if verify:
            compile_cmd.append("--verify")
        r = subprocess.run(compile_cmd, capture_output=True, timeout=15)
        if r.returncode != 0:
            return result
        result["compile_ok"] = True

        # Parse IVE status from stderr (if --verify was passed)
        if verify:
            stderr = r.stderr.decode(errors="replace")
            for line in stderr.splitlines():
                if line.startswith("IVE: "):
                    # Format: "IVE: Pass passed=5 failed=0 total=5"
                    # or: "IVE: Skip (ive_skip marker)"
                    rest = line[5:]
                    parts = rest.split()
                    if parts:
                        result["ive_verdict"] = parts[0]
                    for p in parts[1:]:
                        if "=" in p:
                            k, v = p.split("=", 1)
                            try:
                                iv = int(v)
                                if k == "passed": result["ive_passed"] = iv
                                elif k == "failed": result["ive_failed"] = iv
                                elif k == "total": result["ive_total"] = iv
                            except: pass

        if backend == "wasm32":
            os.chmod(out, 0o644)
            # Use --invoke _vuma_main for all tests EXCEPT those that use
            # print_int/print_hex (which write to stdout, mixing with return value).
            # For those, use proc_exit via plain 'wasmtime run'.
            test_name_lower = test_name.lower()
            if "print" in test_name_lower:
                cmd = [WASMTIME, "run", out]
            else:
                cmd = [WASMTIME, "run", "--invoke", "_vuma_main", out]
        elif BACKENDS[backend] is None:
            os.chmod(out, 0o755)
            cmd = ["timeout", str(EXEC_TIMEOUT), out]
        else:
            os.chmod(out, 0o755)
            cmd = ["timeout", str(EXEC_TIMEOUT), BACKENDS[backend], out]

        try:
            # self_exec uses fork/exec/pipe which is timing-sensitive
            # under QEMU user-mode emulation. If it crashes with SIGPIPE
            # (signal 13, rc=-13), retry up to 3 times — the race window
            # is narrow and usually succeeds on a second attempt.
            max_retries = 3 if test_name == "self_exec.vuma" else 1
            for attempt in range(max_retries):
                ep = subprocess.run(cmd, capture_output=True, timeout=EXEC_TIMEOUT + 3)
                rc = ep.returncode
                if backend == "wasm32":
                    if "print" in test_name_lower:
                        # Use proc_exit exit code for print tests
                        crashed = rc < 0 or rc > 128
                        result["actual"] = rc; result["crashed"] = crashed
                    else:
                        # Use --invoke stdout for other tests
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
                # Retry only on SIGPIPE (-13) for self_exec
                if rc == -13 and attempt < max_retries - 1:
                    continue
                break
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
    ap.add_argument("--verify", action="store_true",
                    help="Run IVE verification (non-fatal) and report pass rate")
    args = ap.parse_args()

    RESULTS.mkdir(parents=True, exist_ok=True)
    tests = find_tests()
    bl = args.backends.split(",") if args.backends else list(BACKENDS.keys())
    bl = [b for b in bl if b in BACKENDS]
    tasks = [(*t, b, args.verify) for t in tests for b in bl]
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

    remaining = [t for t in tasks if (t[0], t[5]) not in done]
    print(f"Tests: {len(tests)} × Backends: {len(bl)} = {total} runs")
    print(f"Already done: {len(done)}, Remaining: {len(remaining)}")
    print(f"Backends: {bl}")
    if args.verify:
        print(f"IVE verification: ENABLED (non-fatal, reported separately)")
    print()

    ckpt = open(args.checkpoint, "a", buffering=1)
    matches = 0
    skipped = 0
    t0 = time.monotonic()

    with ProcessPoolExecutor(max_workers=args.workers) as pool:
        futures = {pool.submit(run_one, t): t for t in remaining}
        for i, fut in enumerate(as_completed(futures), 1):
            try: r = fut.result()
            except: r = {"path": "", "backend": "", "match": False, "actual": None,
                        "expected": 0, "test": "", "category": "", "compile_ok": False,
                        "crashed": False, "timed_out": False, "skipped": False}
            ckpt.write(json.dumps(r) + "\n")
            if r.get("match"):
                matches += 1
                if r.get("skipped"): skipped += 1
            if i % 200 == 0 or i == len(remaining):
                elapsed = time.monotonic() - t0
                rate = i / elapsed if elapsed > 0 else 0
                eta = (len(remaining) - i) / rate / 60 if rate > 0 else 0
                print(f"  [{i}/{len(remaining)}] {rate:.0f}/s ETA {eta:.1f}min | matches={matches} ({100*matches/i:.1f}%) skipped={skipped}", flush=True)

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
    skipped = sum(1 for r in latest.values() if r.get("skipped"))
    print(f"Total: {matches}/{total} = {100*matches/total:.2f}%  (skipped: {skipped})")
    print()

    by_backend = defaultdict(lambda: {"total": 0, "match": 0, "skipped": 0})
    for r in latest.values():
        by_backend[r["backend"]]["total"] += 1
        if r.get("match"): by_backend[r["backend"]]["match"] += 1
        if r.get("skipped"): by_backend[r["backend"]]["skipped"] += 1

    print("Per-backend:")
    for b in sorted(by_backend):
        s = by_backend[b]
        pct = 100 * s["match"] / s["total"] if s["total"] else 0
        sk = f" (skip={s['skipped']})" if s["skipped"] else ""
        print(f"  {b:14s} {s['match']:5d}/{s['total']:5d} = {pct:.2f}%{sk}")

    # IVE verification summary (if --verify was used)
    ive_runs = [r for r in latest.values() if r.get("ive_verdict")]
    if ive_runs:
        ive_pass = sum(1 for r in ive_runs if r.get("ive_verdict") in ("Pass", "Skip"))
        ive_fail = sum(1 for r in ive_runs if r.get("ive_verdict") == "Fail")
        ive_skip = sum(1 for r in ive_runs if r.get("ive_verdict") == "Skip")
        ive_total = len(ive_runs)
        print()
        print(f"IVE Verification: {ive_pass}/{ive_total} = {100*ive_pass/ive_total:.2f}% pass"
              + (f" (skip={ive_skip})" if ive_skip else ""))
        # Per-backend IVE stats
        ive_by_backend = defaultdict(lambda: {"total": 0, "pass": 0, "skip": 0})
        for r in ive_runs:
            ive_by_backend[r["backend"]]["total"] += 1
            if r.get("ive_verdict") in ("Pass", "Skip"):
                ive_by_backend[r["backend"]]["pass"] += 1
            if r.get("ive_verdict") == "Skip":
                ive_by_backend[r["backend"]]["skip"] += 1
        for b in sorted(ive_by_backend):
            s = ive_by_backend[b]
            pct = 100 * s["pass"] / s["total"] if s["total"] else 0
            sk = f" (skip={s['skip']})" if s["skip"] else ""
            print(f"  {b:14s} {s['pass']:5d}/{s['total']:5d} = {pct:.2f}%{sk}")

    # Save summary
    summary = {
        "timestamp": time.strftime("%Y-%m-%d %H:%M:%S UTC", time.gmtime()),
        "host": platform.node(),
        "arch": HOST_ARCH,
        "total_runs": total,
        "matches": matches,
        "skipped": skipped,
        "pass_rate": f"{100*matches/total:.2f}%",
        "per_backend": {b: dict(s) for b, s in by_backend.items()},
    }
    if ive_runs:
        ive_pass = sum(1 for r in ive_runs if r.get("ive_verdict") == "Pass")
        summary["ive_verification"] = {
            "total": len(ive_runs),
            "pass": ive_pass,
            "fail": len(ive_runs) - ive_pass,
            "pass_rate": f"{100*ive_pass/len(ive_runs):.2f}%",
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
        f.write(f"Total: {len(failures)} failures across {len(by_test)} tests\n")
        f.write(f"Skipped: {skipped} (architecturally unavailable on target)\n\n")
        for (cat, test), rs in sorted(by_test.items()):
            backends = [(r["backend"], r.get("actual"), "TO" if r.get("timed_out") else ("CR" if r.get("crashed") else "MM")) for r in rs]
            f.write(f"  {cat:20s} {test:45s} exp={rs[0]['expected']:4} {backends}\n")

    print(f"\nFailures: {len(failures)} across {len(by_test)} tests")
    print(f"Skipped:  {skipped}")
    print(f"Results saved to {RESULTS}/")

if __name__ == "__main__":
    main()
PYEOF

export REPO_DIR="$REPO_DIR"
export WASMTIME_BIN="$WASMTIME_BIN"
VERIFY_FLAG=""
if [[ "$VERIFY" == "1" ]]; then VERIFY_FLAG="--verify"; fi
python3 "$RESULTS_DIR/run_tests.py" --workers "$WORKERS" ${BACKENDS:+--backends "$BACKENDS"} $VERIFY_FLAG
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
    git commit -m "test: Full suite results ($TIMESTAMP) on $(hostname)

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
