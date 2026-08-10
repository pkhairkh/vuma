#!/usr/bin/env python3
"""
Generate VUMA test harnesses v2 — loop-based design.
Uses arrays for vector data and a single loop to avoid variable redeclaration.
"""
import json, os
from pathlib import Path

VECTORS_DIR = "/home/z/my-project/vuma/test_results/vectors"
HARNESS_DIR = "/home/z/my-project/vuma/tests/full_validation"
os.makedirs(HARNESS_DIR, exist_ok=True)

MAX_INPUT_LEN = 128  # max bytes per vector input
MAX_KEY_LEN = 64     # max bytes per key
NUM_VECTORS = 20

# Module API registry
MODULE_APIS = {
    "sha1": {
        "import": 'import "/home/z/my-project/workdir/vuma/womb/crypto/hash/sha1.vuma"::{Sha1Data, Sha1Digest, sha1_oneshot};',
        "state_setup": "let data = state_new(Sha1Data);\n    let out = state_new(Sha1Digest);",
        "call": "sha1_oneshot(data, veclen, out);",
        "output_field": "out.data", "output_len": 20,
        "input_field": "data.data",
    },
    "sha256_sha224": {
        "import": 'import "/home/z/my-project/workdir/vuma/womb/crypto/hash/sha256_sha224.vuma"::{Sha256Ctx, Sha256Data, Sha256Digest, sha256_oneshot};',
        "state_setup": "let ctx = state_new(Sha256Ctx);\n    let data = state_new(Sha256Data);\n    let out = state_new(Sha256Digest);",
        "call": "sha256_oneshot(ctx, data, veclen, out);",
        "output_field": "out.data", "output_len": 32,
        "input_field": "data.data",
    },
    "sha384": {
        "import": 'import "/home/z/my-project/workdir/vuma/womb/crypto/hash/sha384.vuma"::{Sha384Data, Sha384Digest, sha384_oneshot};',
        "state_setup": "let data = state_new(Sha384Data);\n    let out = state_new(Sha384Digest);",
        "call": "sha384_oneshot(data, veclen, out);",
        "output_field": "out.data", "output_len": 48,
        "input_field": "data.data",
    },
    "sha512": {
        "import": 'import "/home/z/my-project/workdir/vuma/womb/crypto/hash/sha512.vuma"::{Sha512Data, Sha512Digest, sha512_oneshot};',
        "state_setup": "let data = state_new(Sha512Data);\n    let out = state_new(Sha512Digest);",
        "call": "sha512_oneshot(data, veclen, out);",
        "output_field": "out.data", "output_len": 64,
        "input_field": "data.data",
    },
    "sha3": {
        "import": 'import "/home/z/my-project/workdir/vuma/womb/crypto/hash/sha3.vuma"::{KeccakIn, KeccakOut, sha3_256_oneshot};',
        "state_setup": "let data = state_new(KeccakIn);\n    let out = state_new(KeccakOut);",
        "call": "sha3_256_oneshot(data, veclen, out);",
        "output_field": "out.data", "output_len": 32,
        "input_field": "data.data",
    },
    "blake2": {
        "import": 'import "/home/z/my-project/workdir/vuma/womb/crypto/hash/blake2.vuma"::{Blake2bData, Blake2bDigest, blake2b_oneshot};',
        "state_setup": "let data = state_new(Blake2bData);\n    let out = state_new(Blake2bDigest);",
        "call": "blake2b_oneshot(data, veclen, out);",
        "output_field": "out.data", "output_len": 64,
        "input_field": "data.data",
    },
    "blake3": {
        "import": 'import "/home/z/my-project/workdir/vuma/womb/crypto/hash/blake3.vuma"::{Blake3Data, Blake3Digest, blake3_oneshot};',
        "state_setup": "let data = state_new(Blake3Data);\n    let out = state_new(Blake3Digest);",
        "call": "blake3_oneshot(data, veclen, out);",
        "output_field": "out.data", "output_len": 32,
        "input_field": "data.data",
    },
    "md5": {
        "import": 'import "/home/z/my-project/workdir/vuma/womb/crypto/hash/md5.vuma"::{Md5Data, Md5Digest, md5_oneshot};',
        "state_setup": "let data = state_new(Md5Data);\n    let out = state_new(Md5Digest);",
        "call": "md5_oneshot(data, veclen, out);",
        "output_field": "out.data", "output_len": 16,
        "input_field": "data.data",
    },
    "aes128": {
        "import": 'import "/home/z/my-project/workdir/vuma/womb/crypto/symmetric/aes128.vuma"::{AesCtx, AesKey, AesBlock, aes128_init, aes_encrypt_block};',
        "state_setup": "let ctx = state_new(AesCtx);\n    let key = state_new(AesKey);\n    let pt = state_new(AesBlock);\n    let ct = state_new(AesBlock);",
        "call": "aes128_init(ctx, key);\n    aes_encrypt_block(ctx, pt, ct);",
        "output_field": "ct.data", "output_len": 16,
        "input_field": "pt.data", "has_key": True, "key_field": "key.data", "key_len": 16,
    },
    "aes192": {
        "import": 'import "/home/z/my-project/workdir/vuma/womb/crypto/symmetric/aes192.vuma"::{AesCtx, AesKey, AesBlock, aes192_init, aes_encrypt_block};',
        "state_setup": "let ctx = state_new(AesCtx);\n    let key = state_new(AesKey);\n    let pt = state_new(AesBlock);\n    let ct = state_new(AesBlock);",
        "call": "aes192_init(ctx, key);\n    aes_encrypt_block(ctx, pt, ct);",
        "output_field": "ct.data", "output_len": 16,
        "input_field": "pt.data", "has_key": True, "key_field": "key.data", "key_len": 24,
    },
    "aes256": {
        "import": 'import "/home/z/my-project/workdir/vuma/womb/crypto/symmetric/aes256.vuma"::{AesCtx, AesKey, AesBlock, aes256_init, aes_encrypt_block};',
        "state_setup": "let ctx = state_new(AesCtx);\n    let key = state_new(AesKey);\n    let pt = state_new(AesBlock);\n    let ct = state_new(AesBlock);",
        "call": "aes256_init(ctx, key);\n    aes_encrypt_block(ctx, pt, ct);",
        "output_field": "ct.data", "output_len": 16,
        "input_field": "pt.data", "has_key": True, "key_field": "key.data", "key_len": 32,
    },
    "des": {
        "import": 'import "/home/z/my-project/workdir/vuma/womb/crypto/symmetric/des.vuma"::{DesCtx, DesKey, DesBlock, des_init, des_encrypt_block};',
        "state_setup": "let ctx = state_new(DesCtx);\n    let key = state_new(DesKey);\n    let pt = state_new(DesBlock);\n    let ct = state_new(DesBlock);",
        "call": "des_init(ctx, key);\n    des_encrypt_block(ctx, pt, ct);",
        "output_field": "ct.data", "output_len": 8,
        "input_field": "pt.data", "has_key": True, "key_field": "key.data", "key_len": 8,
    },
    "rc4": {
        "import": 'import "/home/z/my-project/workdir/vuma/womb/crypto/symmetric/rc4.vuma"::{Rc4Ctx, Rc4Key, Rc4Data, rc4_init, rc4_crypt};',
        "state_setup": "let ctx = state_new(Rc4Ctx);\n    let key = state_new(Rc4Key);\n    let data = state_new(Rc4Data);",
        "call": "rc4_init(ctx, key, KEYLEN);\n    rc4_crypt(ctx, data, veclen);",
        "output_field": "data.data", "output_len": 16,
        "input_field": "data.data", "has_key": True, "key_field": "key.data", "key_len": 16,
    },
    "salsa20": {
        "import": 'import "/home/z/my-project/workdir/vuma/womb/crypto/symmetric/salsa20.vuma"::{Salsa20Ctx, Salsa20Key, Salsa20Nonce, Salsa20Buf, salsa20_init, salsa20_encrypt};',
        "state_setup": "let ctx = state_new(Salsa20Ctx);\n    let key = state_new(Salsa20Key);\n    let nonce = state_new(Salsa20Nonce);\n    let data = state_new(Salsa20Buf);",
        "call": "salsa20_init(ctx, key, nonce);\n    salsa20_encrypt(ctx, data, veclen);",
        "output_field": "data.data", "output_len": 64,
        "input_field": "data.data", "has_key": True, "key_field": "key.data", "key_len": 32,
    },
    "chacha20": {
        "import": 'import "/home/z/my-project/workdir/vuma/womb/crypto/symmetric/chacha20.vuma"::{ChaCha20Ctx, ChaCha20Key, ChaCha20Nonce, ChaCha20Buf, chacha20_init, chacha20_encrypt};',
        "state_setup": "let ctx = state_new(ChaCha20Ctx);\n    let key = state_new(ChaCha20Key);\n    let nonce = state_new(ChaCha20Nonce);\n    let data = state_new(ChaCha20Buf);",
        "call": "chacha20_init(ctx, key, nonce);\n    chacha20_encrypt(ctx, data, veclen);",
        "output_field": "data.data", "output_len": 64,
        "input_field": "data.data", "has_key": True, "key_field": "key.data", "key_len": 32,
    },
    "poly1305": {
        "import": 'import "/home/z/my-project/workdir/vuma/womb/crypto/symmetric/poly1305.vuma"::{Poly1305Ctx, Poly1305Key, Poly1305Msg, Poly1305Tag, poly1305_init, poly1305_update, poly1305_final};',
        "state_setup": "let ctx = state_new(Poly1305Ctx);\n    let key = state_new(Poly1305Key);\n    let msg = state_new(Poly1305Msg);\n    let tag = state_new(Poly1305Tag);",
        "call": "poly1305_init(ctx, key);\n    poly1305_update(ctx, msg, veclen);\n    poly1305_final(ctx, tag);",
        "output_field": "tag.data", "output_len": 16,
        "input_field": "msg.data", "has_key": True, "key_field": "key.data", "key_len": 32,
    },
    "hmac": {
        "import": 'import "/home/z/my-project/workdir/vuma/womb/crypto/mac_kdf/hmac.vuma"::{HmacKey, HmacMsg, HmacOut, hmac_sha256};',
        "state_setup": "let key = state_new(HmacKey);\n    let msg = state_new(HmacMsg);\n    let out = state_new(HmacOut);",
        "call": "hmac_sha256(key, KEYLEN, msg, veclen, out);",
        "output_field": "out.data", "output_len": 32,
        "input_field": "msg.data", "has_key": True, "key_field": "key.data", "key_len": 32,
    },
}

