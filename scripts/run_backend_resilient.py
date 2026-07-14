#!/usr/bin/env python3
"""Run all 5754 files for one QEMU backend with checkpointing.
Appends results to <backend>.tsv. Resumes from where it left off.
Uses 1s timeout per binary. Fixed crash detection (expected checked first).

Usage: python3 run_backend_resilient.py <backend>
"""
import os, re, subprocess, sys, time, signal
from pathlib import Path

REPO_ROOT = Path("/home/z/my-project/vuma")
COMPILE_DUMP = str(REPO_ROOT / "target/release/compile_dump")
GOLD_DIR = REPO_ROOT / "tests/gold_standard"
QEMU_DIR = Path("/tmp/qemu/extracted/usr/bin")
OUT_DIR = Path("/home/z/my-project/download/gold_standard_results")

BACKENDS = {
    "x86_64": "",
    "aarch64": str(QEMU_DIR / "qemu-aarch64"),
    "riscv64": str(QEMU_DIR / "qemu-riscv64"),
    "arm32": str(QEMU_DIR / "qemu-arm"),
    "mips64": str(QEMU_DIR / "qemu-mips64el"),
    "ppc64": str(QEMU_DIR / "qemu-ppc64"),
    "loongarch64": str(QEMU_DIR / "qemu-loongarch64"),
}

EXPECTED_RE = re.compile(r"^//\s*[Ee]xpected\s+exit\s+code\s*:\s*(-?\d+)", re.MULTILINE)


def find_files():
    return sorted(GOLD_DIR.rglob("*.vuma"))


def cat_of(p):
    try:
        return p.relative_to(GOLD_DIR).parts[0]
    except Exception:
        return "_unknown"


def count_done(backend):
    p = OUT_DIR / f"{backend}.tsv"
    if not p.exists():
        return 0
    with p.open() as f:
        return sum(1 for _ in f) - 1  # minus header


def run_one(backend, vuma_file, qemu):
    rel = str(vuma_file.relative_to(GOLD_DIR))
    out_bin = Path(f"/tmp/vuma_r_{os.getpid()}.bin")
    try:
        text = vuma_file.read_text(errors="replace")
        m = EXPECTED_RE.search(text)
        expected = int(m.group(1)) if m else None
    except Exception:
        expected = None
    # Compile
    try:
        subprocess.run(
            [COMPILE_DUMP, str(vuma_file), str(out_bin), backend],
            capture_output=True, timeout=10,
        )
        if not out_bin.exists() or out_bin.stat().st_size == 0:
            out_bin.unlink(missing_ok=True)
            return (rel, backend, "compile_fail", "-",
                    expected if expected is not None else "-", cat_of(vuma_file))
    except Exception:
        out_bin.unlink(missing_ok=True)
        return (rel, backend, "compile_fail", "-",
                expected if expected is not None else "-", cat_of(vuma_file))
    # Execute
    try:
        out_bin.chmod(0o755)
        cp = subprocess.run([qemu, str(out_bin)], capture_output=True, timeout=1)
        code = cp.returncode
    except subprocess.TimeoutExpired:
        out_bin.unlink(missing_ok=True)
        return (rel, backend, "timeout", 124,
                expected if expected is not None else "-", cat_of(vuma_file))
    except Exception:
        out_bin.unlink(missing_ok=True)
        return (rel, backend, "exec_fail", -1,
                expected if expected is not None else "-", cat_of(vuma_file))
    finally:
        out_bin.unlink(missing_ok=True)

    if code < 0:
        code = 128 + (-code)
    # Fixed: check expected FIRST
    if expected is not None and code == expected:
        status = "strict_pass"
    elif code == 124:
        status = "timeout"
    elif code >= 128:
        status = "crash"
    elif expected is not None:
        status = "wrong_exit"
    else:
        status = "pass_any"
    return (rel, backend, status, code,
            expected if expected is not None else "-", cat_of(vuma_file))


def main():
    backend = sys.argv[1]
    qemu = BACKENDS[backend]
    files = find_files()
    total = len(files)

    done = count_done(backend)
    if done >= total:
        print(f"[{time.strftime('%H:%M:%S')}] {backend}: ALREADY DONE ({done}/{total})")
        return

    tsv_path = OUT_DIR / f"{backend}.tsv"
    is_new = not tsv_path.exists()
    f_tsv = tsv_path.open("a")
    if is_new:
        f_tsv.write("file\tbackend\tstatus\texit\texpected\tcategory\n")
        f_tsv.flush()

    print(f"[{time.strftime('%H:%M:%S')}] {backend}: resuming from {done}/{total}", flush=True)
    t0 = time.time()
    count = 0
    for i, vuma_file in enumerate(files):
        if i < done:
            continue
        try:
            r = run_one(backend, vuma_file, qemu)
        except Exception as e:
            r = (str(vuma_file.relative_to(GOLD_DIR)), backend, "exec_fail", -1, "-",
                 cat_of(vuma_file))
        f_tsv.write("\t".join(str(x) for x in r) + "\n")
        count += 1
        if count % 50 == 0:
            f_tsv.flush()
        if count % 500 == 0:
            elapsed = time.time() - t0
            rate = count / elapsed if elapsed > 0 else 0
            eta = (total - i - 1) / rate if rate > 0 else 0
            print(f"[{time.strftime('%H:%M:%S')}] {backend}: {i+1}/{total} "
                  f"({rate:.1f}/s, ETA {eta:.0f}s)", flush=True)
    f_tsv.flush()
    f_tsv.close()
    print(f"[{time.strftime('%H:%M:%S')}] {backend}: COMPLETE ({time.time()-t0:.1f}s)", flush=True)


if __name__ == "__main__":
    main()
