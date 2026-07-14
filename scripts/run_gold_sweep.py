#!/usr/bin/env python3
"""
Run all 5754 VUMA gold standard programs across all 7 native backends
and aggregate pass/fail statistics.

Wasm32 is excluded (no native execution available in this environment).

Output:
  /home/z/my-project/download/gold_standard_results/
    <backend>.tsv          - per-program results
    summary.txt            - aggregated stats per backend & category
    summary.json           - machine-readable stats
"""

import json
import os
import re
import subprocess
import sys
import time
from concurrent.futures import ProcessPoolExecutor, as_completed
from pathlib import Path

REPO_ROOT = Path("/home/z/my-project/vuma")
COMPILE_DUMP = REPO_ROOT / "target/release/compile_dump"
GOLD_DIR = REPO_ROOT / "tests/gold_standard"
QEMU_DIR = Path("/tmp/qemu/extracted/usr/bin")
OUT_DIR = Path("/home/z/my-project/download/gold_standard_results")
OUT_DIR.mkdir(parents=True, exist_ok=True)

# Backend -> qemu binary (empty = native execution)
BACKENDS = {
    "x86_64": "",
    "aarch64": str(QEMU_DIR / "qemu-aarch64"),
    "riscv64": str(QEMU_DIR / "qemu-riscv64"),
    "arm32": str(QEMU_DIR / "qemu-arm"),
    "mips64": str(QEMU_DIR / "qemu-mips64el"),
    "ppc64": str(QEMU_DIR / "qemu-ppc64"),
    "loongarch64": str(QEMU_DIR / "qemu-loongarch64"),
}

# Expected exit code regex (matches "// Expected exit code: 42" etc.)
EXPECTED_RE = re.compile(r"^//\s*[Ee]xpected\s+exit\s+code\s*:\s*(-?\d+)", re.MULTILINE)


def find_vuma_files():
    files = sorted(GOLD_DIR.rglob("*.vuma"))
    return files


def category_of(path: Path) -> str:
    """Return the category name (top-level subdirectory of gold_standard)."""
    try:
        return path.relative_to(GOLD_DIR).parts[0]
    except Exception:
        return "_unknown"


def run_one(args):
    """Compile + execute one (backend, vuma_file) tuple. Returns a result dict."""
    backend, vuma_file_str, qemu = args
    vuma_file = Path(vuma_file_str)
    rel = str(vuma_file.relative_to(GOLD_DIR))
    out_bin = Path(f"/tmp/vuma_bin_{os.getpid()}.bin")

    # Parse expected exit code from header
    try:
        text = vuma_file.read_text(errors="replace")
        m = EXPECTED_RE.search(text)
        expected = int(m.group(1)) if m else None
    except Exception:
        expected = None

    # Compile
    try:
        cp = subprocess.run(
            [str(COMPILE_DUMP), str(vuma_file), str(out_bin), backend],
            capture_output=True, text=True, timeout=30,
        )
        if not out_bin.exists() or out_bin.stat().st_size == 0:
            return {"file": rel, "backend": backend, "status": "compile_fail",
                    "exit": None, "expected": expected, "category": category_of(vuma_file)}
    except subprocess.TimeoutExpired:
        return {"file": rel, "backend": backend, "status": "compile_fail",
                "exit": None, "expected": expected, "category": category_of(vuma_file)}
    except Exception as e:
        return {"file": rel, "backend": backend, "status": "compile_fail",
                "exit": None, "expected": expected, "category": category_of(vuma_file),
                "error": str(e)[:120]}

    # Execute
    try:
        out_bin.chmod(0o755)
        cmd = [qemu, str(out_bin)] if qemu else [str(out_bin)]
        timeout_s = 3 if qemu else 2
        cp = subprocess.run(cmd, capture_output=True, timeout=timeout_s)
        code = cp.returncode
    except subprocess.TimeoutExpired:
        out_bin.unlink(missing_ok=True)
        return {"file": rel, "backend": backend, "status": "timeout",
                "exit": 124, "expected": expected, "category": category_of(vuma_file)}
    except Exception:
        out_bin.unlink(missing_ok=True)
        return {"file": rel, "backend": backend, "status": "exec_fail",
                "exit": -1, "expected": expected, "category": category_of(vuma_file)}
    finally:
        out_bin.unlink(missing_ok=True)

    # Classify
    # On Unix, negative returncode = killed by signal (-N); convert to 128+N
    if code < 0:
        sig = -code
        code = 128 + sig

    # Signals that indicate a crash
    crash_codes = {139, 134, 136, 131, 137, 133, 138, 140, 141}  # SIGSEGV, SIGABRT, SIGFPE, etc.

    if code in crash_codes or code >= 128:
        status = "crash"
    elif code == 124:
        status = "timeout"
    elif expected is not None:
        status = "strict_pass" if code == expected else "wrong_exit"
    else:
        status = "pass_any"

    return {"file": rel, "backend": backend, "status": status,
            "exit": code, "expected": expected, "category": category_of(vuma_file)}


