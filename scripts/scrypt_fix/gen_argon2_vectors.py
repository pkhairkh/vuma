#!/usr/bin/env python3
"""Generate argon2 test vectors and harnesses."""
import json, os, hashlib
# Use argon2-cffi for reference
from argon2 import low_level as argon2_low

REPO = "/home/z/my-project/vuma"
VECTORS_DIR = f"{REPO}/test_results/standard_vectors"
HARNESS_DIR = f"{REPO}/tests/compact_harnesses"

# Constraints (from argon2.vuma):
#   Argon2Buf = 8192 bytes (param_buf, h0, etc.)
#   Argon2Mem = 524288 bytes (memory matrix; m_prime <= 512)
# So m must be small (m_prime = 4*p*m, must be <= 512; for p=1, m <= 128)
# For speed, use t=1, m=8, p=1 (very low memory, fast)

def gen_vectors():
    vecs = []
    # v0: RFC 9106 §4 test vector (if it fits — actually m=32 t=3 p=4 doesn't fit;
    # use a smaller equivalent)
    # Use small params for all vectors to fit in constraints
    test_cases = [
        # (desc, password, salt, t, m, p, dklen, secret, ad) — salt >= 8 bytes (argon2 min)
        ("argon2id empty pw/'salt1234' t=1 m=8 p=1 dklen=32",
         b'', b'salt1234', 1, 8, 1, 32, b'', b''),
        ("argon2id 'password'/'salt1234' t=1 m=8 p=1 dklen=32",
         b'password', b'salt1234', 1, 8, 1, 32, b'', b''),
        ("argon2id 'password'/'salt1234' t=1 m=8 p=1 dklen=64",
         b'password', b'salt1234', 1, 8, 1, 64, b'', b''),
        ("argon2id 'password'/'salt1234' t=1 m=8 p=1 dklen=16",
         b'password', b'salt1234', 1, 8, 1, 16, b'', b''),
        ("argon2id 'password'/'salt1234' t=2 m=8 p=1 dklen=32",
         b'password', b'salt1234', 2, 8, 1, 32, b'', b''),
        ("argon2id 'password'/'salt1234' t=1 m=16 p=1 dklen=32",
         b'password', b'salt1234', 1, 16, 1, 32, b'', b''),
        ("argon2id 'password'/'salt1234' t=1 m=32 p=1 dklen=32",
         b'password', b'salt1234', 1, 32, 1, 32, b'', b''),
        ("argon2id 'password'/'salt1234' t=1 m=64 p=1 dklen=32",
         b'password', b'salt1234', 1, 64, 1, 32, b'', b''),
        ("argon2id 'password'/'salt1234' t=1 m=128 p=1 dklen=32",
         b'password', b'salt1234', 1, 128, 1, 32, b'', b''),
        ("argon2id 'longerpassword'/'longersalt' t=1 m=8 p=1 dklen=32",
         b'longerpassword', b'longersalt', 1, 8, 1, 32, b'', b''),
        ("argon2id empty/'somesalt' t=1 m=8 p=1 dklen=32",
         b'', b'somesalt', 1, 8, 1, 32, b'', b''),
        ("argon2id 'somepassword'/'minimums' t=1 m=8 p=1 dklen=32",
         b'somepassword', b'minimums', 1, 8, 1, 32, b'', b''),
        ("argon2id binary pw/salt t=1 m=8 p=1 dklen=32",
         bytes(range(16)), bytes(range(16,32)), 1, 8, 1, 32, b'', b''),
        ("argon2id 'P@ssw0rd!'/'$4lty$4lt' t=1 m=8 p=1 dklen=32",
         b'P@ssw0rd!', b'$4lty$4lt', 1, 8, 1, 32, b'', b''),
        ("argon2id 'a'*16/'b'*16 t=1 m=8 p=1 dklen=32",
         b'a'*16, b'b'*16, 1, 8, 1, 32, b'', b''),
        ("argon2id 'test'/'testsalt' t=3 m=8 p=1 dklen=32",
         b'test', b'testsalt', 3, 8, 1, 32, b'', b''),
        ("argon2id 'test'/'testsalt' t=1 m=8 p=1 dklen=64",
         b'test', b'testsalt', 1, 8, 1, 64, b'', b''),
        ("argon2id 'test'/'testsalt' t=1 m=8 p=1 dklen=24",
         b'test', b'testsalt', 1, 8, 1, 24, b'', b''),
        ("argon2id 'xor'/'cryptosalt' t=2 m=16 p=1 dklen=48",
         b'xor', b'cryptosalt', 2, 16, 1, 48, b'', b''),
        ("argon2id 'final'/'vectorsalt' t=1 m=8 p=1 dklen=32",
         b'final', b'vectorsalt', 1, 8, 1, 32, b'', b''),
    ]
    for desc, pw, salt, t, m, p, dklen, secret, ad in test_cases:
        # Use argon2-cffi's low_level API for direct control
        out = argon2_low.hash_secret_raw(
            secret=pw,
            salt=salt,
            time_cost=t,
            memory_cost=m,
            parallelism=p,
            hash_len=dklen,
            type=argon2_low.Type.ID,
        )
        vecs.append({
            "desc": desc,
            "pass_hex": pw.hex(),
            "salt_hex": salt.hex(),
            "t": t, "m": m, "p": p,
            "dklen": dklen,
            "expected_hex": out.hex(),
        })
    return vecs


