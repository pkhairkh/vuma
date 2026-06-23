#!/usr/bin/env python3
"""
Chunked sweep: run ONE backend, ONE batch at a time. Writes results to TSV
after each batch (survives crashes). Designed to be invoked many times by
a supervisor script.

Usage: python3 run_one_batch.py <backend> <start_idx> <count>
"""

import os
import re
import subprocess
import sys
import time
from pathlib import Path

REPO_ROOT = Path("/home/z/my-project/vuma")
COMPILE_DUMP = REPO_ROOT / "target/release/compile_dump"
GOLD_DIR = REPO_ROOT / "tests/gold_standard"
QEMU_DIR = Path("/tmp/qemu/extracted/usr/bin")
OUT_DIR = Path("/home/z/my-project/download/gold_standard_results")
OUT_DIR.mkdir(parents=True, exist_ok=True)

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


def find_vuma_files():
    return sorted(GOLD_DIR.rglob("*.vuma"))


def category_of(path: Path) -> str:
    try:
        return path.relative_to(GOLD_DIR).parts[0]
    except Exception:
        return "_unknown"


def run_one(backend, vuma_file, qemu):
    rel = str(vuma_file.relative_to(GOLD_DIR))
    out_bin = Path(f"/tmp/vuma_chunk_{os.getpid()}.bin")

    try:
        text = vuma_file.read_text(errors="replace")
        m = EXPECTED_RE.search(text)
        expected = int(m.group(1)) if m else None
    except Exception:
        expected = None

    # Compile
    try:
        subprocess.run(
            [str(COMPILE_DUMP), str(vuma_file), str(out_bin), backend],
            capture_output=True, text=True, timeout=30,
        )
        if not out_bin.exists() or out_bin.stat().st_size == 0:
            out_bin.unlink(missing_ok=True)
            return {"file": rel, "backend": backend, "status": "compile_fail",
                    "exit": "-", "expected": expected if expected is not None else "-",
                    "category": category_of(vuma_file)}
    except Exception:
        out_bin.unlink(missing_ok=True)
        return {"file": rel, "backend": backend, "status": "compile_fail",
                "exit": "-", "expected": expected if expected is not None else "-",
                "category": category_of(vuma_file)}

    # Execute
    try:
        out_bin.chmod(0o755)
        cmd = [qemu, str(out_bin)] if qemu else [str(out_bin)]
        # 1s timeout — VUMA binaries either complete in <100ms or hang forever
        # (infinite loop from known codegen bugs). 3s/5s wastes time on hangs.
        timeout_s = 1
        cp = subprocess.run(cmd, capture_output=True, timeout=timeout_s)
        code = cp.returncode
    except subprocess.TimeoutExpired:
        out_bin.unlink(missing_ok=True)
        return {"file": rel, "backend": backend, "status": "timeout",
                "exit": 124, "expected": expected if expected is not None else "-",
                "category": category_of(vuma_file)}
    except Exception:
        out_bin.unlink(missing_ok=True)
        return {"file": rel, "backend": backend, "status": "exec_fail",
                "exit": -1, "expected": expected if expected is not None else "-",
                "category": category_of(vuma_file)}
    finally:
        out_bin.unlink(missing_ok=True)

    if code < 0:
        code = 128 + (-code)
    crash_codes = {139, 134, 136, 131, 137, 133, 138, 140, 141}
    if code in crash_codes or code >= 128:
        status = "crash"
    elif code == 124:
        status = "timeout"
    elif expected is not None:
        status = "strict_pass" if code == expected else "wrong_exit"
    else:
        status = "pass_any"

    return {"file": rel, "backend": backend, "status": status,
            "exit": code, "expected": expected if expected is not None else "-",
            "category": category_of(vuma_file)}


def main():
    if len(sys.argv) < 4:
        print("Usage: run_one_batch.py <backend> <start> <count>")
        sys.exit(2)
    backend = sys.argv[1]
    start = int(sys.argv[2])
    count = int(sys.argv[3])
    qemu = BACKENDS[backend]

    files = find_vuma_files()
    total = len(files)
    end = min(start + count, total)
    batch = files[start:end]

    tsv_path = OUT_DIR / f"{backend}.tsv"
    is_new = not tsv_path.exists()
    f_tsv = tsv_path.open("a")
    if is_new:
        f_tsv.write("file\tbackend\tstatus\texit\texpected\tcategory\n")

    t0 = time.time()
    for i, vuma_file in enumerate(batch):
        try:
            r = run_one(backend, vuma_file, qemu)
        except Exception as e:
            r = {"file": str(vuma_file.relative_to(GOLD_DIR)),
                 "backend": backend, "status": "exec_fail",
                 "exit": -1, "expected": "-",
                 "category": category_of(vuma_file)}
        f_tsv.write(f"{r['file']}\t{r['backend']}\t{r['status']}\t"
                    f"{r['exit']}\t{r['expected']}\t{r['category']}\n")
        if (i + 1) % 50 == 0:
            f_tsv.flush()
    f_tsv.flush()
    f_tsv.close()

    elapsed = time.time() - t0
    print(f"{backend}: {start}-{end}/{total} in {elapsed:.1f}s "
          f"({len(batch)/max(elapsed,1):.1f}/s)", flush=True)


if __name__ == "__main__":
    main()
