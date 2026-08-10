#!/usr/bin/env python3
"""
Full differential validation runner.
Compiles each VUMA harness on each backend, runs it, compares to C reference vectors.
Generates a matrix report + JSON summary.
"""
import json, os, subprocess, sys, time
from pathlib import Path

REPO = "/home/z/my-project/vuma"
COMPILE_DUMP = f"{REPO}/target/debug/compile_dump"
# Use release build if available (much faster)
if os.path.exists(f"{REPO}/target/release/compile_dump"):
    COMPILE_DUMP = f"{REPO}/target/release/compile_dump"

HARNESS_DIR = f"{REPO}/tests/full_validation"
VECTORS_DIR = f"{REPO}/test_results/vectors"
OUTDIR = "/tmp/vuma_full_val"
os.makedirs(OUTDIR, exist_ok=True)

BACKENDS = [
    "x86_64", "x86_32", "aarch64", "aarch64_be", "arm32", "armeb",
    "riscv64", "riscv32", "mips64", "mips64be", "ppc64", "ppc64le",
    "loongarch64", "s390x", "sparc64", "alpha", "hppa", "m68k", "wasm32",
]

QEMU_MAP = {
    "x86_64": None,
    "x86_32": "qemu-i386-static",
    "aarch64": "qemu-aarch64-static",
    "aarch64_be": "qemu-aarch64_be-static",
    "arm32": "qemu-arm-static",
    "armeb": "qemu-armeb-static",
    "riscv64": "qemu-riscv64-static",
    "riscv32": "qemu-riscv32-static",
    "mips64": "qemu-mips64el-static",
    "mips64be": "qemu-mips64-static",
    "ppc64": "qemu-ppc64-static",
    "ppc64le": "qemu-ppc64le-static",
    "loongarch64": "qemu-loongarch64-static",
    "s390x": "qemu-s390x-static",
    "sparc64": "qemu-sparc64-static",
    "alpha": "qemu-alpha-static",
    "hppa": "qemu-hppa-static",
    "m68k": "qemu-m68k-static",
    "wasm32": "wasm32",
}

WASM32_RUNNER = f"{REPO}/scripts/wasm32_runner.py"

def parse_output(raw_bytes, num_vectors, output_len_per_vec):
    """Parse print_int output into per-vector hex strings.
    Each byte is print_int(byte+1000) → 4-digit decimal.
    Delimiter: print_int(999).
    Returns list of hex strings (one per vector)."""
    text = raw_bytes.decode("utf-8", "replace")
    # Split by whitespace/newlines to get individual numbers
    tokens = text.split()
    results = []
    current_hex = ""
    for tok in tokens:
        try:
            val = int(tok)
        except ValueError:
            continue
        if val == 999:
            # Vector delimiter
            results.append(current_hex)
            current_hex = ""
        elif 1000 <= val <= 1255:
            current_hex += "%02x" % ((val - 1000) & 0xFF)
    # Last vector (if no trailing delimiter)
    if current_hex:
        results.append(current_hex)
    return results

def load_vectors(module_name):
    """Load expected vectors for a module."""
    vec_path = f"{VECTORS_DIR}/{module_name}.json"
    if not os.path.exists(vec_path):
        return None
    with open(vec_path) as f:
        data = json.load(f)
    return data

def run_harness(module_name, backend, num_vectors=20):
    """Compile and run a harness, return (status, results_list, detail)."""
    harness = f"{HARNESS_DIR}/test_{module_name}_20vec.vuma"
    if not os.path.exists(harness):
        return ("NO_HARNESS", [], "harness file missing")

    vec_data = load_vectors(module_name)
    if not vec_data:
        return ("NO_VECTORS", [], "vectors file missing")

    vectors = vec_data["vectors"]
    output_len = len(vectors[0]["expected_hex"]) // 2 if vectors else 0

    bin_path = f"{OUTDIR}/{module_name}_{backend}.bin"

    # Compile
    try:
        r = subprocess.run(
            [COMPILE_DUMP, harness, bin_path, backend, "--no-verify"],
            capture_output=True, text=True, timeout=120,
        )
        if r.returncode != 0 or not os.path.exists(bin_path):
            err = (r.stderr or r.stdout or "")[:200].replace("\n", " ")
            return ("COMPILE_ERROR", [], err)
    except subprocess.TimeoutExpired:
        return ("COMPILE_TIMEOUT", [], "compile >120s")
    except Exception as e:
        return ("COMPILE_EXC", [], str(e)[:200])

    # Run
    q = QEMU_MAP[backend]
    try:
        if backend == "wasm32":
            r = subprocess.run(
                ["python3", WASM32_RUNNER, bin_path],
                capture_output=True, timeout=60,
            )
        elif q is None:
            r = subprocess.run([bin_path], capture_output=True, timeout=60)
        else:
            r = subprocess.run([q, bin_path], capture_output=True, timeout=60)
    except subprocess.TimeoutExpired:
        return ("RUN_TIMEOUT", [], "run >60s")
    except Exception as e:
        return ("RUN_EXC", [], str(e)[:200])

    # Parse output
    actual_results = parse_output(r.stdout, len(vectors), output_len)

    # Compare each vector
    pass_count = 0
    failures = []
    for vi, vec in enumerate(vectors):
        expected = vec["expected_hex"]
        actual = actual_results[vi] if vi < len(actual_results) else ""
        if actual == expected:
            pass_count += 1
        else:
            failures.append({
                "vector": vi,
                "expected": expected[:32],
                "actual": actual[:32],
            })

    status = "PASS" if pass_count == len(vectors) else "PARTIAL"
    return (status, {"pass": pass_count, "total": len(vectors), "failures": failures[:3]}, "")

