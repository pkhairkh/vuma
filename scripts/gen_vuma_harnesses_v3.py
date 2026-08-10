#!/usr/bin/env python3
"""
Generate VUMA test harnesses v3 — sequential design with unique variable names.
Each vector uses unique variable names to avoid redeclaration.
No loops, no large arrays — just straight-line code.
"""
import json, os

VECTORS_DIR = "/home/z/my-project/vuma/test_results/vectors"
HARNESS_DIR = "/home/z/my-project/vuma/tests/full_validation"
os.makedirs(HARNESS_DIR, exist_ok=True)

NUM_VECTORS = 20

MODULE_APIS = {
    "sha1": {
        "import": 'import "/home/z/my-project/workdir/vuma/womb/crypto/hash/sha1.vuma"::{Sha1Data, Sha1Digest, sha1_oneshot};',
        "state_setup": "let data = state_new(Sha1Data);\n    let out = state_new(Sha1Digest);",
        "call": "sha1_oneshot(data{SUFFIX}, {LEN}, out{SUFFIX});",
        "output_field": "out{SUFFIX}.data", "output_len": 20,
        "input_field": "data{SUFFIX}.data",
    },
    "sha256_sha224": {
        "import": 'import "/home/z/my-project/workdir/vuma/womb/crypto/hash/sha256_sha224.vuma"::{Sha256Ctx, Sha256Data, Sha256Digest, sha256_oneshot};',
        "state_setup": "let ctx = state_new(Sha256Ctx);\n    let data = state_new(Sha256Data);\n    let out = state_new(Sha256Digest);",
        "call": "sha256_oneshot(ctx, data{SUFFIX}, {LEN}, out{SUFFIX});",
        "output_field": "out{SUFFIX}.data", "output_len": 32,
        "input_field": "data{SUFFIX}.data",
    },
    "sha384": {
        "import": 'import "/home/z/my-project/workdir/vuma/womb/crypto/hash/sha384.vuma"::{Sha384Data, Sha384Digest, sha384_oneshot};',
        "state_setup": "let data = state_new(Sha384Data);\n    let out = state_new(Sha384Digest);",
        "call": "sha384_oneshot(data{SUFFIX}, {LEN}, out{SUFFIX});",
        "output_field": "out{SUFFIX}.data", "output_len": 48,
        "input_field": "data{SUFFIX}.data",
    },
    "sha512": {
        "import": 'import "/home/z/my-project/workdir/vuma/womb/crypto/hash/sha512.vuma"::{Sha512Data, Sha512Digest, sha512_oneshot};',
        "state_setup": "let data = state_new(Sha512Data);\n    let out = state_new(Sha512Digest);",
        "call": "sha512_oneshot(data{SUFFIX}, {LEN}, out{SUFFIX});",
        "output_field": "out{SUFFIX}.data", "output_len": 64,
        "input_field": "data{SUFFIX}.data",
    },
    "md5": {
        "import": 'import "/home/z/my-project/workdir/vuma/womb/crypto/hash/md5.vuma"::{Md5Data, Md5Digest, md5_oneshot};',
        "state_setup": "let data = state_new(Md5Data);\n    let out = state_new(Md5Digest);",
        "call": "md5_oneshot(data{SUFFIX}, {LEN}, out{SUFFIX});",
        "output_field": "out{SUFFIX}.data", "output_len": 16,
        "input_field": "data{SUFFIX}.data",
    },
    "aes128": {
        "import": 'import "/home/z/my-project/workdir/vuma/womb/crypto/symmetric/aes128.vuma"::{AesCtx, AesKey, AesBlock, aes128_init, aes_encrypt_block};',
        "state_setup": "let ctx = state_new(AesCtx);\n    let key = state_new(AesKey);\n    let pt = state_new(AesBlock);\n    let ct = state_new(AesBlock);",
        "call": "aes128_init(ctx, key{SUFFIX});\n    aes_encrypt_block(ctx, pt{SUFFIX}, ct{SUFFIX});",
        "output_field": "ct{SUFFIX}.data", "output_len": 16,
        "input_field": "pt{SUFFIX}.data", "has_key": True, "key_field": "key{SUFFIX}.data", "key_len": 16,
    },
    "aes192": {
        "import": 'import "/home/z/my-project/workdir/vuma/womb/crypto/symmetric/aes192.vuma"::{AesCtx, AesKey, AesBlock, aes192_init, aes_encrypt_block};',
        "state_setup": "let ctx = state_new(AesCtx);\n    let key = state_new(AesKey);\n    let pt = state_new(AesBlock);\n    let ct = state_new(AesBlock);",
        "call": "aes192_init(ctx, key{SUFFIX});\n    aes_encrypt_block(ctx, pt{SUFFIX}, ct{SUFFIX});",
        "output_field": "ct{SUFFIX}.data", "output_len": 16,
        "input_field": "pt{SUFFIX}.data", "has_key": True, "key_field": "key{SUFFIX}.data", "key_len": 24,
    },
    "aes256": {
        "import": 'import "/home/z/my-project/workdir/vuma/womb/crypto/symmetric/aes256.vuma"::{AesCtx, AesKey, AesBlock, aes256_init, aes_encrypt_block};',
        "state_setup": "let ctx = state_new(AesCtx);\n    let key = state_new(AesKey);\n    let pt = state_new(AesBlock);\n    let ct = state_new(AesBlock);",
        "call": "aes256_init(ctx, key{SUFFIX});\n    aes_encrypt_block(ctx, pt{SUFFIX}, ct{SUFFIX});",
        "output_field": "ct{SUFFIX}.data", "output_len": 16,
        "input_field": "pt{SUFFIX}.data", "has_key": True, "key_field": "key{SUFFIX}.data", "key_len": 32,
    },
    "des": {
        "import": 'import "/home/z/my-project/workdir/vuma/womb/crypto/symmetric/des.vuma"::{DesCtx, DesKey, DesBlock, des_init, des_encrypt_block};',
        "state_setup": "let ctx = state_new(DesCtx);\n    let key = state_new(DesKey);\n    let pt = state_new(DesBlock);\n    let ct = state_new(DesBlock);",
        "call": "des_init(ctx, key{SUFFIX});\n    des_encrypt_block(ctx, pt{SUFFIX}, ct{SUFFIX});",
        "output_field": "ct{SUFFIX}.data", "output_len": 8,
        "input_field": "pt{SUFFIX}.data", "has_key": True, "key_field": "key{SUFFIX}.data", "key_len": 8,
    },
    "rc4": {
        "import": 'import "/home/z/my-project/workdir/vuma/womb/crypto/symmetric/rc4.vuma"::{Rc4Ctx, Rc4Key, Rc4Data, rc4_init, rc4_crypt};',
        "state_setup": "let ctx = state_new(Rc4Ctx);\n    let key = state_new(Rc4Key);\n    let data = state_new(Rc4Data);",
        "call": "rc4_init(ctx, key{SUFFIX}, {KEYLEN});\n    rc4_crypt(ctx, data{SUFFIX}, {LEN});",
        "output_field": "data{SUFFIX}.data", "output_len": 16,
        "input_field": "data{SUFFIX}.data", "has_key": True, "key_field": "key{SUFFIX}.data", "key_len": 16,
    },
    "hmac": {
        "import": 'import "/home/z/my-project/workdir/vuma/womb/crypto/mac_kdf/hmac.vuma"::{HmacKey, HmacMsg, HmacOut, hmac_sha256};',
        "state_setup": "let key = state_new(HmacKey);\n    let msg = state_new(HmacMsg);\n    let out = state_new(HmacOut);",
        "call": "hmac_sha256(key{SUFFIX}, {KEYLEN}, msg{SUFFIX}, {LEN}, out{SUFFIX});",
        "output_field": "out{SUFFIX}.data", "output_len": 32,
        "input_field": "msg{SUFFIX}.data", "has_key": True, "key_field": "key{SUFFIX}.data", "key_len": 32,
    },
}

