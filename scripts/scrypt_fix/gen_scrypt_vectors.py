#!/usr/bin/env python3
"""
Generate scrypt test vectors that fit within VUMA's buffer constraints,
generate harnesses (1 vector per file) with proper pbkdf2 import,
and update test_results/compact_results.json.

Constraints (from scrypt.vuma):
- ScryptBuf = 8192 bytes
- HmacMsg = 256 bytes (used as b_msg in scrypt())
- b_len = p * 128 * r ≤ 256 (HmacMsg limit)  → p * r ≤ 2
- V array = n * 128 * r ≤ 8192 (ScryptBuf limit) → n * r ≤ 64
"""
import json, hashlib, os

REPO = "/home/z/my-project/vuma"
VECTORS_DIR = f"{REPO}/test_results/standard_vectors"
HARNESS_DIR = f"{REPO}/tests/compact_harnesses"

# Reference vectors from RFC 7914 §5 + additional small-param vectors
# Format: (desc, password_hex, salt_hex, N, r, p, dklen)
# We only include vectors whose memory requirements fit in VUMA's buffers.
#
# Memory budget:
#   b_len = p * 128 * r ≤ 256  →  p * r ≤ 2
#   V_size = n * 128 * r ≤ 8192  →  n * r ≤ 64
#
# RFC 7914 §5 vector 1 (N=16,r=1,p=1) fits. Vectors 2,3,4 do NOT fit.
# Generate 19 more small-param vectors to reach 20 total.
def gen_vectors():
    vecs = []
    # v0: RFC 7914 §5 vector 1 — empty password, empty salt
    out = hashlib.scrypt(b'', salt=b'', n=16, r=1, p=1, dklen=64)
    vecs.append({
        "desc": "scrypt(pw=b'', salt=b'', N=16, r=1, p=1, dklen=64) — RFC 7914 §5 v1",
        "pass_hex": "", "salt_hex": "",
        "n": 16, "r": 1, "p": 1, "dklen": 64,
        "expected_hex": out.hex(),
    })
    # v1: small password, small salt, N=16
    out = hashlib.scrypt(b'password', salt=b'NaCl', n=16, r=1, p=1, dklen=64)
    vecs.append({
        "desc": "scrypt(pw='password', salt='NaCl', N=16, r=1, p=1, dklen=64)",
        "pass_hex": "70617373776f7264", "salt_hex": "4e61436c",
        "n": 16, "r": 1, "p": 1, "dklen": 64,
        "expected_hex": out.hex(),
    })
    # v2-v4: vary dklen (16, 32, 128)
    for dklen in (16, 32, 128):
        out = hashlib.scrypt(b'password', salt=b'NaCl', n=16, r=1, p=1, dklen=dklen)
        vecs.append({
            "desc": f"scrypt(pw='password', salt='NaCl', N=16, r=1, p=1, dklen={dklen})",
            "pass_hex": "70617373776f7264", "salt_hex": "4e61436c",
            "n": 16, "r": 1, "p": 1, "dklen": dklen,
            "expected_hex": out.hex(),
        })
    # v5: N=32
    out = hashlib.scrypt(b'password', salt=b'NaCl', n=32, r=1, p=1, dklen=64)
    vecs.append({
        "desc": "scrypt(pw='password', salt='NaCl', N=32, r=1, p=1, dklen=64)",
        "pass_hex": "70617373776f7264", "salt_hex": "4e61436c",
        "n": 32, "r": 1, "p": 1, "dklen": 64,
        "expected_hex": out.hex(),
    })
    # v6: N=64
    out = hashlib.scrypt(b'password', salt=b'NaCl', n=64, r=1, p=1, dklen=64)
    vecs.append({
        "desc": "scrypt(pw='password', salt='NaCl', N=64, r=1, p=1, dklen=64)",
        "pass_hex": "70617373776f7264", "salt_hex": "4e61436c",
        "n": 64, "r": 1, "p": 1, "dklen": 64,
        "expected_hex": out.hex(),
    })
    # v7-v8: N=16, r=2 (still fits in V: 16*128*2=4096, b_len=256)
    out = hashlib.scrypt(b'password', salt=b'NaCl', n=16, r=2, p=1, dklen=64)
    vecs.append({
        "desc": "scrypt(pw='password', salt='NaCl', N=16, r=2, p=1, dklen=64)",
        "pass_hex": "70617373776f7264", "salt_hex": "4e61436c",
        "n": 16, "r": 2, "p": 1, "dklen": 64,
        "expected_hex": out.hex(),
    })
    out = hashlib.scrypt(b'password', salt=b'NaCl', n=16, r=2, p=1, dklen=32)
    vecs.append({
        "desc": "scrypt(pw='password', salt='NaCl', N=16, r=2, p=1, dklen=32)",
        "pass_hex": "70617373776f7264", "salt_hex": "4e61436c",
        "n": 16, "r": 2, "p": 1, "dklen": 32,
        "expected_hex": out.hex(),
    })
    # v9: N=16, p=2 (b_len=256, V=2048)
    out = hashlib.scrypt(b'password', salt=b'NaCl', n=16, r=1, p=2, dklen=64)
    vecs.append({
        "desc": "scrypt(pw='password', salt='NaCl', N=16, r=1, p=2, dklen=64)",
        "pass_hex": "70617373776f7264", "salt_hex": "4e61436c",
        "n": 16, "r": 1, "p": 2, "dklen": 64,
        "expected_hex": out.hex(),
    })
    # v10-v15: vary passwords and salts
    pws_salts = [
        (b'', b''),
        (b'short', b's'),
        (b'longerpasswordvalue', b'saltsalt'),
        (b'\x00\x01\x02\x03\x04', b'\xff\xfe\xfd\xfc'),
        (b'P@ssw0rd!', b'$4lty$4lt'),
        (b'a' * 32, b'b' * 16),
    ]
    for i, (pw, salt) in enumerate(pws_salts):
        out = hashlib.scrypt(pw, salt=salt, n=16, r=1, p=1, dklen=64)
        vecs.append({
            "desc": f"scrypt(pw={pw!r}, salt={salt!r}, N=16, r=1, p=1, dklen=64)",
            "pass_hex": pw.hex(), "salt_hex": salt.hex(),
            "n": 16, "r": 1, "p": 1, "dklen": 64,
            "expected_hex": out.hex(),
        })
    # v16-v19: vary N
    for N in (8, 32, 16, 64):
        pw, salt = b'testpw', b'testsalt'
        out = hashlib.scrypt(pw, salt=salt, n=N, r=1, p=1, dklen=64)
        vecs.append({
            "desc": f"scrypt(pw={pw!r}, salt={salt!r}, N={N}, r=1, p=1, dklen=64)",
            "pass_hex": pw.hex(), "salt_hex": salt.hex(),
            "n": N, "r": 1, "p": 1, "dklen": 64,
            "expected_hex": out.hex(),
        })
    return vecs


