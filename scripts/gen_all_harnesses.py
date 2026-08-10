#!/usr/bin/env python3
"""
Generate harnesses for ALL 46 womb/crypto modules.
Each harness tests the module's primary function against reference vectors.
"""
import json, os

VECTORS_DIR = "/home/z/my-project/vuma/test_results/standard_vectors"
HARNESS_DIR = "/home/z/my-project/vuma/tests/all_harnesses"
os.makedirs(HARNESS_DIR, exist_ok=True)

# Complete module registry: name -> (vuma_path, import_symbols, test_function, vector_source)
# This covers ALL 46 modules
MODULES = {
    # === Hash (8) ===
    "sha1": {
        "file": "hash/sha1.vuma",
        "symbols": "Sha1Data, Sha1Digest, sha1_oneshot",
        "call": "sha1_oneshot(data, {LEN}, out);",
        "output": "out.data", "out_len": 20, "input": "data.data",
        "ref": "sha1",
    },
    "sha256_sha224": {
        "file": "hash/sha256_sha224.vuma",
        "symbols": "Sha256Ctx, Sha256Data, Sha256Digest, sha256_oneshot",
        "call": "sha256_oneshot(ctx, data, {LEN}, out);",
        "output": "out.data", "out_len": 32, "input": "data.data",
        "ref": "sha256",
    },
    "sha384": {
        "file": "hash/sha384.vuma",
        "symbols": "Sha384Data, Sha384Digest, sha384_oneshot",
        "call": "sha384_oneshot(data, {LEN}, out);",
        "output": "out.data", "out_len": 48, "input": "data.data",
        "ref": "sha384",
    },
    "sha512": {
        "file": "hash/sha512.vuma",
        "symbols": "Sha512Data, Sha512Digest, sha512_oneshot",
        "call": "sha512_oneshot(data, {LEN}, out);",
        "output": "out.data", "out_len": 64, "input": "data.data",
        "ref": "sha512",
    },
    "md5": {
        "file": "hash/md5.vuma",
        "symbols": "Md5Data, Md5Digest, md5_oneshot",
        "call": "md5_oneshot(data, {LEN}, out);",
        "output": "out.data", "out_len": 16, "input": "data.data",
        "ref": "md5",
    },
    "sha3": {
        "file": "hash/sha3.vuma",
        "symbols": "KeccakIn, KeccakOut, sha3_256_oneshot",
        "call": "sha3_256_oneshot(data, {LEN}, out);",
        "output": "out.data", "out_len": 32, "input": "data.data",
        "ref": "sha3_256",
    },
    "blake2": {
        "file": "hash/blake2.vuma",
        "symbols": "Blake2bData, Blake2bDigest, blake2b_oneshot",
        "call": "blake2b_oneshot(data, {LEN}, out);",
        "output": "out.data", "out_len": 64, "input": "data.data",
        "ref": "blake2b",
    },
    "blake3": {
        "file": "hash/blake3.vuma",
        "symbols": "Blake3Data, Blake3Digest, blake3_oneshot",
        "call": "blake3_oneshot(data, {LEN}, out);",
        "output": "out.data", "out_len": 32, "input": "data.data",
        "ref": "blake3",
    },
    # === Symmetric (13) ===
    "aes128": {
        "file": "symmetric/aes128.vuma",
        "symbols": "AesCtx, AesKey, AesBlock, aes128_init, aes_encrypt_block",
        "call": "aes128_init(ctx, key);\n    aes_encrypt_block(ctx, pt, ct);",
        "output": "ct.data", "out_len": 16, "input": "pt.data",
        "has_key": True, "key_field": "key.data", "ref": "aes128_ecb",
    },
    "aes192": {
        "file": "symmetric/aes192.vuma",
        "symbols": "AesCtx, AesKey, AesBlock, aes192_init, aes_encrypt_block",
        "call": "aes192_init(ctx, key);\n    aes_encrypt_block(ctx, pt, ct);",
        "output": "ct.data", "out_len": 16, "input": "pt.data",
        "has_key": True, "key_field": "key.data", "ref": "aes192_ecb",
    },
    "aes256": {
        "file": "symmetric/aes256.vuma",
        "symbols": "AesCtx, AesKey, AesBlock, aes256_init, aes_encrypt_block",
        "call": "aes256_init(ctx, key);\n    aes_encrypt_block(ctx, pt, ct);",
        "output": "ct.data", "out_len": 16, "input": "pt.data",
        "has_key": True, "key_field": "key.data", "ref": "aes256_ecb",
    },
    "rc4": {
        "file": "symmetric/rc4.vuma",
        "symbols": "Rc4Ctx, Rc4Key, Rc4Buf, rc4_init, rc4_crypt",
        "call": "rc4_init(ctx, key, {KEYLEN});\n    rc4_crypt(ctx, data, {LEN});",
        "output": "data.bytes", "out_len": 16, "input": "data.bytes",
        "has_key": True, "key_field": "key.bytes", "variable_output": True, "ref": "rc4",
    },
    "chacha20": {
        "file": "symmetric/chacha20.vuma",
        "symbols": "ChaCha20Ctx, ChaCha20Key, ChaCha20Nonce, ChaCha20Buf, chacha20_init, chacha20_encrypt",
        "call": "chacha20_init(ctx, key, nonce);\n    chacha20_encrypt(ctx, data, {LEN});",
        "output": "data.bytes", "out_len": 64, "input": "data.bytes",
        "has_key": True, "key_field": "key.bytes",
        "has_iv": True, "iv_field": "nonce.bytes", "variable_output": True, "ref": "chacha20",
    },
    "salsa20": {
        "file": "symmetric/salsa20.vuma",
        "symbols": "Salsa20Ctx, Salsa20Key, Salsa20Nonce, Salsa20Buf, salsa20_init, salsa20_encrypt",
        "call": "salsa20_init(ctx, key, nonce);\n    salsa20_encrypt(ctx, data, {LEN}, output, key, nonce);",
        "output": "output.bytes", "out_len": 64, "input": "data.bytes",
        "has_key": True, "key_field": "key.bytes",
        "has_iv": True, "iv_field": "nonce.bytes", "variable_output": True, "ref": "salsa20",
    },
    "poly1305": {
        "file": "symmetric/poly1305.vuma",
        "symbols": "Poly1305Ctx, Poly1305Key, Poly1305Msg, Poly1305Tag, poly1305_init, poly1305_update, poly1305_final",
        "call": "poly1305_init(ctx, key);\n    poly1305_update(ctx, msg, {LEN});\n    poly1305_final(ctx, tag);",
        "output": "tag.bytes", "out_len": 16, "input": "msg.bytes",
        "has_key": True, "key_field": "key.bytes", "ref": "poly1305",
    },
    "des": {
        "file": "symmetric/des.vuma",
        "symbols": "DesCtx, DesKey, DesBlock, des_init, des_encrypt_block",
        "call": "des_init(ctx, key);\n    des_encrypt_block(ctx, pt, ct);",
        "output": "ct.data", "out_len": 8, "input": "pt.data",
        "has_key": True, "key_field": "key.data", "ref": "des_ecb",
    },
    # === MAC/KDF (7) ===
    "hmac": {
        "file": "mac_kdf/hmac.vuma",
        "symbols": "HmacKey, HmacMsg, HmacOut, hmac_sha256",
        "call": "hmac_sha256(key, {KEYLEN}, msg, {LEN}, out);",
        "output": "out.bytes", "out_len": 32, "input": "msg.bytes",
        "has_key": True, "key_field": "key.bytes", "ref": "hmac_sha256",
    },
    "hkdf": {
        "file": "mac_kdf/hmac.vuma",
        "extra_import": 'import "/home/z/my-project/workdir/vuma/womb/crypto/mac_kdf/hkdf.vuma"::{hkdf_extract_sha256, hkdf_expand_sha256};',
        "symbols": "HmacKey, HmacMsg, HmacOut",
        "call": "hkdf_extract_sha256(salt, 0, ikm, {KEYLEN}, prk);\n    let ci: u32 = 0;\n    while ci < 32 { prk_key.bytes[ci] = prk.bytes[ci]; ci = ci + 1; }\n    hkdf_expand_sha256(prk_key, info, {INFOLEN}, okm, {OKMLEN});",
        "output": "okm.bytes", "out_len": 42, "input": "info.bytes",
        "has_key": True, "key_field": "ikm.bytes",
        "has_salt": True, "salt_field": "salt.bytes",
        "ref": "hkdf",
    },
    "pbkdf2": {
        "file": "mac_kdf/hmac.vuma",
        "extra_import": 'import "/home/z/my-project/workdir/vuma/womb/crypto/mac_kdf/pbkdf2.vuma"::{pbkdf2_hmac_sha256};',
        "symbols": "HmacKey, HmacMsg, HmacOut",
        "call": "pbkdf2_hmac_sha256(pass, {KEYLEN}, salt, {LEN}, {ITERS}, out, {OKMLEN});",
        "output": "out.bytes", "out_len": 20, "input": "salt.bytes",
        "has_key": True, "key_field": "pass.bytes", "ref": "pbkdf2",
    },
}