def run_backend(backend, qemu, files, jobs=1):
    """Run all files for one backend sequentially with checkpoint files
    so partial results survive a crash."""
    print(f"[{time.strftime('%H:%M:%S')}] {backend}: starting {len(files)} files", flush=True)
    t0 = time.time()
    args_list = [(backend, str(f), qemu) for f in files]

    # Resume from checkpoint if present
    tsv_path = OUT_DIR / f"{backend}.tsv"
    done_set = set()
    if tsv_path.exists():
        with tsv_path.open() as f:
            next(f, None)  # skip header
            for line in f:
                parts = line.rstrip("\n").split("\t")
                if len(parts) >= 6:
                    done_set.add(parts[0])
        print(f"[{time.strftime('%H:%M:%S')}] {backend}: resuming — "
              f"{len(done_set)} already done", flush=True)

    # Open TSV in append mode
    mode = "a" if done_set else "w"
    f_tsv = tsv_path.open(mode)
    if mode == "w":
        f_tsv.write("file\tbackend\tstatus\texit\texpected\tcategory\n")
        f_tsv.flush()

    done = len(done_set)
    BATCH_FLUSH = 50
    since_flush = 0
    for a in args_list:
        rel = str(Path(a[1]).relative_to(GOLD_DIR))
        if rel in done_set:
            continue
        try:
            r = run_one(a)
        except Exception as e:
            r = {"file": rel, "backend": backend, "status": "exec_fail",
                 "exit": -1, "expected": None,
                 "category": category_of(Path(a[1])), "error": str(e)[:120]}
        f_tsv.write(f"{r['file']}\t{r['backend']}\t{r['status']}\t"
                    f"{r['exit'] if r['exit'] is not None else '-'}\t"
                    f"{r['expected'] if r['expected'] is not None else '-'}\t"
                    f"{r['category']}\n")
        since_flush += 1
        if since_flush >= BATCH_FLUSH:
            f_tsv.flush()
            since_flush = 0
        done += 1
        if done % 500 == 0:
            elapsed = time.time() - t0
            rate = (done - len(done_set)) / elapsed if elapsed > 0 else 0
            eta = (len(files) - done) / rate if rate > 0 else 0
            print(f"[{time.strftime('%H:%M:%S')}] {backend}: {done}/{len(files)} "
                  f"({rate:.1f}/s, ETA {eta:.0f}s)", flush=True)

    f_tsv.close()
    elapsed = time.time() - t0
    print(f"[{time.strftime('%H:%M:%S')}] {backend}: DONE in {elapsed:.1f}s "
          f"({len(files)/max(elapsed,1):.1f}/s)", flush=True)


def aggregate(results):
    """Compute per-backend and per-category stats."""
    stats = {}
    by_backend = {}
    by_backend_cat = {}
    for r in results:
        b = r["backend"]
        c = r["category"]
        by_backend.setdefault(b, []).append(r)
        by_backend_cat.setdefault((b, c), []).append(r)

    # Per-backend totals
    backend_stats = {}
    for b, rs in by_backend.items():
        total = len(rs)
        counts = {}
        for r in rs:
            counts[r["status"]] = counts.get(r["status"], 0) + 1
        backend_stats[b] = {
            "total": total,
            "counts": counts,
            "strict_pass_pct": round(100 * counts.get("strict_pass", 0) / total, 2) if total else 0,
            "pass_any_pct": round(100 * (counts.get("strict_pass", 0) + counts.get("pass_any", 0)) / total, 2) if total else 0,
        }

    # Per-(backend, category) breakdown
    cat_stats = {}
    for (b, c), rs in by_backend_cat.items():
        total = len(rs)
        counts = {}
        for r in rs:
            counts[r["status"]] = counts.get(r["status"], 0) + 1
        cat_stats.setdefault(c, {})[b] = {
            "total": total,
            "counts": counts,
            "strict_pass_pct": round(100 * counts.get("strict_pass", 0) / total, 2) if total else 0,
        }

    return {"backend_stats": backend_stats, "category_stats": cat_stats}


