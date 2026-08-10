#!/usr/bin/env python3
"""
Generate VUMA test harnesses v4 — reuse state variables across vectors.
Each vector overwrites the input data in the same state objects.
Avoids arena overflow from too many state_new allocations.
"""
import json, os

VECTORS_DIR = "/home/z/my-project/vuma/test_results/vectors"
HARNESS_DIR = "/home/z/my-project/vuma/tests/full_validation"
os.makedirs(HARNESS_DIR, exist_ok=True)

NUM_VECTORS = 20
MAX_INPUT = 256  # max input bytes per vector

MODULE_APIS = {
    "sha1": {
        "import": 'import "/home/z/my-project/workdir/vuma/womb/crypto/hash/sha1.vuma"::{Sha1Data, Sha1Digest, sha1_oneshot};',
        "state_setup": "let data = state_new(Sha1Data);\n    let out = state_new(Sha1Digest);",
        "call": "sha1_oneshot(data, {LEN}, out);",
        "output_field": "out.data", "output_len": 20,
        "input_field": "data.bytes",
    },
    "sha256_sha224": {
        "import": 'import "/home/z/my-project/workdir/vuma/womb/crypto/hash/sha256_sha224.vuma"::{Sha256Ctx, Sha256Data, Sha256Digest, sha256_oneshot};',
        "state_setup": "let ctx = state_new(Sha256Ctx);\n    let data = state_new(Sha256Data);\n    let out = state_new(Sha256Digest);",
        "call": "sha256_oneshot(ctx, data, {LEN}, out);",
        "output_field": "out.data", "output_len": 32,
        "input_field": "data.bytes",
    },
    "sha384": {
        "import": 'import "/home/z/my-project/workdir/vuma/womb/crypto/hash/sha384.vuma"::{Sha384Data, Sha384Digest, sha384_oneshot};',
        "state_setup": "let data = state_new(Sha384Data);\n    let out = state_new(Sha384Digest);",
        "call": "sha384_oneshot(data, {LEN}, out);",
        "output_field": "out.data", "output_len": 48,
        "input_field": "data.bytes",
    },
    "sha512": {
        "import": 'import "/home/z/my-project/workdir/vuma/womb/crypto/hash/sha512.vuma"::{Sha512Data, Sha512Digest, sha512_oneshot};',
        "state_setup": "let data = state_new(Sha512Data);\n    let out = state_new(Sha512Digest);",
        "call": "sha512_oneshot(data, {LEN}, out);",
        "output_field": "out.data", "output_len": 64,
        "input_field": "data.bytes",
    },
    "md5": {
        "import": 'import "/home/z/my-project/workdir/vuma/womb/crypto/hash/md5.vuma"::{Md5Data, Md5Digest, md5_oneshot};',
        "state_setup": "let data = state_new(Md5Data);\n    let out = state_new(Md5Digest);",
        "call": "md5_oneshot(data, {LEN}, out);",
        "output_field": "out.data", "output_len": 16,
        "input_field": "data.bytes",
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
        "import": 'import "/home/z/my-project/workdir/vuma/womb/crypto/symmetric/rc4.vuma"::{Rc4Ctx, Rc4Key, Rc4Buf, rc4_init, rc4_crypt};',
        "state_setup": "let ctx = state_new(Rc4Ctx);\n    let key = state_new(Rc4Key);\n    let data = state_new(Rc4Buf);",
        "call": "rc4_init(ctx, key, {KEYLEN});\n    rc4_crypt(ctx, data, {LEN});",
        "output_field": "data.bytes", "output_len": 16,
        "input_field": "data.bytes", "has_key": True, "key_field": "key.data", "key_len": 16,
    },
    "hmac": {
        "import": 'import "/home/z/my-project/workdir/vuma/womb/crypto/mac_kdf/hmac.vuma"::{HmacKey, HmacMsg, HmacOut, hmac_sha256};',
        "state_setup": "let key = state_new(HmacKey);\n    let msg = state_new(HmacMsg);\n    let out = state_new(HmacOut);",
        "call": "hmac_sha256(key, {KEYLEN}, msg, {LEN}, out);",
        "output_field": "out.data", "output_len": 32,
        "input_field": "msg.data", "has_key": True, "key_field": "key.data", "key_len": 32,
    },
    # Additional hash modules
    "sha3": {
        "import": 'import "/home/z/my-project/workdir/vuma/womb/crypto/hash/sha3.vuma"::{KeccakIn, KeccakOut, sha3_256_oneshot};',
        "state_setup": "let data = state_new(KeccakIn);\n    let out = state_new(KeccakOut);",
        "call": "sha3_256_oneshot(data, {LEN}, out);",
        "output_field": "out.data", "output_len": 32,
        "input_field": "data.bytes",
    },
    "blake2": {
        "import": 'import "/home/z/my-project/workdir/vuma/womb/crypto/hash/blake2.vuma"::{Blake2bData, Blake2bDigest, blake2b_oneshot};',
        "state_setup": "let data = state_new(Blake2bData);\n    let out = state_new(Blake2bDigest);",
        "call": "blake2b_oneshot(data, {LEN}, out);",
        "output_field": "out.data", "output_len": 64,
        "input_field": "data.bytes",
    },
    "blake3": {
        "import": 'import "/home/z/my-project/workdir/vuma/womb/crypto/hash/blake3.vuma"::{Blake3Data, Blake3Digest, blake3_oneshot};',
        "state_setup": "let data = state_new(Blake3Data);\n    let out = state_new(Blake3Digest);",
        "call": "blake3_oneshot(data, {LEN}, out);",
        "output_field": "out.data", "output_len": 32,
        "input_field": "data.bytes",
    },
    # Additional symmetric modules
    "salsa20": {
        "import": 'import "/home/z/my-project/workdir/vuma/womb/crypto/symmetric/salsa20.vuma"::{Salsa20Ctx, Salsa20Key, Salsa20Nonce, Salsa20Buf, salsa20_init, salsa20_encrypt};',
        "state_setup": "let ctx = state_new(Salsa20Ctx);\n    let key = state_new(Salsa20Key);\n    let nonce = state_new(Salsa20Nonce);\n    let data = state_new(Salsa20Buf);",
        "call": "salsa20_init(ctx, key, nonce);\n    salsa20_encrypt(ctx, data, {LEN});",
        "output_field": "data.bytes", "output_len": 64,
        "input_field": "data.bytes", "has_key": True, "key_field": "key.data", "key_len": 32,
    },
    "chacha20": {
        "import": 'import "/home/z/my-project/workdir/vuma/womb/crypto/symmetric/chacha20.vuma"::{ChaCha20Ctx, ChaCha20Key, ChaCha20Nonce, ChaCha20Buf, chacha20_init, chacha20_encrypt};',
        "state_setup": "let ctx = state_new(ChaCha20Ctx);\n    let key = state_new(ChaCha20Key);\n    let nonce = state_new(ChaCha20Nonce);\n    let data = state_new(ChaCha20Buf);",
        "call": "chacha20_init(ctx, key, nonce);\n    chacha20_encrypt(ctx, data, {LEN});",
        "output_field": "data.bytes", "output_len": 64,
        "input_field": "data.bytes", "has_key": True, "key_field": "key.data", "key_len": 32,
    },
    "poly1305": {
        "import": 'import "/home/z/my-project/workdir/vuma/womb/crypto/symmetric/poly1305.vuma"::{Poly1305Ctx, Poly1305Key, Poly1305Msg, Poly1305Tag, poly1305_init, poly1305_update, poly1305_final};',
        "state_setup": "let ctx = state_new(Poly1305Ctx);\n    let key = state_new(Poly1305Key);\n    let msg = state_new(Poly1305Msg);\n    let tag = state_new(Poly1305Tag);",
        "call": "poly1305_init(ctx, key);\n    poly1305_update(ctx, msg, {LEN});\n    poly1305_final(ctx, tag);",
        "output_field": "tag.data", "output_len": 16,
        "input_field": "msg.data", "has_key": True, "key_field": "key.data", "key_len": 32,
    },
    # Additional MAC/KDF modules
    "hkdf": {
        "import": 'import "/home/z/my-project/workdir/vuma/womb/crypto/mac_kdf/hkdf.vuma"::{HmacKey, HmacMsg, HmacOut, hkdf_sha256};',
        "state_setup": "let salt = state_new(HmacKey);\n    let ikm = state_new(HmacMsg);\n    let okm = state_new(HmacMsg);",
        "call": "hkdf_sha256(salt, 0, ikm, {LEN}, ikm, 0, okm, 32);",
        "output_field": "okm.data", "output_len": 32,
        "input_field": "ikm.data", "has_key": False,
    },
    "pbkdf2": {
        "import": 'import "/home/z/my-project/workdir/vuma/womb/crypto/mac_kdf/pbkdf2.vuma"::{HmacKey, HmacMsg, pbkdf2_hmac_sha256};\nimport "/home/z/my-project/workdir/vuma/womb/crypto/mac_kdf/hmac.vuma"::{HmacKey, HmacMsg, HmacOut};',
        "state_setup": "let pass = state_new(HmacKey);\n    let salt = state_new(HmacMsg);\n    let out = state_new(HmacMsg);",
        "call": "pbkdf2_hmac_sha256(pass, 32, salt, {LEN}, 1000, out, 32);",
        "output_field": "out.bytes", "output_len": 32,
        "input_field": "salt.bytes", "has_key": True, "key_field": "pass.bytes", "key_len": 32,
    },
    # ─── Symmetric (5) ───────────────────────────────────────────────────────
    "aes_cfb_ofb": {
        "import": 'import "/home/z/my-project/workdir/vuma/womb/crypto/symmetric/aes128.vuma"::{AesCtx, AesKey, aes128_init, aes_encrypt_block};\nimport "/home/z/my-project/workdir/vuma/womb/crypto/symmetric/aes_cfb_ofb.vuma"::{CfbOfbIv, CfbOfbBuf, aes_ofb_crypt};',
        "state_setup": "let ctx = state_new(AesCtx);\n    let key = state_new(AesKey);\n    let iv = state_new(CfbOfbIv);\n    let data = state_new(CfbOfbBuf);",
        "call": "aes128_init(ctx, key);\n    aes_ofb_crypt(ctx, iv, data, 1);",
        "output_field": "data.bytes", "output_len": 16,
        "input_field": "data.bytes", "has_key": True, "key_field": "key.data", "key_len": 16,
    },
    "aes_extra_modes": {
        "import": 'import "/home/z/my-project/workdir/vuma/womb/crypto/symmetric/aes128.vuma"::{AesKey};\nimport "/home/z/my-project/workdir/vuma/womb/crypto/symmetric/aes_extra_modes.vuma"::{AesXBuf, aes_cmac};',
        "state_setup": "let key = state_new(AesKey);\n    let msg = state_new(AesXBuf);\n    let out = state_new(AesXBuf);",
        "call": "aes_cmac(key, msg, {LEN}, out);",
        "output_field": "out.bytes", "output_len": 16,
        "input_field": "msg.bytes", "has_key": True, "key_field": "key.data", "key_len": 16,
    },
    "aes_modes": {
        "import": 'import "/home/z/my-project/workdir/vuma/womb/crypto/symmetric/aes128.vuma"::{AesCtx, AesKey, aes128_init};\nimport "/home/z/my-project/workdir/vuma/womb/crypto/symmetric/aes_modes.vuma"::{AesModeBuf, aes_ecb_encrypt};',
        "state_setup": "let ctx = state_new(AesCtx);\n    let key = state_new(AesKey);\n    let pt = state_new(AesModeBuf);\n    let ct = state_new(AesModeBuf);",
        "call": "aes128_init(ctx, key);\n    aes_ecb_encrypt(ctx, pt, ct, 1);",
        "output_field": "ct.bytes", "output_len": 16,
        "input_field": "pt.bytes", "has_key": True, "key_field": "key.data", "key_len": 16,
    },
    "chacha20_poly1305": {
        "import": 'import "/home/z/my-project/workdir/vuma/womb/crypto/symmetric/chacha20_poly1305.vuma"::{ChaCha20Key, ChaCha20Nonce, ChaCha20Buf, Poly1305Tag, chacha20_poly1305_encrypt};',
        "state_setup": "let key = state_new(ChaCha20Key);\n    let nonce = state_new(ChaCha20Nonce);\n    let aad = state_new(ChaCha20Buf);\n    let plaintext = state_new(ChaCha20Buf);\n    let ciphertext = state_new(ChaCha20Buf);\n    let tag = state_new(Poly1305Tag);",
        "call": "chacha20_poly1305_encrypt(key, nonce, aad, 0, plaintext, {LEN}, ciphertext, tag);",
        "output_field": "tag.bytes", "output_len": 16,
        "input_field": "plaintext.bytes", "has_key": True, "key_field": "key.bytes", "key_len": 32,
    },
    "des_rc4_aria_camellia": {
        "import": 'import "/home/z/my-project/workdir/vuma/womb/crypto/symmetric/des_rc4_aria_camellia.vuma"::{DesWord, des_encrypt_block};',
        "state_setup": "let pt = state_new(DesWord);\n    let key = state_new(DesWord);\n    let ct = state_new(DesWord);",
        "call": "des_encrypt_block(pt, key, ct);",
        "output_field": "ct.bytes", "output_len": 8,
        "input_field": "pt.bytes", "has_key": True, "key_field": "key.bytes", "key_len": 8,
    },
    # ─── MAC / KDF (4) ───────────────────────────────────────────────────────
    "scrypt": {
        "import": 'import "/home/z/my-project/workdir/vuma/womb/crypto/mac_kdf/hmac.vuma"::{HmacKey, HmacMsg};\nimport "/home/z/my-project/workdir/vuma/womb/crypto/mac_kdf/scrypt.vuma"::{ScryptBuf, scrypt};',
        "state_setup": "let password = state_new(HmacKey);\n    let salt = state_new(HmacMsg);\n    let out = state_new(HmacMsg);",
        "call": "scrypt(password, 32, salt, {LEN}, 16, 8, 1, out, 32);",
        "output_field": "out.bytes", "output_len": 32,
        "input_field": "salt.bytes", "has_key": True, "key_field": "password.bytes", "key_len": 32,
    },
    "argon2": {
        "import": 'import "/home/z/my-project/workdir/vuma/womb/crypto/mac_kdf/argon2.vuma"::{Argon2Buf, argon2id};',
        "state_setup": "let password = state_new(Argon2Buf);\n    let salt = state_new(Argon2Buf);\n    let out = state_new(Argon2Buf);",
        "call": "argon2id(password, 32, salt, {LEN}, 1, 8, 1, out, 32);",
        "output_field": "out.bytes", "output_len": 32,
        "input_field": "salt.bytes", "has_key": True, "key_field": "password.bytes", "key_len": 32,
    },
    "cmac_bcrypt_kdf": {
        "import": 'import "/home/z/my-project/workdir/vuma/womb/crypto/symmetric/aes128.vuma"::{AesKey, AesBlock};\nimport "/home/z/my-project/workdir/vuma/womb/crypto/mac_kdf/hmac.vuma"::{HmacMsg};\nimport "/home/z/my-project/workdir/vuma/womb/crypto/mac_kdf/cmac_bcrypt_kdf.vuma"::{KcCtx, aes_cmac_kdf};',
        "state_setup": "let key = state_new(AesKey);\n    let msg = state_new(HmacMsg);\n    let out = state_new(AesBlock);",
        "call": "aes_cmac_kdf(key, msg, {LEN}, out);",
        "output_field": "out.data", "output_len": 16,
        "input_field": "msg.bytes", "has_key": True, "key_field": "key.data", "key_len": 16,
    },
    "key_agreement": {
        "import": 'import "/home/z/my-project/workdir/vuma/womb/crypto/asym/x25519.vuma"::{X25519Bytes};\nimport "/home/z/my-project/workdir/vuma/womb/crypto/mac_kdf/key_agreement.vuma"::{x25519_keygen};',
        "state_setup": "let privkey = state_new(X25519Bytes);\n    let pubkey = state_new(X25519Bytes);",
        "call": "x25519_keygen(privkey, pubkey);",
        "output_field": "pubkey.data", "output_len": 32,
        "input_field": "privkey.data",
    },
    # ─── DRBG (2) ────────────────────────────────────────────────────────────
    "drbg": {
        "import": 'import "/home/z/my-project/workdir/vuma/womb/crypto/drbg/drbg.vuma"::{DrbgCtx, DrbgData, DrbgOut, drbg_init, drbg_generate};',
        "state_setup": "let drbg = state_new(DrbgCtx);\n    let entropy = state_new(DrbgData);\n    let nonce = state_new(DrbgData);\n    let out = state_new(DrbgOut);",
        "call": "drbg_init(drbg, entropy, {LEN}, nonce, 0);\n    drbg_generate(drbg, out, 32);",
        "output_field": "out.bytes", "output_len": 32,
        "input_field": "entropy.bytes",
    },
    "drbg_extra": {
        "import": 'import "/home/z/my-project/workdir/vuma/womb/crypto/drbg/drbg_extra.vuma"::{DrbgExtraCtx, DrbgXBuf, hash_drbg_init, hash_drbg_generate};',
        "state_setup": "let state = state_new(DrbgExtraCtx);\n    let entropy = state_new(DrbgXBuf);\n    let nonce = state_new(DrbgXBuf);\n    let out = state_new(DrbgXBuf);",
        "call": "hash_drbg_init(state, entropy, {LEN}, nonce, 0);\n    hash_drbg_generate(state, out, 32);",
        "output_field": "out.bytes", "output_len": 32,
        "input_field": "entropy.bytes",
    },
    # ─── Bignum (2) ──────────────────────────────────────────────────────────
    "bignum": {
        "import": 'import "/home/z/my-project/workdir/vuma/womb/crypto/bignum/bignum.vuma"::{Bn256Ctx, bn256_add};',
        "state_setup": "let a = state_new(Bn256Ctx);\n    let r = state_new(Bn256Ctx);",
        "call": "bn256_add(r, a, a);",
        "output_field": "r.limbs", "output_len": 4,
        "input_field": "a.limbs",
    },
    "bignum2048": {
        "import": 'import "/home/z/my-project/workdir/vuma/womb/crypto/bignum/bignum2048.vuma"::{Bn2048Ctx, bn2048_add};',
        "state_setup": "let a = state_new(Bn2048Ctx);\n    let r = state_new(Bn2048Ctx);",
        "call": "bn2048_add(r, a, a);",
        "output_field": "r.limbs", "output_len": 32,
        "input_field": "a.limbs",
    },
    # ─── Asymmetric (9) ──────────────────────────────────────────────────────
    "rsa": {
        "import": 'import "/home/z/my-project/workdir/vuma/womb/crypto/asym/rsa.vuma"::{Rsa256, RsaKey, rsa_bn_add};',
        "state_setup": "let a = state_new(Rsa256);\n    let r = state_new(Rsa256);",
        "call": "rsa_bn_add(r, a, a);",
        "output_field": "r.bytes", "output_len": 32,
        "input_field": "a.bytes",
    },
    "rsa_oaep_pss": {
        "import": 'import "/home/z/my-project/workdir/vuma/womb/crypto/asym/rsa_oaep_pss.vuma"::{Rsa256, RsaHash, oaep_sha256};',
        "state_setup": "let data = state_new(Rsa256);\n    let out = state_new(RsaHash);",
        "call": "oaep_sha256(data, {LEN}, out);",
        "output_field": "out.bytes", "output_len": 32,
        "input_field": "data.bytes",
    },
    "rsa_pkcs1_ecdsa_extra": {
        "import": 'import "/home/z/my-project/workdir/vuma/womb/crypto/asym/rsa_pkcs1_ecdsa_extra.vuma"::{Ed448Seed, ed448_keygen};',
        "state_setup": "let privkey = state_new(Ed448Seed);\n    let pubkey = state_new(Ed448Seed);",
        "call": "ed448_keygen(privkey, pubkey);",
        "output_field": "pubkey.bytes", "output_len": 32,
        "input_field": "privkey.bytes",
    },
    "ed25519": {
        "import": 'import "/home/z/my-project/workdir/vuma/womb/crypto/asym/ed25519.vuma"::{B32, ed25519_keygen};',
        "state_setup": "let privkey = state_new(B32);\n    let pubkey = state_new(B32);",
        "call": "ed25519_keygen(privkey, pubkey);",
        "output_field": "pubkey.bytes", "output_len": 32,
        "input_field": "privkey.bytes",
    },
    "x25519": {
        "import": 'import "/home/z/my-project/workdir/vuma/womb/crypto/asym/x25519.vuma"::{X25519Bytes, x25519_base};',
        "state_setup": "let scalar = state_new(X25519Bytes);\n    let out = state_new(X25519Bytes);",
        "call": "x25519_base(out, scalar);",
        "output_field": "out.data", "output_len": 32,
        "input_field": "scalar.data",
    },
    "ecdsa_p256": {
        "import": 'import "/home/z/my-project/workdir/vuma/womb/crypto/asym/ecdsa_p256.vuma"::{B32, B256, ecdsa_p256_keygen};',
        "state_setup": "let privkey = state_new(B32);\n    let pubx = state_new(B32);\n    let puby = state_new(B32);",
        "call": "ecdsa_p256_keygen(privkey, pubx, puby);",
        "output_field": "pubx.bytes", "output_len": 32,
        "input_field": "privkey.bytes",
    },
    "ecdsa_p384": {
        "import": 'import "/home/z/my-project/workdir/vuma/womb/crypto/asym/ecdsa_p384.vuma"::{B48, B256, ecdsa_p384_keygen};',
        "state_setup": "let privkey = state_new(B48);\n    let pubx = state_new(B48);\n    let puby = state_new(B48);",
        "call": "ecdsa_p384_keygen(privkey, pubx, puby);",
        "output_field": "pubx.bytes", "output_len": 48,
        "input_field": "privkey.bytes",
    },
    "ecdh_p256": {
        "import": 'import "/home/z/my-project/workdir/vuma/womb/crypto/asym/ecdh_p256.vuma"::{B32, B256, ecdh_p256_keygen};',
        "state_setup": "let privkey = state_new(B32);\n    let pubx = state_new(B32);\n    let puby = state_new(B32);",
        "call": "ecdh_p256_keygen(privkey, pubx, puby);",
        "output_field": "pubx.bytes", "output_len": 32,
        "input_field": "privkey.bytes",
    },
    "secp256k1": {
        "import": 'import "/home/z/my-project/workdir/vuma/womb/crypto/asym/secp256k1.vuma"::{B32, B256, secp256k1_pubkeygen};',
        "state_setup": "let privkey = state_new(B32);\n    let pubx = state_new(B32);\n    let puby = state_new(B32);",
        "call": "secp256k1_pubkeygen(privkey, pubx, puby);",
        "output_field": "pubx.bytes", "output_len": 32,
        "input_field": "privkey.bytes",
    },
    # ─── Post-quantum (5) ────────────────────────────────────────────────────
    "ml_kem": {
        "import": 'import "/home/z/my-project/workdir/vuma/womb/crypto/post_quantum/ml_kem.vuma"::{MlKemBuf, MlKemEntropy, ml_kem_keygen_512};',
        "state_setup": "let pool = state_new(MlKemEntropy);\n    let pk = state_new(MlKemBuf);\n    let sk = state_new(MlKemBuf);",
        "call": "ml_kem_keygen_512(pool, pk, sk);",
        "output_field": "pk.bytes", "output_len": 32,
        "input_field": "pool.bytes",
    },
    "ml_dsa": {
        "import": 'import "/home/z/my-project/workdir/vuma/womb/crypto/post_quantum/ml_dsa.vuma"::{MlDsaBuf, ml_dsa_keygen};',
        "state_setup": "let seed = state_new(MlDsaBuf);\n    let pk = state_new(MlDsaBuf);\n    let sk = state_new(MlDsaBuf);",
        "call": "ml_dsa_keygen(seed, pk, sk);",
        "output_field": "pk.bytes", "output_len": 32,
        "input_field": "seed.bytes",
    },
    "slh_dsa": {
        "import": 'import "/home/z/my-project/workdir/vuma/womb/crypto/post_quantum/slh_dsa.vuma"::{SlhBuf, slh_dsa_keygen};',
        "state_setup": "let seed = state_new(SlhBuf);\n    let pk = state_new(SlhBuf);\n    let sk = state_new(SlhBuf);",
        "call": "slh_dsa_keygen(seed, pk, sk);",
        "output_field": "pk.bytes", "output_len": 32,
        "input_field": "seed.bytes",
    },
    "falcon": {
        "import": 'import "/home/z/my-project/workdir/vuma/womb/crypto/post_quantum/falcon.vuma"::{FalconBuf, falcon_keygen};',
        "state_setup": "let seed = state_new(FalconBuf);\n    let pk = state_new(FalconBuf);\n    let sk = state_new(FalconBuf);",
        "call": "falcon_keygen(seed, pk, sk);",
        "output_field": "pk.bytes", "output_len": 32,
        "input_field": "seed.bytes",
    },
    "hqc": {
        "import": 'import "/home/z/my-project/workdir/vuma/womb/crypto/post_quantum/hqc.vuma"::{HqcBuf, hqc_keygen};',
        "state_setup": "let seed = state_new(HqcBuf);\n    let pk = state_new(HqcBuf);\n    let sk = state_new(HqcBuf);",
        "call": "hqc_keygen(seed, pk, sk);",
        "output_field": "pk.bytes", "output_len": 32,
        "input_field": "seed.bytes",
    },
}

