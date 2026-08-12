#!/usr/bin/env python3
"""Validate compact harnesses — runs all batches for a module on a backend.

Resumable: saves after each harness, skips harnesses that already passed.
"""
import json, os, subprocess, sys, time, glob, re

REPO = "/home/z/my-project/vuma"
COMPILE_DUMP = f"{REPO}/target/release/compile_dump"
HARNESS_DIR = f"{REPO}/tests/compact_harnesses"
VECTORS_DIR = f"{REPO}/test_results/standard_vectors"
OUTDIR = "/tmp/vuma_compact_val"
RESULTS_FILE = f"{REPO}/test_results/compact_results.json"
DETAIL_FILE = f"{REPO}/test_results/compact_results_detail.json"
os.makedirs(OUTDIR, exist_ok=True)

# Module-specific run timeout override (seconds). ECC sign operations are
# SLOW in VUMA-compiled code (>60-120s per vector) but the code is correct.
RUN_TIMEOUT_OVERRIDE = {
    "ecdsa_p256": 600,
    "ecdsa_p384": 600,
    "secp256k1": 600,
    "ml_kem": 120,
    "ml_dsa": 120,
    "falcon": 120,
    "hqc": 120,
    "slh_dsa": 600,
    "argon2": 120,
    "scrypt": 120,
}
DEFAULT_RUN_TIMEOUT = 30

def get_run_timeout(module):
    return RUN_TIMEOUT_OVERRIDE.get(module, DEFAULT_RUN_TIMEOUT)

BACKENDS = ["x86_64","x86_32","aarch64","aarch64_be","arm32","armeb","riscv64","riscv32",
            "mips64","mips64be","ppc64","ppc64le","loongarch64","s390x","sparc64","alpha",
            "hppa","m68k","wasm32"]

QEMU = {"x86_64":None,"x86_32":"qemu-i386-static","aarch64":"qemu-aarch64-static",
        "aarch64_be":"qemu-aarch64_be-static","arm32":"qemu-arm-static","armeb":"qemu-armeb-static",
        "riscv64":"qemu-riscv64-static","riscv32":"qemu-riscv32-static","mips64":"qemu-mips64el-static",
        "mips64be":"qemu-mips64-static","ppc64":"qemu-ppc64-static","ppc64le":"qemu-ppc64le-static",
        "loongarch64":"qemu-loongarch64-static","s390x":"qemu-s390x-static","sparc64":"qemu-sparc64-static",
        "alpha":"qemu-alpha-static","hppa":"qemu-hppa-static","m68k":"qemu-m68k-static","wasm32":"wasm32"}

def _batch_vph(harness):
    """Count print_int(999) delimiters in a harness file."""
    with open(harness) as f:
        return f.read().count("print_int(999);")

def parse_output(raw):
    text = raw.decode("utf-8","replace")
    text = "".join(text.split())
    is_hex = False
    if len(text) >= 8 and text[:4] == "0000":
        try:
            val = int(text[:8], 16)
            if 1000 <= val <= 1255: is_hex = True
        except: pass
    results = []
    cur = ""
    i = 0
    if is_hex:
        delim = "%08x" % 999
        while i + 8 <= len(text):
            chunk = text[i:i+8]
            if chunk == delim: results.append(cur); cur = ""; i += 8; continue
            try:
                val = int(chunk, 16)
                if 1000 <= val <= 1255: cur += "%02x" % (val-1000); i += 8; continue
            except: pass
            i += 1
    else:
        while i < len(text):
            if i + 3 <= len(text) and text[i:i+3] == "999":
                results.append(cur); cur = ""; i += 3; continue
            if i + 4 <= len(text):
                chunk = text[i:i+4]
                try:
                    v = int(chunk)
                    if 1000 <= v <= 1255: cur += "%02x" % (v-1000); i += 4; continue
                except: pass
            i += 1
    if cur: results.append(cur)
    return results

def load_results():
    if os.path.exists(RESULTS_FILE):
        with open(RESULTS_FILE) as f: return json.load(f)
    return {}

def save_results(data):
    with open(RESULTS_FILE, "w") as f: json.dump(data, f, indent=2)

def load_detail():
    if os.path.exists(DETAIL_FILE):
        with open(DETAIL_FILE) as f: return json.load(f)
    return {}

def save_detail(data):
    with open(DETAIL_FILE, "w") as f: json.dump(data, f, indent=2)

def batch_sort_key(path):
    m = re.search(r'_b(\d+)\.vuma$', path)
    return int(m.group(1)) if m else 0