def main():
    if not COMPILE_DUMP.exists():
        print(f"ERROR: {COMPILE_DUMP} not found. Build it first:")
        print("  cargo build --release --bin compile_dump")
        sys.exit(1)

    # Optional --backend <name> to run only one backend (for parallel launches)
    only_backend = None
    if "--backend" in sys.argv:
        i = sys.argv.index("--backend")
        only_backend = sys.argv[i + 1]
    only_aggregate = "--aggregate-only" in sys.argv

    files = find_vuma_files()
    print(f"Found {len(files)} .vuma test programs")
    print(f"Output: {OUT_DIR}")
    if only_backend:
        print(f"Running only backend: {only_backend}")
    print()

    all_results = []
    if not only_aggregate:
        backends_to_run = {only_backend: BACKENDS[only_backend]} if only_backend else BACKENDS
        for backend, qemu in backends_to_run.items():
            run_backend(backend, qemu, files, jobs=1)

    # Load all results from TSVs
    for backend in BACKENDS:
        tsv_path = OUT_DIR / f"{backend}.tsv"
        if not tsv_path.exists():
            continue
        with tsv_path.open() as f:
            next(f, None)  # skip header
            for line in f:
                parts = line.rstrip("\n").split("\t")
                if len(parts) >= 6:
                    exit_str = parts[3]
                    exp_str = parts[4]
                    all_results.append({
                        "file": parts[0],
                        "backend": parts[1],
                        "status": parts[2],
                        "exit": int(exit_str) if exit_str.isdigit() or (
                            exit_str.startswith("-") and exit_str[1:].isdigit()
                        ) else None,
                        "expected": int(exp_str) if exp_str.isdigit() or (
                            exp_str.startswith("-") and exp_str[1:].isdigit()
                        ) else None,
                        "category": parts[5],
                    })
        print(f"  -> loaded {tsv_path}")

    # Aggregate stats
    stats = aggregate(all_results)

    # JSON summary
    with (OUT_DIR / "summary.json").open("w") as f:
        json.dump(stats, f, indent=2)

    # Human-readable summary
    lines = []
    lines.append("=" * 78)
    lines.append("VUMA Gold Standard Test Results — Full Sweep")
    lines.append(f"Date: {time.strftime('%Y-%m-%d %H:%M:%S %Z')}")
    lines.append(f"Total programs: {len(files)}")
    lines.append(f"Backends tested: {len(BACKENDS)} (Wasm32 excluded — no native exec)")
    lines.append("=" * 78)
    lines.append("")
    lines.append("STATUS DEFINITIONS:")
    lines.append("  strict_pass  - exit code matches file's 'Expected exit code' header")
    lines.append("  pass_any     - ran cleanly, no expected code to compare")
    lines.append("  wrong_exit   - ran cleanly but exit code != expected")
    lines.append("  crash        - killed by signal (SIGSEGV=139, SIGABRT=134, etc.)")
    lines.append("  timeout      - 3s (native) / 5s (QEMU) limit exceeded")
    lines.append("  compile_fail - compile_dump could not produce a binary")
    lines.append("  exec_fail    - binary produced but could not be executed")
    lines.append("")
    lines.append("-" * 78)
    lines.append("PER-BACKEND SUMMARY")
    lines.append("-" * 78)
    hdr = f"{'Backend':<14}{'Total':>7}{'Strct':>8}{'Any':>7}{'Wrong':>7}{'Crash':>7}{'Tmout':>7}{'CmpFl':>7}{'ExFl':>6}{'Strict%':>10}{'Any%':>8}"
    lines.append(hdr)
    lines.append("-" * len(hdr))
    for b in BACKENDS:
        s = stats["backend_stats"].get(b, {})
        c = s.get("counts", {})
        total = s.get("total", 0)
        lines.append(
            f"{b:<14}{total:>7}"
            f"{c.get('strict_pass', 0):>8}"
            f"{c.get('pass_any', 0):>7}"
            f"{c.get('wrong_exit', 0):>7}"
            f"{c.get('crash', 0):>7}"
            f"{c.get('timeout', 0):>7}"
            f"{c.get('compile_fail', 0):>7}"
            f"{c.get('exec_fail', 0):>6}"
            f"{s.get('strict_pass_pct', 0):>9.2f}%"
            f"{s.get('pass_any_pct', 0):>7.2f}%"
        )
    lines.append("")
    lines.append("-" * 78)
    lines.append("PER-CATEGORY × PER-BACKEND (strict pass rate %)")
    lines.append("-" * 78)
    categories = sorted(stats["category_stats"].keys())
    cat_hdr = f"{'Category':<20}" + "".join(f"{b[:8]:>10}" for b in BACKENDS)
    lines.append(cat_hdr)
    lines.append("-" * len(cat_hdr))
    for cat in categories:
        row = f"{cat:<20}"
        for b in BACKENDS:
            v = stats["category_stats"].get(cat, {}).get(b, {})
            pct = v.get("strict_pass_pct", 0)
            total = v.get("total", 0)
            row += f"{pct:>9.1f}%"
        lines.append(row)
    lines.append("")

    txt = "\n".join(lines) + "\n"
    (OUT_DIR / "summary.txt").write_text(txt)
    print()
    print(txt)


if __name__ == "__main__":
    main()
