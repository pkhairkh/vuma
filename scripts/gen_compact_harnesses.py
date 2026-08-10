#!/usr/bin/env python3
"""
Generate compact VUMA test harnesses — 5 vectors per harness.
Multiple harnesses per module to avoid compile timeout.
"""
import json, os

VECTORS_DIR = "/home/z/my-project/vuma/test_results/standard_vectors"
HARNESS_DIR = "/home/z/my-project/vuma/tests/compact_harnesses"
os.makedirs(HARNESS_DIR, exist_ok=True)

VECS_PER_HARNESS = 5

MODULE_APIS = {
    "sha1": {
        "import": 'import "/home/z/my-project/workdir/vuma/womb/crypto/hash/sha1.vuma"::{Sha1Data, Sha1Digest, sha1_oneshot};',
        "state_setup": "let data = state_new(Sha1Data);\n    let out = state_new(Sha1Digest);",
        "call": "sha1_oneshot(data, {LEN}, out);",
        "output_field": "out.data", "output_len": 20,
        "input_field": "data.data",
    },
    "sha256_sha224": {
        "import": 'import "/home/z/my-project/workdir/vuma/womb/crypto/hash/sha256_sha224.vuma"::{Sha256Ctx, Sha256Data, Sha256Digest, sha256_oneshot};',
        "state_setup": "let ctx = state_new(Sha256Ctx);\n    let data = state_new(Sha256Data);\n    let out = state_new(Sha256Digest);",
        "call": "sha256_oneshot(ctx, data, {LEN}, out);",
        "output_field": "out.data", "output_len": 32,
        "input_field": "data.data",
    },
    "sha384": {
        "import": 'import "/home/z/my-project/workdir/vuma/womb/crypto/hash/sha384.vuma"::{Sha384Data, Sha384Digest, sha384_oneshot};',
        "state_setup": "let data = state_new(Sha384Data);\n    let out = state_new(Sha384Digest);",
        "call": "sha384_oneshot(data, {LEN}, out);",
        "output_field": "out.data", "output_len": 48,
        "input_field": "data.data",
    },
    "sha512": {
        "import": 'import "/home/z/my-project/workdir/vuma/womb/crypto/hash/sha512.vuma"::{Sha512Data, Sha512Digest, sha512_oneshot};',
        "state_setup": "let data = state_new(Sha512Data);\n    let out = state_new(Sha512Digest);",
        "call": "sha512_oneshot(data, {LEN}, out);",
        "output_field": "out.data", "output_len": 64,
        "input_field": "data.data",
    },
    "md5": {
        "import": 'import "/home/z/my-project/workdir/vuma/womb/crypto/hash/md5.vuma"::{Md5Data, Md5Digest, md5_oneshot};',
        "state_setup": "let data = state_new(Md5Data);\n    let out = state_new(Md5Digest);",
        "call": "md5_oneshot(data, {LEN}, out);",
        "output_field": "out.data", "output_len": 16,
        "input_field": "data.data",
    },
    "sha3": {
        "import": 'import "/home/z/my-project/workdir/vuma/womb/crypto/hash/sha3.vuma"::{KeccakIn, KeccakOut, sha3_256_oneshot};',
        "state_setup": "let data = state_new(KeccakIn);\n    let out = state_new(KeccakOut);",
        "call": "sha3_256_oneshot(data, {LEN}, out);",
        "output_field": "out.data", "output_len": 32,
        "input_field": "data.data",
    },
    "blake2": {
        "import": 'import "/home/z/my-project/workdir/vuma/womb/crypto/hash/blake2.vuma"::{Blake2bData, Blake2bDigest, blake2b_oneshot};',
        "state_setup": "let data = state_new(Blake2bData);\n    let out = state_new(Blake2bDigest);",
        "call": "blake2b_oneshot(data, {LEN}, out);",
        "output_field": "out.data", "output_len": 64,
        "input_field": "data.data",
    },
    "blake3": {
        "import": 'import "/home/z/my-project/workdir/vuma/womb/crypto/hash/blake3.vuma"::{Blake3Data, Blake3Digest, blake3_oneshot};',
        "state_setup": "let data = state_new(Blake3Data);\n    let out = state_new(Blake3Digest);",
        "call": "blake3_oneshot(data, {LEN}, out);",
        "output_field": "out.data", "output_len": 32,
        "input_field": "data.data",
    },
    "aes128": {
        "import": 'import "/home/z/my-project/workdir/vuma/womb/crypto/symmetric/aes128.vuma"::{AesCtx, AesKey, AesBlock, aes128_init, aes_encrypt_block};',
        "state_setup": "let ctx = state_new(AesCtx);\n    let key = state_new(AesKey);\n    let pt = state_new(AesBlock);\n    let ct = state_new(AesBlock);",
        "call": "aes128_init(ctx, key);\n    aes_encrypt_block(ctx, pt, ct);",
        "output_field": "ct.data", "output_len": 16,
        "input_field": "pt.data",
        "has_key": True, "key_field": "key.data", "key_len": 16,
    },
    "aes192": {
        "import": 'import "/home/z/my-project/workdir/vuma/womb/crypto/symmetric/aes192.vuma"::{AesCtx, AesKey, AesBlock, aes192_init, aes_encrypt_block};',
        "state_setup": "let ctx = state_new(AesCtx);\n    let key = state_new(AesKey);\n    let pt = state_new(AesBlock);\n    let ct = state_new(AesBlock);",
        "call": "aes192_init(ctx, key);\n    aes_encrypt_block(ctx, pt, ct);",
        "output_field": "ct.data", "output_len": 16,
        "input_field": "pt.data",
        "has_key": True, "key_field": "key.data", "key_len": 24,
    },
    "aes256": {
        "import": 'import "/home/z/my-project/workdir/vuma/womb/crypto/symmetric/aes256.vuma"::{AesCtx, AesKey, AesBlock, aes256_init, aes_encrypt_block};',
        "state_setup": "let ctx = state_new(AesCtx);\n    let key = state_new(AesKey);\n    let pt = state_new(AesBlock);\n    let ct = state_new(AesBlock);",
        "call": "aes256_init(ctx, key);\n    aes_encrypt_block(ctx, pt, ct);",
        "output_field": "ct.data", "output_len": 16,
        "input_field": "pt.data",
        "has_key": True, "key_field": "key.data", "key_len": 32,
    },
    "des": {
        "import": 'import "/home/z/my-project/workdir/vuma/womb/crypto/symmetric/des.vuma"::{DesCtx, DesKey, DesBlock, des_init, des_encrypt_block};',
        "state_setup": "let ctx = state_new(DesCtx);\n    let key = state_new(DesKey);\n    let pt = state_new(DesBlock);\n    let ct = state_new(DesBlock);",
        "call": "des_init(ctx, key);\n    des_encrypt_block(ctx, pt, ct);",
        "output_field": "ct.data", "output_len": 8,
        "input_field": "pt.data",
        "has_key": True, "key_field": "key.data", "key_len": 8,
    },
    "rc4": {
        "import": 'import "/home/z/my-project/workdir/vuma/womb/crypto/symmetric/rc4.vuma"::{Rc4Ctx, Rc4Key, Rc4Buf, rc4_init, rc4_crypt};',
        "state_setup": "let ctx = state_new(Rc4Ctx);\n    let key = state_new(Rc4Key);\n    let data = state_new(Rc4Buf);",
        "call": "rc4_init(ctx, key, {KEYLEN});\n    rc4_crypt(ctx, data, {LEN});",
        "output_field": "data.bytes", "output_len": 16,
        "input_field": "data.bytes",
        "has_key": True, "key_field": "key.bytes", "key_len": 16,
        "variable_output": True,
    },
    "chacha20": {
        "import": 'import "/home/z/my-project/workdir/vuma/womb/crypto/symmetric/chacha20.vuma"::{ChaCha20Ctx, ChaCha20Key, ChaCha20Nonce, ChaCha20Buf, chacha20_init, chacha20_set_counter, chacha20_encrypt};',
        "state_setup": "let ctx = state_new(ChaCha20Ctx);\n    let key = state_new(ChaCha20Key);\n    let nonce = state_new(ChaCha20Nonce);\n    let data = state_new(ChaCha20Buf);",
        "call": "chacha20_init(ctx, key, nonce);\n    chacha20_encrypt(ctx, data, {LEN});",
        "output_field": "data.bytes", "output_len": 64,
        "input_field": "data.bytes",
        "has_key": True, "key_field": "key.bytes", "key_len": 32,
        "has_iv": True, "iv_field": "nonce.bytes", "iv_len": 12,
        "variable_output": True,
    },
    "salsa20": {
        "import": 'import "/home/z/my-project/workdir/vuma/womb/crypto/symmetric/salsa20.vuma"::{Salsa20Ctx, Salsa20Key, Salsa20Nonce, Salsa20Buf, salsa20_init, salsa20_encrypt};',
        "state_setup": "let ctx = state_new(Salsa20Ctx);\n    let key = state_new(Salsa20Key);\n    let nonce = state_new(Salsa20Nonce);\n    let data = state_new(Salsa20Buf);",
        "call": "salsa20_init(ctx, key, nonce);\n    salsa20_encrypt(ctx, data, {LEN});",
        "output_field": "data.bytes", "output_len": 64,
        "input_field": "data.bytes",
        "has_key": True, "key_field": "key.bytes", "key_len": 32,
        "has_iv": True, "iv_field": "nonce.bytes", "iv_len": 8,
        "variable_output": True,
    },
    "poly1305": {
        "import": 'import "/home/z/my-project/workdir/vuma/womb/crypto/symmetric/poly1305.vuma"::{Poly1305Ctx, Poly1305Key, Poly1305Msg, Poly1305Tag, poly1305_init, poly1305_update, poly1305_final};',
        "state_setup": "let ctx = state_new(Poly1305Ctx);\n    let key = state_new(Poly1305Key);\n    let msg = state_new(Poly1305Msg);\n    let tag = state_new(Poly1305Tag);",
        "call": "poly1305_init(ctx, key);\n    poly1305_update(ctx, msg, {LEN});\n    poly1305_final(ctx, tag);",
        "output_field": "tag.data", "output_len": 16,
        "input_field": "msg.data",
        "has_key": True, "key_field": "key.data", "key_len": 32,
    },
    "hmac": {
        "import": 'import "/home/z/my-project/workdir/vuma/womb/crypto/mac_kdf/hmac.vuma"::{HmacKey, HmacMsg, HmacOut, hmac_sha256};',
        "state_setup": "let key = state_new(HmacKey);\n    let msg = state_new(HmacMsg);\n    let out = state_new(HmacOut);",
        "call": "hmac_sha256(key, {KEYLEN}, msg, {LEN}, out);",
        "output_field": "out.bytes", "output_len": 32,
        "input_field": "msg.bytes",
        "has_key": True, "key_field": "key.bytes", "key_len": 32,
    },
    "hkdf": {
        "import": 'import "/home/z/my-project/workdir/vuma/womb/crypto/mac_kdf/hmac.vuma"::{HmacKey, HmacMsg, HmacOut};\nimport "/home/z/my-project/workdir/vuma/womb/crypto/mac_kdf/hkdf.vuma"::{hkdf_sha256};',
        "state_setup": "let salt = state_new(HmacKey);\n    let ikm = state_new(HmacMsg);\n    let info = state_new(HmacMsg);\n    let okm = state_new(HmacMsg);",
        "call": "hkdf_sha256(salt, 0, ikm, {KEYLEN}, info, 0, okm, 32);",
        "output_field": "okm.bytes", "output_len": 32,
        "input_field": "ikm.bytes",
        "has_key": True, "key_field": "ikm.bytes", "key_len": 32,
    },
    "pbkdf2": {
        "import": 'import "/home/z/my-project/workdir/vuma/womb/crypto/mac_kdf/hmac.vuma"::{HmacKey, HmacMsg, HmacOut};\nimport "/home/z/my-project/workdir/vuma/womb/crypto/mac_kdf/pbkdf2.vuma"::{pbkdf2_hmac_sha256};',
        "state_setup": "let pass = state_new(HmacKey);\n    let salt = state_new(HmacMsg);\n    let out = state_new(HmacMsg);",
        "call": "pbkdf2_hmac_sha256(pass, {KEYLEN}, salt, {LEN}, 1000, out, 32);",
        "output_field": "out.bytes", "output_len": 32,
        "input_field": "salt.bytes",
        "has_key": True, "key_field": "pass.bytes", "key_len": 32,
    },
}

