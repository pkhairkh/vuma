#!/usr/bin/env python3
"""Run all 5754 gold standard programs across all 8 backends (including Wasm32).
Writes results to per-backend TSV files with checkpointing."""
import os, re, subprocess, sys, time
from pathlib import Path

REPO_ROOT = Path("/home/z/my-project/vuma")
COMPILE_DUMP = str(REPO_ROOT / "target/release/compile_dump")
GOLD_DIR = REPO_ROOT / "tests/gold_standard"
QEMU_DIR = Path("/tmp/qemu/extracted/usr/bin")
OUT_DIR = Path("/home/z/my-project/download/gold_standard_results")
RUN_WASM = "/tmp/run_wasm.js"
NODE = "/usr/bin/node"

BACKENDS = {
    "x86_64": {"qemu": "", "timeout": 2},
    "aarch64": {"qemu": str(QEMU_DIR / "qemu-aarch64"), "timeout": 1},
    "riscv64": {"qemu": str(QEMU_DIR / "qemu-riscv64"), "timeout": 1},
    "arm32": {"qemu": str(QEMU_DIR / "qemu-arm"), "timeout": 1},
    "mips64": {"qemu": str(QEMU_DIR / "qemu-mips64el"), "timeout": 1},
    "ppc64": {"qemu": str(QEMU_DIR / "qemu-ppc64"), "timeout": 1},
    "loongarch64": {"qemu": str(QEMU_DIR / "qemu-loongarch64"), "timeout": 0.5},
    "wasm32": {"qemu": None, "timeout": 3, "wasm": True},  # uses node
}

EXPECTED_RE = re.compile(r"^//\s*[Ee]xpected\s+exit\s+code\s*:\s*(-?\d+)", re.MULTILINE)

def find_files():
    return sorted(GOLD_DIR.rglob("*.vuma"))

def cat_of(p):
    try: return p.relative_to(GOLD_DIR).parts[0]
    except: return "_unknown"

def count_done(backend):
    p = OUT_DIR / f"{backend}.tsv"
    if not p.exists(): return 0
    with p.open() as f:
        return sum(1 for _ in f) - 1

def run_one(backend, vuma_file, config):
    rel = str(vuma_file.relative_to(GOLD_DIR))
    out_bin = Path(f"/tmp/vuma8_{os.getpid()}.bin")
    try:
        text = vuma_file.read_text(errors="replace")
        m = EXPECTED_RE.search(text)
        expected = int(m.group(1)) if m else None
    except: expected = None

    # Compile
    try:
        subprocess.run([COMPILE_DUMP, str(vuma_file), str(out_bin), backend],
                       capture_output=True, timeout=10)
        if not out_bin.exists() or out_bin.stat().st_size == 0:
            out_bin.unlink(missing_ok=True)
            return (rel, backend, "compile_fail", "-", expected if expected is not None else "-", cat_of(vuma_file))
    except:
        out_bin.unlink(missing_ok=True)
        return (rel, backend, "compile_fail", "-", expected if expected is not None else "-", cat_of(vuma_file))

    # Execute
    timeout_s = config["timeout"]
    try:
        if config.get("wasm"):
            # Wasm32: run with node
            cp = subprocess.run([NODE, RUN_WASM, str(out_bin)],
                              capture_output=True, timeout=timeout_s)
            code = cp.returncode
        else:
            out_bin.chmod(0o755)
            qemu = config["qemu"]
            cmd = [qemu, str(out_bin)] if qemu else [str(out_bin)]
            cp = subprocess.run(cmd, capture_output=True, timeout=timeout_s)
            code = cp.returncode
    except subprocess.TimeoutExpired:
        out_bin.unlink(missing_ok=True)
        return (rel, backend, "timeout", 124, expected if expected is not None else "-", cat_of(vuma_file))
    except:
        out_bin.unlink(missing_ok=True)
        return (rel, backend, "exec_fail", -1, expected if expected is not None else "-", cat_of(vuma_file))
    finally:
        out_bin.unlink(missing_ok=True)

    if code < 0: code = 128 + (-code)
    if expected is not None and code == expected: status = "strict_pass"
    elif code == 124: status = "timeout"
    elif code >= 128: status = "crash"
    elif expected is not None: status = "wrong_exit"
    else: status = "pass_any"
    return (rel, backend, status, code, expected if expected is not None else "-", cat_of(vuma_file))

def main():
    backend = sys.argv[1] if len(sys.argv) > 1 else "x86_64"
    config = BACKENDS[backend]
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
        if i < done: continue
        try:
            r = run_one(backend, vuma_file, config)
        except Exception as e:
            r = (str(vuma_file.relative_to(GOLD_DIR)), backend, "exec_fail", -1, "-", cat_of(vuma_file))
        f_tsv.write("\t".join(str(x) for x in r) + "\n")
        count += 1
        if count % 50 == 0: f_tsv.flush()
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