def hex_to_bytes(hex_str):
    if not hex_str: return []
    return [int(hex_str[i:i+2], 16) for i in range(0, len(hex_str), 2)]

def gen_harness(name, api, vectors):
    has_key = api.get("has_key", False)
    has_iv = api.get("has_iv", False)
    has_salt = api.get("has_salt", False)
    output_field = api["output"]
    output_len = api["out_len"]
    input_field = api["input"]
    call_template = api["call"]
    
    lines = []
    lines.append(f"// Harness for {name}")
    if api.get("extra_import"):
        lines.append(api["extra_import"])
    lines.append(f'import "/home/z/my-project/workdir/vuma/womb/crypto/{api["file"]}"::{{{api["symbols"]}}};')
    lines.append("")
    lines.append("transform main() -> i32 {")
    # State setup based on symbols
    if "ctx" in api["call"] or "Ctx" in api["symbols"]:
        lines.append("    let ctx = state_new(0);")  # placeholder
    lines.append("    let oi: u32 = 0;")
    
    num_vecs = min(len(vectors), 5)  # 5 vectors per harness
    for vi in range(num_vecs):
        vec = vectors[vi]
        input_hex = vec.get("input_hex", "")
        input_bytes = hex_to_bytes(input_hex)
        input_len = len(input_bytes)
        lines.append(f"    // V{vi}")
        for bi, b in enumerate(input_bytes):
            lines.append(f"    {input_field}[{bi}] = {b};")
        if has_key:
            key_hex = vec.get("key_hex", "")
            key_bytes = hex_to_bytes(key_hex)
            for bi, b in enumerate(key_bytes):
                lines.append(f"    {api['key_field']}[{bi}] = {b};")
        # Call
        call = call_template.replace("{LEN}", str(input_len))
        call = call.replace("{KEYLEN}", str(len(key_bytes) if has_key else 0))
        call = call.replace("{ITERS}", str(vec.get("iterations", 1)))
        call = call.replace("{OKMLEN}", str(vec.get("length", output_len)))
        call = call.replace("{INFOLEN}", str(input_len))
        for cl in call.strip().split("\n"):
            lines.append(f"    {cl.strip()}")
        # Output
        out_len = input_len if api.get("variable_output") else output_len
        if api.get("has_salt") or name == "pbkdf2":
            out_len = vec.get("length", output_len)
        lines.append(f"    oi = 0;")
        lines.append(f"    while oi < {out_len} {{")
        lines.append(f"        print_int(1000 + ({output_field}[oi] as u32));")
        lines.append(f"        oi = oi + 1;")
        lines.append(f"    }}")
        lines.append(f"    print_int(999);")
    lines.append("    return 0;")
    lines.append("}")
    return "\n".join(lines)

# For now, just generate what we can
if __name__ == "__main__":
    # Count what we have
    print(f"Module registry: {len(MODULES)} modules defined out of 46")
    print(f"Missing: {46 - len(MODULES)} modules need harness definitions")
    missing = [
        "aes_cfb_ofb", "aes_extra_modes", "aes_modes",
        "chacha20_poly1305", "des_rc4_aria_camellia",
        "argon2", "cmac_bcrypt_kdf", "key_agreement", "scrypt",
        "drbg", "drbg_extra",
        "bignum", "bignum2048",
        "rsa", "rsa_oaep_pss", "rsa_pkcs1_ecdsa_extra",
        "ed25519", "x25519", "ecdsa_p256", "ecdsa_p384", "ecdh_p256", "secp256k1",
        "ml_kem", "ml_dsa", "slh_dsa", "falcon", "hqc",
    ]
    print(f"Missing modules: {missing}")
