#!/usr/bin/env python3
"""Generate drbg test vectors and harnesses."""
import json, os, sys
sys.path.insert(0, '/home/z/my-project/scripts')
from hmac_drbg_ref import HMAC_DRBG

REPO = "/home/z/my-project/vuma"
VECTORS_DIR = f"{REPO}/test_results/standard_vectors"
HARNESS_DIR = f"{REPO}/tests/compact_harnesses"

def gen_vectors():
    vecs = []
    # 20 vectors: vary entropy, nonce, output length
    test_cases = [
        # (desc, entropy_hex, nonce_hex, personalization_hex, out_len, num_generate_calls)
        ("DRBG: 4-byte entropy, no nonce, 32 bytes", "01020304", "", "", 32, 1),
        ("DRBG: 8-byte entropy, no nonce, 32 bytes", "0102030405060708", "", "", 32, 1),
        ("DRBG: 16-byte entropy, no nonce, 32 bytes", "00112233445566778899aabbccddeeff", "", "", 32, 1),
        ("DRBG: 32-byte entropy, no nonce, 32 bytes", "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff", "", "", 32, 1),
        ("DRBG: 4-byte entropy, no nonce, 16 bytes", "01020304", "", "", 16, 1),
        ("DRBG: 4-byte entropy, no nonce, 64 bytes", "01020304", "", "", 64, 1),
        ("DRBG: 4-byte entropy, no nonce, 128 bytes", "01020304", "", "", 128, 1),
        ("DRBG: empty entropy, no nonce, 32 bytes", "", "", "", 32, 1),
        ("DRBG: 'a'*8 entropy, no nonce, 32 bytes", "6161616161616161", "", "", 32, 1),
        ("DRBG: 'P@ssw0rd' entropy, no nonce, 32 bytes", "5040737377307264", "", "", 32, 1),
        ("DRBG: 16-byte entropy, 8-byte nonce, 32 bytes", "00112233445566778899aabbccddeeff", "0102030405060708", "", 32, 1),
        ("DRBG: 16-byte entropy, no nonce, 32 bytes (v2)", "ffeeddccbbaa99887766554433221100", "", "", 32, 1),
        ("DRBG: 16-byte entropy, no nonce, 48 bytes", "00112233445566778899aabbccddeeff", "", "", 48, 1),
        ("DRBG: 16-byte entropy, no nonce, 24 bytes", "00112233445566778899aabbccddeeff", "", "", 24, 1),
        ("DRBG: binary entropy 0..15, 32 bytes", "000102030405060708090a0b0c0d0e0f", "", "", 32, 1),
        ("DRBG: 32-byte entropy (binary), 32 bytes", "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f", "", "", 32, 1),
        ("DRBG: 8-byte entropy v2, 32 bytes", "deadbeefcafebabe", "", "", 32, 1),
        ("DRBG: 4-byte entropy, 4-byte nonce, 32 bytes", "11223344", "55667788", "", 32, 1),
        ("DRBG: 8-byte entropy, no nonce, 80 bytes", "1122334455667788", "", "", 80, 1),
        ("DRBG: 16-byte entropy, no nonce, 96 bytes", "aabbccddeeff00112233445566778899", "", "", 96, 1),
    ]
    for desc, ent_hex, nonce_hex, pers_hex, out_len, num_gen in test_cases:
        ent = bytes.fromhex(ent_hex)
        nonce = bytes.fromhex(nonce_hex)
        pers = bytes.fromhex(pers_hex)
        d = HMAC_DRBG()
        d.instantiate(ent, nonce, pers)
        out = d.generate(out_len)
        vecs.append({
            "desc": desc,
            "entropy_hex": ent_hex,
            "nonce_hex": nonce_hex,
            "personalization_hex": pers_hex,
            "out_len": out_len,
            "expected_hex": out.hex(),
        })
    return vecs


def gen_harness(vec, idx):
    ent_hex = vec["entropy_hex"]
    nonce_hex = vec.get("nonce_hex", "")
    out_len = vec["out_len"]
    ent_bytes = bytes.fromhex(ent_hex)
    nonce_bytes = bytes.fromhex(nonce_hex)

    lines = []
    lines.append(f"// drbg batch {idx} (vector {idx})")
    lines.append(f"// {vec['desc']}")
    lines.append('import "/home/z/my-project/workdir/vuma/womb/crypto/mac_kdf/hmac.vuma"::{HmacKey, HmacMsg, HmacOut, hmac_sha256};')
    lines.append('import "/home/z/my-project/workdir/vuma/womb/crypto/drbg/drbg.vuma"::{DrbgCtx, DrbgData, DrbgOut, drbg_init, drbg_generate};')
    lines.append("")
    lines.append("transform main() -> i32 {")
    lines.append("    let drbg = state_new(DrbgCtx);")
    lines.append("    let entropy = state_new(DrbgData);")
    lines.append("    let nonce = state_new(DrbgData);")
    lines.append("    let out = state_new(DrbgOut);")
    for bi, b in enumerate(ent_bytes):
        lines.append(f"    entropy.bytes[{bi}] = {b};")
    for bi, b in enumerate(nonce_bytes):
        lines.append(f"    nonce.bytes[{bi}] = {b};")
    lines.append(f"    drbg_init(drbg, entropy, {len(ent_bytes)}, nonce, {len(nonce_bytes)});")
    lines.append(f"    drbg_generate(drbg, out, {out_len});")
    lines.append("    let oi: u32 = 0;")
    lines.append(f"    while oi < {out_len} {{")
    lines.append("        print_int(1000 + (out.bytes[oi] as u32));")
    lines.append("        oi = oi + 1;")
    lines.append("    }")
    lines.append("    print_int(999);")
    lines.append("    return 0;")
    lines.append("}")
    return "\n".join(lines) + "\n"


def main():
    vecs = gen_vectors()
    print(f"Generated {len(vecs)} drbg vectors")
    os.makedirs(VECTORS_DIR, exist_ok=True)
    with open(f"{VECTORS_DIR}/drbg.json", "w") as f:
        json.dump({"module": "drbg", "vectors": vecs}, f, indent=2)
    os.makedirs(HARNESS_DIR, exist_ok=True)
    for old in sorted(os.listdir(HARNESS_DIR)):
        if old.startswith("test_drbg_b") and old.endswith(".vuma"):
            os.remove(f"{HARNESS_DIR}/{old}")
    for i, v in enumerate(vecs):
        path = f"{HARNESS_DIR}/test_drbg_b{i}.vuma"
        with open(path, "w") as f:
            f.write(gen_harness(v, i))
    print(f"Generated {len(vecs)} harnesses")


if __name__ == "__main__":
    main()
