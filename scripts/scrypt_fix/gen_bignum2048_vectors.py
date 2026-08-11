#!/usr/bin/env python3
"""Generate bignum2048 test vectors and harnesses.

Tests bn2048_add and bn2048_sub with 2048-bit values.
The harness uses the same u64 literal splitting pattern as bignum:
  `((hi_u32 as u64) << 32) | (lo_u32 as u64)`
to avoid VUMA's u64 literal parser truncating values >= 2^63.
"""
import json, os, secrets

REPO = "/home/z/my-project/vuma"
VECTORS_DIR = f"{REPO}/test_results/standard_vectors"
HARNESS_DIR = f"{REPO}/tests/compact_harnesses"

def split_u64(hi32, lo32):
    """Generate VUMA literal for u64 from two u32 halves."""
    return f"(({hi32} as u64) << 32) | ({lo32} as u64)"

def gen_vectors():
    vecs = []
    # 20 vectors: mix of add and sub, various edge cases
    test_cases = [
        # Edge cases
        ("bn2048_add 0+0", "add", "00"*32, "00"*32),
        ("bn2048_add 1+0", "add", "01" + "00"*31, "00"*32),
        ("bn2048_add 1+1", "add", "01" + "00"*31, "01" + "00"*31),
        ("bn2048_add max+1 (carry)", "add", "ff"*32, "01" + "00"*31),
        ("bn2048_add max+max (carry)", "add", "ff"*32, "ff"*32),
        # Random values
        ("bn2048_add random 0", "add", secrets.token_hex(32), secrets.token_hex(32)),
        ("bn2048_add random 1", "add", secrets.token_hex(32), secrets.token_hex(32)),
        ("bn2048_add random 2", "add", secrets.token_hex(32), secrets.token_hex(32)),
        ("bn2048_add random 3", "add", secrets.token_hex(32), secrets.token_hex(32)),
        ("bn2048_add random 4", "add", secrets.token_hex(32), secrets.token_hex(32)),
        ("bn2048_add random 5", "add", secrets.token_hex(32), secrets.token_hex(32)),
        ("bn2048_add random 6", "add", secrets.token_hex(32), secrets.token_hex(32)),
        ("bn2048_add random 7", "add", secrets.token_hex(32), secrets.token_hex(32)),
        ("bn2048_add random 8", "add", secrets.token_hex(32), secrets.token_hex(32)),
        ("bn2048_add random 9", "add", secrets.token_hex(32), secrets.token_hex(32)),
        # Sub cases
        ("bn2048_sub 0-0", "sub", "00"*32, "00"*32),
        ("bn2048_sub 1-0", "sub", "01" + "00"*31, "00"*32),
        ("bn2048_sub 1-1", "sub", "01" + "00"*31, "01" + "00"*31),
        ("bn2048_sub max-0", "sub", "ff"*32, "00"*32),
        ("bn2048_sub max-max", "sub", "ff"*32, "ff"*32),
    ]
    for desc, op, a_hex, b_hex in test_cases:
        a = int(a_hex, 16)
        b = int(b_hex, 16)
        if op == "add":
            result = (a + b) % (1 << 2048)
        else:
            result = (a - b) % (1 << 2048)
        vecs.append({
            "desc": desc,
            "op": op,
            "a_hex": a_hex,
            "b_hex": b_hex,
            "expected_hex": hex(result)[2:].zfill(512),
        })
    return vecs


def gen_harness(vec, idx):
    op = vec["op"]
    a_hex = vec["a_hex"]
    b_hex = vec["b_hex"]
    a_val = int(a_hex, 16)
    b_val = int(b_hex, 16)
    # 32 LE limbs (limb[0] = lowest 64 bits)
    a_limbs = [(a_val >> (64 * i)) & ((1 << 64) - 1) for i in range(32)]
    b_limbs = [(b_val >> (64 * i)) & ((1 << 64) - 1) for i in range(32)]

    func = "bn2048_add" if op == "add" else "bn2048_sub"
    lines = []
    lines.append(f"// bignum2048 batch {idx} (vector {idx}) — op={op}")
    lines.append(f"// {vec['desc']}")
    lines.append(f'import "/home/z/my-project/workdir/vuma/womb/crypto/bignum/bignum2048.vuma"::{{Bn2048Ctx, {func}}};')
    lines.append("")
    lines.append("transform main() -> i32 {")
    lines.append("    let a = state_new(Bn2048Ctx);")
    lines.append("    let b = state_new(Bn2048Ctx);")
    lines.append("    let r = state_new(Bn2048Ctx);")
    lines.append("")
    for i in range(32):
        hi = (a_limbs[i] >> 32) & 0xFFFFFFFF
        lo = a_limbs[i] & 0xFFFFFFFF
        lines.append(f"    a.limbs[{i}] = {split_u64(hi, lo)};")
    lines.append("")
    for i in range(32):
        hi = (b_limbs[i] >> 32) & 0xFFFFFFFF
        lo = b_limbs[i] & 0xFFFFFFFF
        lines.append(f"    b.limbs[{i}] = {split_u64(hi, lo)};")
    lines.append("")
    lines.append(f"    {func}(r, a, b);")
    # Output 32 limbs in BE order (limb[31] first, high byte first)
    lines.append("    let oi: u32 = 0;")
    lines.append("    while oi < 32 {")
    lines.append("        let v = r.limbs[31 - oi];")
    lines.append("        print_int(1000 + (((v >> 56) & 255) as u32));")
    lines.append("        print_int(1000 + (((v >> 48) & 255) as u32));")
    lines.append("        print_int(1000 + (((v >> 40) & 255) as u32));")
    lines.append("        print_int(1000 + (((v >> 32) & 255) as u32));")
    lines.append("        print_int(1000 + (((v >> 24) & 255) as u32));")
    lines.append("        print_int(1000 + (((v >> 16) & 255) as u32));")
    lines.append("        print_int(1000 + (((v >> 8) & 255) as u32));")
    lines.append("        print_int(1000 + ((v & 255) as u32));")
    lines.append("        oi = oi + 1;")
    lines.append("    }")
    lines.append("    print_int(999);")
    lines.append("    return 0;")
    lines.append("}")
    return "\n".join(lines) + "\n"


def main():
    vecs = gen_vectors()
    print(f"Generated {len(vecs)} bignum2048 vectors")
    os.makedirs(VECTORS_DIR, exist_ok=True)
    with open(f"{VECTORS_DIR}/bignum2048.json", "w") as f:
        json.dump({"module": "bignum2048", "vectors": vecs}, f, indent=2)
    os.makedirs(HARNESS_DIR, exist_ok=True)
    for old in sorted(os.listdir(HARNESS_DIR)):
        if old.startswith("test_bignum2048_b") and old.endswith(".vuma"):
            os.remove(f"{HARNESS_DIR}/{old}")
    for i, v in enumerate(vecs):
        path = f"{HARNESS_DIR}/test_bignum2048_b{i}.vuma"
        with open(path, "w") as f:
            f.write(gen_harness(v, i))
    print(f"Generated {len(vecs)} harnesses")


if __name__ == "__main__":
    main()