def hex_to_bytes(hex_str):
    if not hex_str:
        return []
    return [int(hex_str[i:i+2], 16) for i in range(0, len(hex_str), 2)]

def gen_harness(module_name, api, vectors):
    has_key = api.get("has_key", False)
    output_len = api["output_len"]
    input_field = api["input_field"]
    output_field = api["output_field"]
    key_field = api.get("key_field", "")
    key_len_val = api.get("key_len", 0)
    call_template = api["call"]
    num_vecs = min(len(vectors), NUM_VECTORS)

    lines = []
    lines.append(f"// Auto-generated VUMA harness for {module_name}")
    lines.append(f"// Tests {num_vecs} vectors against C reference (OpenSSL)")
    lines.append(f"// Reuses state variables to avoid arena overflow")
    lines.append("")
    lines.append(api["import"])
    lines.append("")
    lines.append("transform main() -> i32 {")

    # Single state setup (reused across all vectors)
    lines.append(f"    // Shared state (reused for all vectors)")
    for state_line in api["state_setup"].split("\n"):
        state_line = state_line.strip()
        if state_line:
            lines.append(f"    {state_line}")

    # Output counter (reused)
    lines.append(f"    let oi: u32 = 0;")

    for vi in range(num_vecs):
        vec = vectors[vi]
        input_hex = vec.get("input_hex", "")
        input_bytes = hex_to_bytes(input_hex)
        input_len = len(input_bytes)

        lines.append(f"")
        lines.append(f"    // Vector {vi}: {vec.get('desc', '')}")

        # Set input bytes (no zeroing needed — hash uses len param)
        for bi, b in enumerate(input_bytes):
            lines.append(f"    {input_field}[{bi}] = {b};")

        # Set key if needed
        if has_key:
            key_hex = vec.get("key_hex", "")
            key_bytes = hex_to_bytes(key_hex)
            for bi, b in enumerate(key_bytes):
                lines.append(f"    {key_field}[{bi}] = {b};")

        # Call the function
        call = call_template.replace("{LEN}", str(input_len)).replace("{KEYLEN}", str(key_len_val))
        for cl in call.strip().split("\n"):
            lines.append(f"    {cl.strip()}")

        # Output result bytes (reuse oi counter)
        lines.append(f"    oi = 0;")
        lines.append(f"    while oi < {output_len} {{")
        lines.append(f"        print_int(1000 + ({output_field}[oi] as u32));")
        lines.append(f"        oi = oi + 1;")
        lines.append(f"    }}")
        lines.append(f"    print_int(999);")

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