def hex_to_bytes(hex_str):
    if not hex_str:
        return []
    return [int(hex_str[i:i+2], 16) for i in range(0, len(hex_str), 2)]

def gen_harness(module_name, api, vectors):
    """Generate loop-based VUMA harness."""
    has_key = api.get("has_key", False)
    input_field = api["input_field"]
    key_field = api.get("key_field", "")
    key_len = api.get("key_len", 0)
    output_field = api["output_field"]
    output_len = api["output_len"]
    call = api["call"]

    # Total data sizes
    total_input_size = NUM_VECTORS * MAX_INPUT_LEN
    total_key_size = NUM_VECTORS * MAX_KEY_LEN if has_key else 0

    lines = []
    lines.append(f"// Auto-generated VUMA harness for {module_name}")
    lines.append(f"// Tests {NUM_VECTORS} vectors against C reference (OpenSSL)")
    lines.append(f"// Output: each result byte as print_int(byte+1000), delimiter print_int(999)")
    lines.append("")
    lines.append(api["import"])
    lines.append("")

    # Define layouts for vector data storage
    lines.append(f"layout VecInputs = {{ data: [u8; {total_input_size}] }}")
    lines.append(f"layout VecLens = {{ len: [u32; {NUM_VECTORS}] }}")
    if has_key:
        lines.append(f"layout VecKeys = {{ data: [u8; {total_key_size}] }}")
    lines.append("")

    lines.append("transform main() -> i32 {")

    # State setup
    lines.append(f"    {api['state_setup']}")
    lines.append(f"    let vec_inputs = state_new(VecInputs);")
    lines.append(f"    let vec_lens = state_new(VecLens);")
    if has_key:
        lines.append(f"    let vec_keys = state_new(VecKeys);")

    # Hardcode vector data
    for vi, vec in enumerate(vectors):
        input_hex = vec.get("input_hex", "")
        input_bytes = hex_to_bytes(input_hex)
        base = vi * MAX_INPUT_LEN
        for bi, b in enumerate(input_bytes):
            lines.append(f"    vec_inputs.data[{base + bi}] = {b};")
        lines.append(f"    vec_lens.len[{vi}] = {len(input_bytes)};")

        if has_key:
            key_hex = vec.get("key_hex", "")
            key_bytes = hex_to_bytes(key_hex)
            kbase = vi * MAX_KEY_LEN
            for bi, b in enumerate(key_bytes):
                lines.append(f"    vec_keys.data[{kbase + bi}] = {b};")

    # Main loop
    lines.append("")
    lines.append(f"    let vi: u32 = 0;")
    lines.append(f"    while vi < {NUM_VECTORS} {{")

    # Copy input data to module's input buffer
    lines.append(f"        // Copy input for vector vi")
    lines.append(f"        let veclen: u32 = vec_lens.len[vi];")
    lines.append(f"        let base: u32 = vi * {MAX_INPUT_LEN};")
    lines.append(f"        let j: u32 = 0;")
    lines.append(f"        while j < veclen {{")
    lines.append(f"            {input_field}[j] = vec_inputs.data[base + j];")
    lines.append(f"            j = j + 1;")
    lines.append(f"        }}")

    # Copy key data if needed
    if has_key:
        lines.append(f"        // Copy key for vector vi")
        lines.append(f"        let kbase: u32 = vi * {MAX_KEY_LEN};")
        lines.append(f"        let kj: u32 = 0;")
        lines.append(f"        while kj < {key_len} {{")
        lines.append(f"            {key_field}[kj] = vec_keys.data[kbase + kj];")
        lines.append(f"            kj = kj + 1;")
        lines.append(f"        }}")
        # Replace KEYLEN in call
        actual_call = call.replace("KEYLEN", str(key_len))
    else:
        actual_call = call

    # Call the module function
    lines.append(f"        // Run module")
    for cl in actual_call.strip().split("\n"):
        lines.append(f"        {cl.strip()}")

    # Output result bytes
    lines.append(f"        // Output {output_len} result bytes")
    lines.append(f"        let oi: u32 = 0;")
    lines.append(f"        while oi < {output_len} {{")
    lines.append(f"            print_int(({output_field}[oi] as u32) + 1000);")
    lines.append(f"            oi = oi + 1;")
    lines.append(f"        }}")
    lines.append(f"        print_int(999); // vector delimiter")

    lines.append(f"        vi = vi + 1;")
    lines.append(f"    }}")

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
