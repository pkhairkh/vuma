#!/usr/bin/env python3
"""
Generate VUMA test harnesses for all 46 womb/crypto modules.
Each harness tests 20 vectors, outputting result bytes via print_int(byte+1000).
Vector delimiter: print_int(999).
"""
import json, os
from pathlib import Path

VECTORS_DIR = "/home/z/my-project/vuma/test_results/vectors"
HARNESS_DIR = "/home/z/my-project/vuma/tests/full_validation"
os.makedirs(HARNESS_DIR, exist_ok=True)

# Module API registry: module_name -> {
#   import_path, symbols, layouts, setup_code, call_pattern, output_field, output_len
# }
MODULE_APIS = {
    # === Hash modules ===
    "sha1": {
        "import": 'import "/home/z/my-project/workdir/vuma/womb/crypto/hash/sha1.vuma"::{Sha1Data, Sha1Digest, sha1_oneshot};',
        "layouts": ["Sha1Data", "Sha1Digest"],
        "setup": "let data = state_new(Sha1Data);\n    let out = state_new(Sha1Digest);",
        "call": "sha1_oneshot(data, {len}, out);",
        "output_field": "out.data", "output_len": 20, "input_field": "data.data", "input_layout": "Sha1Data",
    },
    "sha256_sha224": {
        "import": 'import "/home/z/my-project/workdir/vuma/womb/crypto/hash/sha256_sha224.vuma"::{Sha256Ctx, Sha256Data, Sha256Digest, sha256_oneshot};',
        "layouts": ["Sha256Ctx", "Sha256Data", "Sha256Digest"],
        "setup": "let ctx = state_new(Sha256Ctx);\n    let data = state_new(Sha256Data);\n    let out = state_new(Sha256Digest);",
        "call": "sha256_oneshot(ctx, data, {len}, out);",
        "output_field": "out.data", "output_len": 32, "input_field": "data.data", "input_layout": "Sha256Data",
    },
    "sha384": {
        "import": 'import "/home/z/my-project/workdir/vuma/womb/crypto/hash/sha384.vuma"::{Sha384Data, Sha384Digest, sha384_oneshot};',
        "layouts": ["Sha384Data", "Sha384Digest"],
        "setup": "let data = state_new(Sha384Data);\n    let out = state_new(Sha384Digest);",
        "call": "sha384_oneshot(data, {len}, out);",
        "output_field": "out.data", "output_len": 48, "input_field": "data.data", "input_layout": "Sha384Data",
    },
    "sha512": {
        "import": 'import "/home/z/my-project/workdir/vuma/womb/crypto/hash/sha512.vuma"::{Sha512Data, Sha512Digest, sha512_oneshot};',
        "layouts": ["Sha512Data", "Sha512Digest"],
        "setup": "let data = state_new(Sha512Data);\n    let out = state_new(Sha512Digest);",
        "call": "sha512_oneshot(data, {len}, out);",
        "output_field": "out.data", "output_len": 64, "input_field": "data.data", "input_layout": "Sha512Data",
    },
    "sha3": {
        "import": 'import "/home/z/my-project/workdir/vuma/womb/crypto/hash/sha3.vuma"::{KeccakIn, KeccakOut, sha3_256_oneshot};',
        "layouts": ["KeccakIn", "KeccakOut"],
        "setup": "let data = state_new(KeccakIn);\n    let out = state_new(KeccakOut);",
        "call": "sha3_256_oneshot(data, {len}, out);",
        "output_field": "out.data", "output_len": 32, "input_field": "data.data", "input_layout": "KeccakIn",
    },
    "blake2": {
        "import": 'import "/home/z/my-project/workdir/vuma/womb/crypto/hash/blake2.vuma"::{Blake2bData, Blake2bDigest, blake2b_oneshot};',
        "layouts": ["Blake2bData", "Blake2bDigest"],
        "setup": "let data = state_new(Blake2bData);\n    let out = state_new(Blake2bDigest);",
        "call": "blake2b_oneshot(data, {len}, out);",
        "output_field": "out.data", "output_len": 64, "input_field": "data.data", "input_layout": "Blake2bData",
    },
    "blake3": {
        "import": 'import "/home/z/my-project/workdir/vuma/womb/crypto/hash/blake3.vuma"::{Blake3Data, Blake3Digest, blake3_oneshot};',
        "layouts": ["Blake3Data", "Blake3Digest"],
        "setup": "let data = state_new(Blake3Data);\n    let out = state_new(Blake3Digest);",
        "call": "blake3_oneshot(data, {len}, out);",
        "output_field": "out.data", "output_len": 32, "input_field": "data.data", "input_layout": "Blake3Data",
    },
    "md5": {
        "import": 'import "/home/z/my-project/workdir/vuma/womb/crypto/hash/md5.vuma"::{Md5Data, Md5Digest, md5_oneshot};',
        "layouts": ["Md5Data", "Md5Digest"],
        "setup": "let data = state_new(Md5Data);\n    let out = state_new(Md5Digest);",
        "call": "md5_oneshot(data, {len}, out);",
        "output_field": "out.data", "output_len": 16, "input_field": "data.data", "input_layout": "Md5Data",
    },
    # === Symmetric cipher modules ===
    "aes128": {
        "import": 'import "/home/z/my-project/workdir/vuma/womb/crypto/symmetric/aes128.vuma"::{AesCtx, AesKey, AesBlock, aes128_init, aes_encrypt_block};',
        "layouts": ["AesCtx", "AesKey", "AesBlock"],
        "setup": "let ctx = state_new(AesCtx);\n    let key = state_new(AesKey);\n    let pt = state_new(AesBlock);\n    let ct = state_new(AesBlock);",
        "call": "aes128_init(ctx, key);\n    aes_encrypt_block(ctx, pt, ct);",
        "output_field": "ct.data", "output_len": 16, "input_field": "pt.data", "input_layout": "AesBlock",
        "key_field": "key.data", "key_layout": "AesKey", "key_len": 16, "input_len": 16,
    },
    "aes192": {
        "import": 'import "/home/z/my-project/workdir/vuma/womb/crypto/symmetric/aes192.vuma"::{AesCtx, AesKey, AesBlock, aes192_init, aes_encrypt_block};',
        "layouts": ["AesCtx", "AesKey", "AesBlock"],
        "setup": "let ctx = state_new(AesCtx);\n    let key = state_new(AesKey);\n    let pt = state_new(AesBlock);\n    let ct = state_new(AesBlock);",
        "call": "aes192_init(ctx, key);\n    aes_encrypt_block(ctx, pt, ct);",
        "output_field": "ct.data", "output_len": 16, "input_field": "pt.data", "input_layout": "AesBlock",
        "key_field": "key.data", "key_layout": "AesKey", "key_len": 24, "input_len": 16,
    },
    "aes256": {
        "import": 'import "/home/z/my-project/workdir/vuma/womb/crypto/symmetric/aes256.vuma"::{AesCtx, AesKey, AesBlock, aes256_init, aes_encrypt_block};',
        "layouts": ["AesCtx", "AesKey", "AesBlock"],
        "setup": "let ctx = state_new(AesCtx);\n    let key = state_new(AesKey);\n    let pt = state_new(AesBlock);\n    let ct = state_new(AesBlock);",
        "call": "aes256_init(ctx, key);\n    aes_encrypt_block(ctx, pt, ct);",
        "output_field": "ct.data", "output_len": 16, "input_field": "pt.data", "input_layout": "AesBlock",
        "key_field": "key.data", "key_layout": "AesKey", "key_len": 32, "input_len": 16,
    },
    "des": {
        "import": 'import "/home/z/my-project/workdir/vuma/womb/crypto/symmetric/des.vuma"::{DesCtx, DesKey, DesBlock, des_init, des_encrypt_block};',
        "layouts": ["DesCtx", "DesKey", "DesBlock"],
        "setup": "let ctx = state_new(DesCtx);\n    let key = state_new(DesKey);\n    let pt = state_new(DesBlock);\n    let ct = state_new(DesBlock);",
        "call": "des_init(ctx, key);\n    des_encrypt_block(ctx, pt, ct);",
        "output_field": "ct.data", "output_len": 8, "input_field": "pt.data", "input_layout": "DesBlock",
        "key_field": "key.data", "key_layout": "DesKey", "key_len": 8, "input_len": 8,
    },
    "rc4": {
        "import": 'import "/home/z/my-project/workdir/vuma/womb/crypto/symmetric/rc4.vuma"::{Rc4Ctx, Rc4Key, Rc4Data, rc4_init, rc4_crypt};',
        "layouts": ["Rc4Ctx", "Rc4Key", "Rc4Data"],
        "setup": "let ctx = state_new(Rc4Ctx);\n    let key = state_new(Rc4Key);\n    let data = state_new(Rc4Data);",
        "call": "rc4_init(ctx, key, {keylen});\n    rc4_crypt(ctx, data, {len});",
        "output_field": "data.data", "output_len": 16, "input_field": "data.data", "input_layout": "Rc4Data",
        "key_field": "key.data", "key_layout": "Rc4Key", "key_len": 16, "input_len": 16,
    },
    "salsa20": {
        "import": 'import "/home/z/my-project/workdir/vuma/womb/crypto/symmetric/salsa20.vuma"::{Salsa20Ctx, Salsa20Key, Salsa20Nonce, Salsa20Buf, salsa20_init, salsa20_encrypt};',
        "layouts": ["Salsa20Ctx", "Salsa20Key", "Salsa20Nonce", "Salsa20Buf"],
        "setup": "let ctx = state_new(Salsa20Ctx);\n    let key = state_new(Salsa20Key);\n    let nonce = state_new(Salsa20Nonce);\n    let data = state_new(Salsa20Buf);",
        "call": "salsa20_init(ctx, key, nonce);\n    salsa20_encrypt(ctx, data, {len});",
        "output_field": "data.data", "output_len": 64, "input_field": "data.data", "input_layout": "Salsa20Buf",
        "key_field": "key.data", "key_layout": "Salsa20Key", "key_len": 32, "input_len": 64,
    },
    "chacha20": {
        "import": 'import "/home/z/my-project/workdir/vuma/womb/crypto/symmetric/chacha20.vuma"::{ChaCha20Ctx, ChaCha20Key, ChaCha20Nonce, ChaCha20Buf, chacha20_init, chacha20_encrypt};',
        "layouts": ["ChaCha20Ctx", "ChaCha20Key", "ChaCha20Nonce", "ChaCha20Buf"],
        "setup": "let ctx = state_new(ChaCha20Ctx);\n    let key = state_new(ChaCha20Key);\n    let nonce = state_new(ChaCha20Nonce);\n    let data = state_new(ChaCha20Buf);",
        "call": "chacha20_init(ctx, key, nonce);\n    chacha20_encrypt(ctx, data, {len});",
        "output_field": "data.data", "output_len": 64, "input_field": "data.data", "input_layout": "ChaCha20Buf",
        "key_field": "key.data", "key_layout": "ChaCha20Key", "key_len": 32, "input_len": 64,
    },
    "poly1305": {
        "import": 'import "/home/z/my-project/workdir/vuma/womb/crypto/symmetric/poly1305.vuma"::{Poly1305Ctx, Poly1305Key, Poly1305Msg, Poly1305Tag, poly1305_init, poly1305_update, poly1305_final};',
        "layouts": ["Poly1305Ctx", "Poly1305Key", "Poly1305Msg", "Poly1305Tag"],
        "setup": "let ctx = state_new(Poly1305Ctx);\n    let key = state_new(Poly1305Key);\n    let msg = state_new(Poly1305Msg);\n    let tag = state_new(Poly1305Tag);",
        "call": "poly1305_init(ctx, key);\n    poly1305_update(ctx, msg, {len});\n    poly1305_final(ctx, tag);",
        "output_field": "tag.data", "output_len": 16, "input_field": "msg.data", "input_layout": "Poly1305Msg",
        "key_field": "key.data", "key_layout": "Poly1305Key", "key_len": 32, "input_len": 16,
    },
    # === MAC/KDF modules ===
    "hmac": {
        "import": 'import "/home/z/my-project/workdir/vuma/womb/crypto/mac_kdf/hmac.vuma"::{HmacKey, HmacMsg, HmacOut, hmac_sha256};',
        "layouts": ["HmacKey", "HmacMsg", "HmacOut"],
        "setup": "let key = state_new(HmacKey);\n    let msg = state_new(HmacMsg);\n    let out = state_new(HmacOut);",
        "call": "hmac_sha256(key, {keylen}, msg, {len}, out);",
        "output_field": "out.data", "output_len": 32, "input_field": "msg.data", "input_layout": "HmacMsg",
        "key_field": "key.data", "key_layout": "HmacKey", "key_len": 32, "input_len": 32,
    },
    "hkdf": {
        "import": 'import "/home/z/my-project/workdir/vuma/womb/crypto/mac_kdf/hkdf.vuma"::{HmacKey, HmacMsg, HmacOut, hkdf_sha256};',
        "layouts": ["HmacKey", "HmacMsg", "HmacOut"],
        "setup": "let salt = state_new(HmacKey);\n    let ikm = state_new(HmacMsg);\n    let okm = state_new(HmacMsg);",
        "call": "hkdf_sha256(salt, 0, ikm, {len}, ikm, 0, okm, 32);",
        "output_field": "okm.data", "output_len": 32, "input_field": "ikm.data", "input_layout": "HmacMsg",
        "key_field": "ikm.data", "key_layout": "HmacMsg", "key_len": 32, "input_len": 32,
    },
    "pbkdf2": {
        "import": 'import "/home/z/my-project/workdir/vuma/womb/crypto/mac_kdf/pbkdf2.vuma"::{HmacKey, HmacMsg, pbkdf2_hmac_sha256};',
        "layouts": ["HmacKey", "HmacMsg"],
        "setup": "let pass = state_new(HmacKey);\n    let salt = state_new(HmacMsg);\n    let out = state_new(HmacMsg);",
        "call": "pbkdf2_hmac_sha256(pass, {keylen}, salt, {len}, 1000, out, 32);",
        "output_field": "out.data", "output_len": 32, "input_field": "salt.data", "input_layout": "HmacMsg",
        "key_field": "pass.data", "key_layout": "HmacKey", "key_len": 32, "input_len": 32,
    },
}