def main():
    only_modules = set(sys.argv[1:]) if len(sys.argv) > 1 and not sys.argv[1].startswith("-") else None
    only_backends = None
    if "--backends" in sys.argv:
        idx = sys.argv.index("--backends")
        only_backends = set(sys.argv[idx+1:])

    # Discover available harnesses
    harnesses = []
    for f in sorted(os.listdir(HARNESS_DIR)):
        if f.startswith("test_") and f.endswith("_20vec.vuma"):
            mod = f.replace("test_", "").replace("_20vec.vuma", "")
            if only_modules and mod not in only_modules:
                continue
            harnesses.append(mod)

    backends = [b for b in BACKENDS if not only_backends or b in only_backends]

    print(f"Modules: {len(harnesses)} | Backends: {len(backends)}")
    print(f"Total runs: {len(harnesses) * len(backends)}")
    print(f"Compile dump: {COMPILE_DUMP}")
    print()

    t0 = time.time()
    results = {}
    total_pass = 0
    total_runs = 0

    for mi, module in enumerate(harnesses):
        for bi, backend in enumerate(backends):
            status, detail, err = run_harness(module, backend)
            results[(module, backend)] = (status, detail, err)
            total_runs += 1
            if status == "PASS":
                total_pass += 1
                mark = "✓"
            elif status == "PARTIAL":
                p = detail.get("pass", 0)
                t = detail.get("total", 0)
                total_pass += p / t if t > 0 else 0
                mark = f"{p}/{t}"
            else:
                mark = status[:8]
            if bi == 0:
                print(f"[{mi+1}/{len(harnesses)}] {module:<25} {backend:<12} {mark}")
            elif status != "PASS":
                print(f"         {'':25} {backend:<12} {mark} {err[:40]}")

    elapsed = time.time() - t0

    # Generate matrix
    print("\n" + "=" * 120)
    print("VALIDATION MATRIX")
    print("=" * 120)
    header = f"{'MODULE':<28}" + "".join(f"{b[:7]:<8}" for b in backends)
    print(header)
    print("-" * 120)

    per_backend_pass = {b: 0.0 for b in backends}
    per_backend_total = {b: 0 for b in backends}
    per_module_pass = {}

    for module in harnesses:
        row = f"{module:<28}"
        mp = 0.0
        mt = 0
        for backend in backends:
            status, detail, err = results.get((module, backend), ("MISSING",))
            mt += 1
            per_backend_total[backend] += 1
            if status == "PASS":
                ch = "✓"
                mp += 1
                per_backend_pass[backend] += 1
            elif status == "PARTIAL":
                p = detail.get("pass", 0)
                t = detail.get("total", 0)
                frac = p / t if t > 0 else 0
                ch = f"{p}/{t}"
                mp += frac
                per_backend_pass[backend] += frac
            elif status == "COMPILE_ERROR":
                ch = "CERR"
            elif status == "RUN_TIMEOUT":
                ch = "TOUT"
            elif status == "NO_HARNESS":
                ch = "—"
            else:
                ch = "F"
            row += f"{ch:<8}"
        per_module_pass[module] = (mp, mt)
        print(row)

    print("\n" + "-" * 120)
    print(f"TOTAL: {total_pass:.1f}/{total_runs} ({100*total_pass/max(total_runs,1):.1f}%) in {elapsed:.0f}s")

    print("\nPer-backend:")
    for b in backends:
        p = per_backend_pass[b]
        t = per_backend_total[b]
        print(f"  {b:<14} {p:.1f}/{t} ({100*p/max(t,1):.0f}%)")

    print("\nPer-module:")
    for module in harnesses:
        mp, mt = per_module_pass[module]
        print(f"  {module:<26} {mp:.1f}/{mt} ({100*mp/max(mt,1):.0f}%)")

    # Save JSON summary
    summary = {
        "timestamp": time.strftime("%Y-%m-%d %H:%M:%S UTC", time.gmtime()),
        "total_runs": total_runs,
        "total_pass": round(total_pass, 1),
        "pass_rate": f"{100*total_pass/max(total_runs,1):.2f}%",
        "elapsed_sec": round(elapsed, 1),
        "modules_tested": len(harnesses),
        "backends_tested": len(backends),
        "per_backend": {b: {"pass": round(per_backend_pass[b],1), "total": per_backend_total[b]}
                        for b in backends},
        "per_module": {m: {"pass": round(per_module_pass[m][0],1), "total": per_module_pass[m][1]}
                       for m in harnesses},
        "results": {f"{m}|{b}": {"status": results[(m,b)][0],
                                  "detail": results[(m,b)][1] if isinstance(results[(m,b)][1], dict) else str(results[(m,b)][1]),
                                  "error": results[(m,b)][2]}
                    for (m, b) in results},
    }
    out_json = f"{REPO}/test_results/full_validation.json"
    with open(out_json, "w") as f:
        json.dump(summary, f, indent=2)
    print(f"\nJSON summary: {out_json}")
    return 0

if __name__ == "__main__":
    sys.exit(main())
