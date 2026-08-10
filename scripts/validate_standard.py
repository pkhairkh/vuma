#!/usr/bin/env python3
"""
Validate VUMA harnesses against standard test vectors.
Compiles each harness on specified backends, runs it, parses output,
and compares to expected vectors from NIST/RFC/pycryptodome.
"""
import json, os, subprocess, sys, time

REPO = "/home/z/my-project/vuma"
COMPILE_DUMP = f"{REPO}/target/debug/compile_dump"
if os.path.exists(f"{REPO}/target/release/compile_dump"):
    COMPILE_DUMP = f"{REPO}/target/release/compile_dump"
HARNESS_DIR = f"{REPO}/tests/standard_harnesses"
VECTORS_DIR = f"{REPO}/test_results/standard_vectors"
OUTDIR = "/tmp/vuma_std_val"
RESULTS_FILE = f"{REPO}/test_results/standard_results.json"
os.makedirs(OUTDIR, exist_ok=True)

BACKENDS = ["x86_64","x86_32","aarch64","aarch64_be","arm32","armeb","riscv64","riscv32",
            "mips64","mips64be","ppc64","ppc64le","loongarch64","s390x","sparc64","alpha",
            "hppa","m68k","wasm32"]

QEMU = {"x86_64":None,"x86_32":"qemu-i386-static","aarch64":"qemu-aarch64-static",
        "aarch64_be":"qemu-aarch64_be-static","arm32":"qemu-arm-static","armeb":"qemu-armeb-static",
        "riscv64":"qemu-riscv64-static","riscv32":"qemu-riscv32-static","mips64":"qemu-mips64el-static",
        "mips64be":"qemu-mips64-static","ppc64":"qemu-ppc64-static","ppc64le":"qemu-ppc64le-static",
        "loongarch64":"qemu-loongarch64-static","s390x":"qemu-s390x-static","sparc64":"qemu-sparc64-static",
        "alpha":"qemu-alpha-static","hppa":"qemu-hppa-static","m68k":"qemu-m68k-static","wasm32":"wasm32"}

def parse_output(raw):
    """Parse print_int output. Handles decimal and hex formats."""
    text = raw.decode("utf-8","replace")
    text = "".join(text.split())
    # Detect 8-digit hex format (arm32/armeb)
    is_hex = False
    if len(text) >= 8 and text[:4] == "0000":
        try:
            val = int(text[:8], 16)
            if 1000 <= val <= 1255: is_hex = True
        except ValueError: pass
    results = []
    cur = ""
    i = 0
    if is_hex:
        delim = "%08x" % 999
        while i + 8 <= len(text):
            chunk = text[i:i+8]
            if chunk == delim:
                results.append(cur); cur = ""; i += 8; continue
            try:
                val = int(chunk, 16)
                if 1000 <= val <= 1255:
                    cur += "%02x" % (val-1000); i += 8; continue
            except ValueError: pass
            i += 1
    else:
        while i < len(text):
            if i + 3 <= len(text) and text[i:i+3] == "999":
                results.append(cur); cur = ""; i += 3; continue
            if i + 4 <= len(text):
                chunk = text[i:i+4]
                try:
                    v = int(chunk)
                    if 1000 <= v <= 1255:
                        cur += "%02x" % (v-1000); i += 4; continue
                except ValueError: pass
            i += 1
    if cur: results.append(cur)
    return results

def load_results():
    if os.path.exists(RESULTS_FILE):
        with open(RESULTS_FILE) as f: return json.load(f)
    return {}

def save_results(data):
    with open(RESULTS_FILE, "w") as f: json.dump(data, f, indent=2)

def run_module(module, backends):
    vec_path = f"{VECTORS_DIR}/{module}.json"
    if not os.path.exists(vec_path):
        print(f"  {module}: NO VECTORS"); return
    with open(vec_path) as f: vec_data = json.load(f)
    vectors = vec_data["vectors"]
    harness = f"{HARNESS_DIR}/test_{module}_std.vuma"
    if not os.path.exists(harness):
        print(f"  {module}: NO HARNESS"); return

    results = load_results()
    t0 = time.time()
    for bi, backend in enumerate(backends):
        bin_path = f"{OUTDIR}/{module}_{backend}.bin"
        try:
            r = subprocess.run([COMPILE_DUMP, harness, bin_path, backend, "--no-verify"],
                             capture_output=True, text=True, timeout=120)
            if r.returncode != 0 or not os.path.exists(bin_path):
                results[f"{module}|{backend}"] = {"status":"CERR","pass":0,"total":len(vectors),
                    "err":(r.stderr or r.stdout or "")[:150]}
                print(f"  [{bi+1}/{len(backends)}] {module}|{backend}: CERR")
                save_results(results); continue
        except Exception as e:
            results[f"{module}|{backend}"] = {"status":"EXC","pass":0,"total":len(vectors),"err":str(e)[:150]}
            print(f"  [{bi+1}/{len(backends)}] {module}|{backend}: EXC")
            save_results(results); continue
        q = QEMU[backend]
        try:
            if backend == "wasm32":
                r = subprocess.run(["python3",f"{REPO}/scripts/wasm32_runner.py",bin_path],capture_output=True,timeout=60)
            elif q is None:
                r = subprocess.run([bin_path],capture_output=True,timeout=60)
            else:
                r = subprocess.run([q,bin_path],capture_output=True,timeout=60)
        except Exception as e:
            results[f"{module}|{backend}"] = {"status":"TOUT","pass":0,"total":len(vectors),"err":str(e)[:150]}
            print(f"  [{bi+1}/{len(backends)}] {module}|{backend}: TOUT")
            save_results(results); continue
        actual = parse_output(r.stdout)
        pass_count = 0
        for vi, vec in enumerate(vectors):
            exp = vec["expected_hex"]
            act = actual[vi] if vi < len(actual) else ""
            if act == exp: pass_count += 1
        status = "PASS" if pass_count == len(vectors) else "PARTIAL"
        results[f"{module}|{backend}"] = {"status":status,"pass":pass_count,"total":len(vectors)}
        print(f"  [{bi+1}/{len(backends)}] {module}|{backend}: {pass_count}/{len(vectors)}")
        save_results(results)
    print(f"  Elapsed: {time.time()-t0:.0f}s")

if __name__ == "__main__":
    module = sys.argv[1] if len(sys.argv) > 1 else "sha1"
    backends = sys.argv[2:] if len(sys.argv) > 2 else ["x86_64"]
    run_module(module, backends)