def hex_to_bytes(hex_str):
    if not hex_str: return []
    return [int(hex_str[i:i+2], 16) for i in range(0, len(hex_str), 2)]

def gen_harness(module_name, api, vectors, batch_idx):
    has_key = api.get("has_key", False)
    has_iv = api.get("has_iv", False)
    output_len = api["output_len"]
    input_field = api["input_field"]
    output_field = api["output_field"]
    key_field = api.get("key_field", "")
    iv_field = api.get("iv_field", "")
    key_len_val = api.get("key_len", 0)
    call_template = api["call"]

    lines = []
    lines.append(f"// {module_name} batch {batch_idx} (vectors {batch_idx*VECS_PER_HARNESS}-{batch_idx*VECS_PER_HARNESS+len(vectors)-1})")
    lines.append(api["import"])
    lines.append("")
    lines.append("transform main() -> i32 {")
    for state_line in api["state_setup"].split("\n"):
        state_line = state_line.strip()
        if state_line: lines.append(f"    {state_line}")
    lines.append(f"    let oi: u32 = 0;")

    for vi, vec in enumerate(vectors):
        input_hex = vec.get("input_hex", "")
        input_bytes = hex_to_bytes(input_hex)
        input_len = len(input_bytes)
        lines.append(f"    // V{batch_idx*VECS_PER_HARNESS+vi}: {vec.get('desc','')}")
        for bi, b in enumerate(input_bytes):
            lines.append(f"    {input_field}[{bi}] = {b};")
        if has_key:
            key_hex = vec.get("key_hex", "")
            key_bytes = hex_to_bytes(key_hex)
            for bi, b in enumerate(key_bytes):
                lines.append(f"    {key_field}[{bi}] = {b};")
        if has_iv:
            iv_hex = vec.get("iv_hex", "")
            iv_bytes = hex_to_bytes(iv_hex)
            for bi, b in enumerate(iv_bytes):
                lines.append(f"    {iv_field}[{bi}] = {b};")
        # Use actual key length from vector, not fixed API value
        actual_key_len = len(key_bytes) if has_key else key_len_val
        call = call_template.replace("{LEN}", str(input_len)).replace("{KEYLEN}", str(actual_key_len))
        for cl in call.strip().split("\n"):
            lines.append(f"    {cl.strip()}")
        # Output: for variable_output modules, output input_len bytes; else output_len
        out_len = input_len if api.get("variable_output", False) else output_len
        lines.append(f"    oi = 0;")
        lines.append(f"    while oi < {out_len} {{")
        lines.append(f"        print_int(1000 + ({output_field}[oi] as u32));")
        lines.append(f"        oi = oi + 1;")
        lines.append(f"    }}")
        lines.append(f"    print_int(999);")
    lines.append("    return 0;")
    lines.append("}")
    return "\n".join(lines)

def main():
    for module_name, api in MODULE_APIS.items():
        vec_path = f"{VECTORS_DIR}/{module_name}.json"
        if not os.path.exists(vec_path): continue
        with open(vec_path) as f: vec_data = json.load(f)
        vectors = vec_data["vectors"]
        num_batches = (len(vectors) + VECS_PER_HARNESS - 1) // VECS_PER_HARNESS
        for bi in range(num_batches):
            batch = vectors[bi*VECS_PER_HARNESS:(bi+1)*VECS_PER_HARNESS]
            harness = gen_harness(module_name, api, batch, bi)
            out_path = f"{HARNESS_DIR}/test_{module_name}_b{bi}.vuma"
            with open(out_path, "w") as f: f.write(harness)
        print(f"  {module_name}: {num_batches} harnesses")

if __name__ == "__main__":
    main()