def run_one_harness(module, backend, hi, harness, all_vectors, vph, one_vec_per_batch):
    """Run one harness, return (pass_count, total_count, status_flag)."""
    bin_path = f"{OUTDIR}/{module}_b{hi}_{backend}.bin"
    detail_key = f"{module}|{backend}|b{hi}"

    detail = load_detail()
    # If already done, return cached result
    if detail_key in detail:
        d = detail[detail_key]
        return d["pass"], d["total"], d.get("status_flag", "DONE")

    batch_vph = _batch_vph(harness)
    # Compile (skip if binary already exists and is newer than harness)
    need_compile = True
    if os.path.exists(bin_path) and os.path.exists(harness):
        if os.path.getmtime(bin_path) > os.path.getmtime(harness):
            need_compile = False

    if need_compile:
        try:
            r = subprocess.run([COMPILE_DUMP, harness, bin_path, backend, "--no-verify"],
                             capture_output=True, text=True, timeout=300)
            if r.returncode != 0 or not os.path.exists(bin_path):
                total = 1 if one_vec_per_batch else batch_vph
                detail[detail_key] = {"pass": 0, "total": total, "status_flag": "CERR"}
                save_detail(detail)
                return 0, total, "CERR"
        except Exception as e:
            total = 1 if one_vec_per_batch else batch_vph
            detail[detail_key] = {"pass": 0, "total": total, "status_flag": "EXC"}
            save_detail(detail)
            return 0, total, "EXC"

    # Run
    q = QEMU[backend]
    run_to = get_run_timeout(module)
    try:
        if backend == "wasm32":
            r = subprocess.run(["python3",f"{REPO}/scripts/wasm32_runner.py",bin_path],capture_output=True,timeout=run_to)
        elif q is None:
            r = subprocess.run([bin_path],capture_output=True,timeout=run_to)
        else:
            r = subprocess.run([q,bin_path],capture_output=True,timeout=run_to)
    except:
        total = 1 if one_vec_per_batch else batch_vph
        detail[detail_key] = {"pass": 0, "total": total, "status_flag": "TOUT"}
        save_detail(detail)
        return 0, total, "TOUT"

    actual = parse_output(r.stdout)
    pass_count = 0
    total_count = 0
    status_flag = "PASS"
    if one_vec_per_batch:
        total_count = 1
        act_concat = "".join(actual)
        exp = all_vectors[hi]["expected_hex"] if hi < len(all_vectors) else ""
        if act_concat == exp:
            pass_count = 1
        else:
            status_flag = "PARTIAL"
    else:
        for vi in range(batch_vph):
            vec_idx = hi * vph + vi
            if vec_idx >= len(all_vectors): break
            total_count += 1
            exp = all_vectors[vec_idx]["expected_hex"]
            act = actual[vi] if vi < len(actual) else ""
            if act == exp:
                pass_count += 1
            else:
                status_flag = "PARTIAL"

    detail[detail_key] = {"pass": pass_count, "total": total_count, "status_flag": status_flag}
    save_detail(detail)
    return pass_count, total_count, status_flag

def run_module(module, backends):
    vec_path = f"{VECTORS_DIR}/{module}.json"
    if not os.path.exists(vec_path): print(f"  {module}: NO VECTORS"); return
    with open(vec_path) as f: vec_data = json.load(f)
    all_vectors = vec_data["vectors"]

    harnesses = sorted(glob.glob(f"{HARNESS_DIR}/test_{module}_b*.vuma"), key=batch_sort_key)
    if not harnesses: print(f"  {module}: NO HARNESS"); return

    with open(harnesses[0]) as f:
        content = f.read()
    vph = content.count("print_int(999);")
    if vph == 0: vph = 5
    one_vec_per_batch = (len(harnesses) == len(all_vectors))

    results = load_results()
    t0 = time.time()

    for bi, backend in enumerate(backends):
        total_pass = 0
        total_vecs = 0
        status = "PASS"
        for hi, harness in enumerate(harnesses):
            p, t, flag = run_one_harness(module, backend, hi, harness, all_vectors, vph, one_vec_per_batch)
            total_pass += p
            total_vecs += t
            if flag == "CERR" and status == "PASS": status = "CERR"
            elif flag == "EXC" and status == "PASS": status = "EXC"
            elif flag == "TOUT" and status == "PASS": status = "TOUT"
            elif flag == "PARTIAL" and status == "PASS": status = "PARTIAL"
            elif flag == "PARTIAL": pass
            elif flag in ("CERR","EXC","TOUT") and status == "PASS": status = flag
            # Print progress for long-running modules
            if t > 0 and (hi + 1) % 5 == 0:
                print(f"    [{hi+1}/{len(harnesses)}] {module}|{backend}: {total_pass}/{total_vecs} so far")
        results[f"{module}|{backend}"] = {"status":status,"pass":total_pass,"total":total_vecs}
        print(f"  [{bi+1}/{len(backends)}] {module}|{backend}: {total_pass}/{total_vecs}")
        save_results(results)
    print(f"  Elapsed: {time.time()-t0:.0f}s")

def clear_module_results(module, backends=None):
    """Clear per-harness detail for a module so it re-runs from scratch."""
    detail = load_detail()
    keys_to_remove = [k for k in detail if k.startswith(f"{module}|") and (backends is None or any(f"|{b}|" in k for b in backends))]
    for k in keys_to_remove:
        del detail[k]
    save_detail(detail)
    print(f"  Cleared {len(keys_to_remove)} harness results for {module}")

if __name__ == "__main__":
    args = sys.argv[1:]
    if args and args[0] == "--clear":
        # --clear <module> [backends...]
        module = args[1] if len(args) > 1 else ""
        backends = args[2:] if len(args) > 2 else None
        if module:
            clear_module_results(module, backends)
    else:
        module = args[0] if args else "sha1"
        backends = args[1:] if len(args) > 1 else ["x86_64"]
        run_module(module, backends)
