#!/usr/bin/env python3
"""Regenerate bignum harnesses to output bytes in big-endian order
(matching the expected_hex format in the vectors JSON).

The bignum module stores 256-bit values as 4 little-endian u64 limbs
(limb[0] = lowest 64 bits, limb[3] = highest 64 bits).

The harness outputs bytes in the order: limb[3] high→low, limb[2] high→low, ..., limb[0] high→low
which is the standard big-endian byte representation of the 256-bit number.
"""
import json, os

REPO = "/home/z/my-project/vuma"
HARNESS_DIR = f"{REPO}/tests/compact_harnesses"
VECTORS_DIR = f"{REPO}/test_results/standard_vectors"


def split_u64_to_hi32_lo32(hex_str):
    """Convert a 64-bit hex value to (hi_u32, lo_u32) for VUMA literal syntax."""
    val = int(hex_str, 16)
    hi = (val >> 32) & 0xFFFFFFFF
    lo = val & 0xFFFFFFFF
    return hi, lo


def gen_harness(vec, idx):
    op = vec.get("op", "add")
    a_hex = vec["a_hex"]
    b_hex = vec["b_hex"]

    # Parse 256-bit hex (big-endian) into 4 LE limbs (limb[0] = lowest)
    a_val = int(a_hex, 16)
    b_val = int(b_hex, 16)
    a_limbs = [(a_val >> (64 * i)) & ((1 << 64) - 1) for i in range(4)]
    b_limbs = [(b_val >> (64 * i)) & ((1 << 64) - 1) for i in range(4)]

    # Determine which function to call
    if op == "add":
        func = "bn256_add"
    elif op == "sub":
        func = "bn256_sub"
    elif op == "mul":
        func = "bn256_mul_512"  # Note: produces 512-bit result
    elif op == "modexp":
        func = "bn256_mod_exp"
    else:
        func = "bn256_add"

    lines = []
    lines.append(f"// bignum batch {idx} (vector {idx}) — op={op}")
    lines.append(f"// {vec.get('desc', '')}")
    if op == "mul":
        lines.append('import "/home/z/my-project/workdir/vuma/womb/crypto/bignum/bignum.vuma"::{Bn256Ctx, Bn512Ctx, bn256_mul_512};')
    else:
        lines.append(f'import "/home/z/my-project/workdir/vuma/womb/crypto/bignum/bignum.vuma"::{{Bn256Ctx, {func}}};')
    lines.append("")
    lines.append("transform main() -> i32 {")
    lines.append("    let a = state_new(Bn256Ctx);")
    lines.append("    let b = state_new(Bn256Ctx);")
    if op == "mul":
        lines.append("    let r = state_new(Bn512Ctx);")
    else:
        lines.append("    let r = state_new(Bn256Ctx);")
    lines.append("")

    # Set a limbs (LE order: limb[0] = lowest)
    for i in range(4):
        hi, lo = split_u64_to_hi32_lo32(f"{a_limbs[i]:016x}")
        lines.append(f"    a.limbs[{i}] = (({hi} as u64) << 32) | ({lo} as u64);")
    lines.append("")
    for i in range(4):
        hi, lo = split_u64_to_hi32_lo32(f"{b_limbs[i]:016x}")
        lines.append(f"    b.limbs[{i}] = (({hi} as u64) << 32) | ({lo} as u64);")
    lines.append("")

    # Call the function
    if op == "mul":
        lines.append(f"    {func}(r, a, b);")
        # Output 8 limbs in BE order (limb[7] first, high byte first)
        lines.append("    let oi: u32 = 0;")
        lines.append("    while oi < 8 {")
        lines.append("        let v = r.limbs[7 - oi];")
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
    else:
        lines.append(f"    {func}(r, a, b);")
        # Output 4 limbs in BE order (limb[3] first, high byte first)
        lines.append("    let oi: u32 = 0;")
        lines.append("    while oi < 4 {")
        lines.append("        let v = r.limbs[3 - oi];")
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
    with open(f"{VECTORS_DIR}/bignum.json") as f:
        d = json.load(f)
    vectors = d["vectors"]
    print(f"Regenerating {len(vectors)} bignum harnesses (BE byte order)")
    for i, v in enumerate(vectors):
        path = f"{HARNESS_DIR}/test_bignum_b{i}.vuma"
        with open(path, "w") as f:
            f.write(gen_harness(v, i))
    print("Done")


if __name__ == "__main__":
    main()