def gen_harness(vec, idx):
    pw_hex = vec["pass_hex"]
    salt_hex = vec["salt_hex"]
    t, m, p, dklen = vec["t"], vec["m"], vec["p"], vec["dklen"]
    pw_bytes = bytes.fromhex(pw_hex)
    salt_bytes = bytes.fromhex(salt_hex)

    lines = []
    lines.append(f"// argon2 batch {idx} (vector {idx})")
    lines.append(f"// {vec['desc']}")
    # Import blake2 layouts + functions needed by argon2.vuma internally
    lines.append('import "/home/z/my-project/workdir/vuma/womb/crypto/hash/blake2.vuma"::{Blake2bCtx, Blake2bData, Blake2bDigest, blake2b_init_64, blake2b_update, blake2b_final};')
    # Import argon2 layouts + the argon2id function
    lines.append('import "/home/z/my-project/workdir/vuma/womb/crypto/mac_kdf/argon2.vuma"::{Argon2Buf, argon2id};')
    lines.append("")
    lines.append("transform main() -> i32 {")
    lines.append("    let password = state_new(Argon2Buf);")
    lines.append("    let salt = state_new(Argon2Buf);")
    lines.append("    let out = state_new(Argon2Buf);")
    for bi, b in enumerate(pw_bytes):
        lines.append(f"    password.bytes[{bi}] = {b};")
    for bi, b in enumerate(salt_bytes):
        lines.append(f"    salt.bytes[{bi}] = {b};")
    lines.append(f"    argon2id(password, {len(pw_bytes)}, salt, {len(salt_bytes)}, {t}, {m}, {p}, out, {dklen});")
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
    print(f"Generated {len(vecs)} argon2 vectors")
    os.makedirs(VECTORS_DIR, exist_ok=True)
    with open(f"{VECTORS_DIR}/argon2.json", "w") as f:
        json.dump({"module": "argon2", "vectors": vecs}, f, indent=2)
    os.makedirs(HARNESS_DIR, exist_ok=True)
    # Remove old harnesses
    for old in sorted(os.listdir(HARNESS_DIR)):
        if old.startswith("test_argon2_b") and old.endswith(".vuma"):
            os.remove(f"{HARNESS_DIR}/{old}")
    for i, v in enumerate(vecs):
        path = f"{HARNESS_DIR}/test_argon2_b{i}.vuma"
        with open(path, "w") as f:
            f.write(gen_harness(v, i))
    print(f"Generated {len(vecs)} harnesses")


if __name__ == "__main__":
    main()