def hex_to_bytes(hex_str):
    """Convert hex string to list of ints."""
    if not hex_str:
        return []
    return [int(hex_str[i:i+2], 16) for i in range(0, len(hex_str), 2)]

def gen_harness(module_name, api, vectors):
    """Generate a VUMA harness for a module with 20 test vectors."""
    lines = []
    lines.append(f"// Auto-generated VUMA harness for {module_name}")
    lines.append(f"// Tests 20 vectors against C reference (OpenSSL)")
    lines.append(f"// Output: each result byte as print_int(byte+1000), delimiter print_int(999)")
    lines.append("")
    lines.append(api["import"])
    lines.append("")
    lines.append("transform main() -> i32 {")

    # State setup
    lines.append(f"    {api['setup']}")

    # For modules with key support, set up key
    has_key = "key_field" in api

    # Generate 20 vector tests
    for vi, vec in enumerate(vectors):
        lines.append(f"")
        lines.append(f"    // Vector {vi}: {vec.get('desc', '')}")

        # Set input data
        input_hex = vec.get("input_hex", "")
        input_bytes = hex_to_bytes(input_hex)
        input_field = api["input_field"]

        # Clear input buffer first (for shorter inputs)
        input_len = api.get("input_len", len(input_bytes))
        lines.append(f"    // Input: {input_hex[:32]}{'...' if len(input_hex) > 32 else ''}")

        # Set input bytes
        for bi, b in enumerate(input_bytes):
            lines.append(f"    {input_field}[{bi}] = {b};")

        # Set key if needed
        if has_key:
            key_hex = vec.get("key_hex", "")
            key_bytes = hex_to_bytes(key_hex)
            key_field = api["key_field"]
            key_len = api.get("key_len", len(key_bytes))
            lines.append(f"    // Key: {key_hex[:32]}{'...' if len(key_hex) > 32 else ''}")
            for bi, b in enumerate(key_bytes):
                lines.append(f"    {key_field}[{bi}] = {b};")

        # Call the function
        call = api["call"]
        call = call.replace("{len}", str(len(input_bytes)))
        call = call.replace("{keylen}", str(api.get("key_len", 0)))
        lines.append(f"    {call}")

        # Output result bytes
        output_field = api["output_field"]
        output_len = api["output_len"]
        lines.append(f"    // Output {output_len} bytes")
        lines.append(f"    let _oi: u32 = 0;")
        lines.append(f"    while _oi < {output_len} {{")
        lines.append(f"        print_int(({output_field}[_oi] as u32) + 1000);")
        lines.append(f"        _oi = _oi + 1;")
        lines.append(f"    }}")
        lines.append(f"    print_int(999); // vector delimiter")

    lines.append("    return 0;")
    lines.append("}")
    return "\n".join(lines)

def main():
    # Generate harnesses for modules with known APIs
    generated = 0
    skipped = []
    for module_name, api in MODULE_APIS.items():
        vec_path = f"{VECTORS_DIR}/{module_name}.json"
        if not os.path.exists(vec_path):
            skipped.append(f"{module_name} (no vectors)")
            continue
        with open(vec_path) as f:
            vec_data = json.load(f)
        vectors = vec_data["vectors"]
        harness = gen_harness(module_name, api, vectors)
        out_path = f"{HARNESS_DIR}/test_{module_name}_20vec.vuma"
        with open(out_path, "w") as f:
            f.write(harness)
        generated += 1
        print(f"  {module_name:<30} → {out_path}")

    # List modules without APIs (need manual harnesses)
    all_modules = []
    for f in os.listdir(VECTORS_DIR):
        if f.endswith(".json"):
            all_modules.append(f.replace(".json", ""))
    no_api = [m for m in all_modules if m not in MODULE_APIS]
    if no_api:
        print(f"\nModules needing manual harnesses ({len(no_api)}):")
        for m in no_api:
            print(f"  {m}")

    print(f"\nGenerated {generated} harnesses, skipped {len(skipped)}")

if __name__ == "__main__":
    main()