def hex_to_bytes(hex_str):
    if not hex_str:
        return []
    return [int(hex_str[i:i+2], 16) for i in range(0, len(hex_str), 2)]

def gen_harness(module_name, api, vectors):
    has_key = api.get("has_key", False)
    output_len = api["output_len"]
    num_vecs = min(len(vectors), NUM_VECTORS)

    lines = []
    lines.append(f"// Auto-generated VUMA harness for {module_name}")
    lines.append(f"// Tests {num_vecs} vectors against C reference (OpenSSL)")
    lines.append(f"// Output: result bytes via print_int(byte+1000), delimiter print_int(999)")
    lines.append("")
    lines.append(api["import"])
    lines.append("")
    lines.append("transform main() -> i32 {")

    # Emit initial state setup (for vector 0, no suffix)
    lines.append(f"    // State for vector 0")
    for state_line in api["state_setup"].split("\n"):
        state_line = state_line.strip()
        if state_line:
            lines.append(f"    {state_line}")

    for vi in range(num_vecs):
        suffix = f"_v{vi}" if vi > 0 else ""
        vec = vectors[vi]
        input_hex = vec.get("input_hex", "")
        input_bytes = hex_to_bytes(input_hex)
        input_len = len(input_bytes)

        lines.append(f"    // Vector {vi}: {vec.get('desc', '')}")

        # Create per-vector state (with unique suffix for vi > 0)
        if vi > 0:
            for state_line in api["state_setup"].split("\n"):
                state_line = state_line.strip()
                if state_line:
                    # Replace variable names with suffixed versions
                    for var in ["ctx", "key", "data", "out", "pt", "ct", "msg", "pt", "nonce", "tag"]:
                        state_line = state_line.replace(f"let {var} ", f"let {var}{suffix} ")
                    lines.append(f"    {state_line}")

        # Set input bytes
        input_field = api["input_field"].replace("{SUFFIX}", suffix)
        for bi, b in enumerate(input_bytes):
            lines.append(f"    {input_field}[{bi}] = {b};")

        # Set key if needed
        if has_key:
            key_hex = vec.get("key_hex", "")
            key_bytes = hex_to_bytes(key_hex)
            key_field = api["key_field"].replace("{SUFFIX}", suffix)
            for bi, b in enumerate(key_bytes):
                lines.append(f"    {key_field}[{bi}] = {b};")

        # Call the function
        call = api["call"].replace("{SUFFIX}", suffix).replace("{LEN}", str(input_len)).replace("{KEYLEN}", str(api.get("key_len", 0)))
        for cl in call.strip().split("\n"):
            lines.append(f"    {cl.strip()}")

        # Output result bytes
        output_field = api["output_field"].replace("{SUFFIX}", suffix)
        lines.append(f"    print_int(1000 + ({output_field}[0] as u32));")
        lines.append(f"    print_int(1000 + ({output_field}[1] as u32));")
        if output_len <= 8:
            for oi in range(2, output_len):
                lines.append(f"    print_int(1000 + ({output_field}[{oi}] as u32));")
        else:
            # Use a loop with a unique counter variable
            lines.append(f"    let oi{suffix}: u32 = 2;")
            lines.append(f"    while oi{suffix} < {output_len} {{")
            lines.append(f"        print_int(1000 + ({output_field}[oi{suffix}] as u32));")
            lines.append(f"        oi{suffix} = oi{suffix} + 1;")
            lines.append(f"    }}")
        lines.append(f"    print_int(999);")
        lines.append("")

    lines.append("    return 0;")
    lines.append("}")
    return "\n".join(lines)

def main():
    generated = 0
    for module_name, api in MODULE_APIS.items():
        vec_path = f"{VECTORS_DIR}/{module_name}.json"
        if not os.path.exists(vec_path):
            continue
        with open(vec_path) as f:
            vec_data = json.load(f)
        vectors = vec_data["vectors"][:NUM_VECTORS]
        harness = gen_harness(module_name, api, vectors)
        out_path = f"{HARNESS_DIR}/test_{module_name}_20vec.vuma"
        with open(out_path, "w") as f:
            f.write(harness)
        generated += 1
        print(f"  {module_name:<30} → {out_path}")
    print(f"\nGenerated {generated} harnesses")

if __name__ == "__main__":
    main()