def gen_harness(vec, idx):
    """Generate a 1-vector harness file with proper pbkdf2 import."""
    pw_hex = vec["pass_hex"]
    salt_hex = vec["salt_hex"]
    n, r, p, dklen = vec["n"], vec["r"], vec["p"], vec["dklen"]
    pw_bytes = bytes.fromhex(pw_hex)
    salt_bytes = bytes.fromhex(salt_hex)

    lines = []
    lines.append(f"// scrypt batch {idx} (vector {idx})")
    lines.append(f"// {vec['desc']}")
    lines.append('import "/home/z/my-project/workdir/vuma/womb/crypto/mac_kdf/hmac.vuma"::{HmacKey, HmacMsg};')
    lines.append('import "/home/z/my-project/workdir/vuma/womb/crypto/mac_kdf/pbkdf2.vuma"::{pbkdf2_hmac_sha256};')
    lines.append('import "/home/z/my-project/workdir/vuma/womb/crypto/mac_kdf/scrypt.vuma"::{scrypt};')
    lines.append("")
    lines.append("transform main() -> i32 {")
    lines.append("    let pass = state_new(HmacKey);")
    lines.append("    let salt = state_new(HmacMsg);")
    lines.append("    let out = state_new(HmacMsg);")
    # Set password bytes
    for bi, b in enumerate(pw_bytes):
        lines.append(f"    pass.bytes[{bi}] = {b};")
    # Set salt bytes
    for bi, b in enumerate(salt_bytes):
        lines.append(f"    salt.bytes[{bi}] = {b};")
    lines.append(f"    scrypt(pass, {len(pw_bytes)}, salt, {len(salt_bytes)}, {n}, {r}, {p}, out, {dklen});")
    lines.append("    let oi: u32 = 0;")
    lines.append(f"    while oi < {dklen} {{")
    lines.append("        print_int(1000 + (out.bytes[oi] as u32));")
    lines.append("        oi = oi + 1;")
    lines.append("    }")
    lines.append("    print_int(999);")
    lines.append("    return 0;")
    lines.append("}")
    return "\n".join(lines) + "\n"


def main():
    vecs = gen_vectors()
    print(f"Generated {len(vecs)} scrypt vectors")
    # Save vectors JSON
    os.makedirs(VECTORS_DIR, exist_ok=True)
    with open(f"{VECTORS_DIR}/scrypt.json", "w") as f:
        json.dump({"module": "scrypt", "vectors": vecs}, f, indent=2)
    print(f"Saved vectors to {VECTORS_DIR}/scrypt.json")
    # Generate harnesses
    os.makedirs(HARNESS_DIR, exist_ok=True)
    # Remove old harnesses
    for old in sorted(os.listdir(HARNESS_DIR)):
        if old.startswith("test_scrypt_b") and old.endswith(".vuma"):
            os.remove(f"{HARNESS_DIR}/{old}")
    for i, v in enumerate(vecs):
        path = f"{HARNESS_DIR}/test_scrypt_b{i}.vuma"
        with open(path, "w") as f:
            f.write(gen_harness(v, i))
    print(f"Generated {len(vecs)} harnesses in {HARNESS_DIR}")


if __name__ == "__main__":
    main()
