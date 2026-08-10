#!/usr/bin/env python3
"""Quick validation: run one module on specified backends, save results."""
import json, os, subprocess, sys, time

REPO = "/home/z/my-project/vuma"
COMPILE_DUMP = f"{REPO}/target/release/compile_dump"
HARNESS_DIR = f"{REPO}/tests/full_validation"
VECTORS_DIR = f"{REPO}/test_results/vectors"
OUTDIR = "/tmp/vuma_full_val"
RESULTS_FILE = f"{REPO}/test_results/validation_results.json"
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

def parse_output(raw, nvec, olen):
    """Parse print_int output. Handles multiple formats:
    - x86_64: decimal with newlines (4-digit: 1000-1255, delimiter: 999)
    - aarch64: decimal without newlines (concatenated 4-digit)
    - arm32: 8-digit hex with leading zeros (e.g. 000004c2 = 1218 = 0xDA+1000)
    - Others may vary
    """
    text = raw.decode("utf-8","replace")
    # Remove all whitespace for uniform processing
    text = "".join(text.split())

    # Detect format: if text contains mostly 8-digit hex patterns starting with 0000,
    # it's the arm32 hex format. Otherwise, it's decimal format.
    # Check first 8 chars: if they look like hex (0-9, a-f) and start with 0000,
    # use hex parser.
    is_hex = False
    if len(text) >= 8 and text[:4] == "0000":
        # Likely hex format — verify by checking if the value makes sense
        try:
            val = int(text[:8], 16)
            if 1000 <= val <= 1255:
                is_hex = True
        except ValueError:
            pass

    results = []
    cur = ""
    i = 0
    if is_hex:
        # Parse 8-digit hex values, delimiter is 000003e7 (999 in hex)
        delimiter_hex = "%08x" % 999  # 000003e7
        while i + 8 <= len(text):
            chunk = text[i:i+8]
            if chunk == delimiter_hex:
                results.append(cur)
                cur = ""
                i += 8
                continue
            try:
                val = int(chunk, 16)
                if 1000 <= val <= 1255:
                    cur += "%02x" % (val - 1000)
                    i += 8
                    continue
            except ValueError:
                pass
            i += 1
    else:
        # Parse decimal values (4-digit: 1000-1255, delimiter: 999)
        while i < len(text):
            # Check for 999 delimiter (3 digits)
            if i + 3 <= len(text) and text[i:i+3] == "999":
                results.append(cur)
                cur = ""
                i += 3
                continue
            # Try 4-digit value (1000-1255)
            if i + 4 <= len(text):
                chunk = text[i:i+4]
                try:
                    v = int(chunk)
                    if 1000 <= v <= 1255:
                        cur += "%02x" % (v - 1000)
                        i += 4
                        continue
                except ValueError:
                    pass
            i += 1
    if cur:
        results.append(cur)
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
        print(f"  {module}: NO VECTORS")
        return
    with open(vec_path) as f: vec_data = json.load(f)
    vectors = vec_data["vectors"]
    harness = f"{HARNESS_DIR}/test_{module}_20vec.vuma"
    if not os.path.exists(harness):
        print(f"  {module}: NO HARNESS")
        return

    results = load_results()
    t0 = time.time()
    
    for bi, backend in enumerate(backends):
        bin_path = f"{OUTDIR}/{module}_{backend}.bin"
        
        # Compile
        try:
            r = subprocess.run([COMPILE_DUMP, harness, bin_path, backend, "--no-verify"],
                             capture_output=True, text=True, timeout=120)
            if r.returncode != 0 or not os.path.exists(bin_path):
                results[f"{module}|{backend}"] = {"status":"CERR","pass":0,"total":20,"err":(r.stderr or r.stdout or "")[:100]}
                print(f"  [{bi+1}/{len(backends)}] {module}|{backend}: CERR")
                save_results(results)
                continue
        except Exception as e:
            results[f"{module}|{backend}"] = {"status":"EXC","pass":0,"total":20,"err":str(e)[:100]}
            print(f"  [{bi+1}/{len(backends)}] {module}|{backend}: EXC")
            save_results(results)
            continue
        
        # Run
        q = QEMU[backend]
        try:
            if backend == "wasm32":
                r = subprocess.run(["python3",f"{REPO}/scripts/wasm32_runner.py",bin_path],capture_output=True,timeout=60)
            elif q is None:
                r = subprocess.run([bin_path],capture_output=True,timeout=60)
            else:
                r = subprocess.run([q,bin_path],capture_output=True,timeout=60)
        except Exception as e:
            results[f"{module}|{backend}"] = {"status":"TIMEOUT","pass":0,"total":20,"err":str(e)[:100]}
            print(f"  [{bi+1}/{len(backends)}] {module}|{backend}: TIMEOUT")
            save_results(results)
            continue
        
        # Parse and compare
        actual = parse_output(r.stdout, len(vectors), 32)
        pass_count = 0
        for vi, vec in enumerate(vectors):
            exp = vec["expected_hex"]
            act = actual[vi] if vi < len(actual) else ""
            if act == exp: pass_count += 1
        
        status = "PASS" if pass_count == len(vectors) else "PARTIAL"
        results[f"{module}|{backend}"] = {"status":status,"pass":pass_count,"total":len(vectors)}
        print(f"  [{bi+1}/{len(backends)}] {module}|{backend}: {pass_count}/{len(vectors)}")
        save_results(results)
    
    elapsed = time.time() - t0
    print(f"  Elapsed: {elapsed:.0f}s")

if __name__ == "__main__":
    module = sys.argv[1] if len(sys.argv) > 1 else "sha1"
    backends = sys.argv[2:] if len(sys.argv) > 2 else BACKENDS
    run_module(module, backends)
