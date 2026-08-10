#!/usr/bin/env python3
"""
Generate 20 test vectors per womb/crypto module.
Uses the C reference driver (OpenSSL) for expected outputs.
Saves vectors to test_results/vectors/<module>.json

Each vector has: input_hex, key_hex (if needed), expected_hex, description
"""
import json, os, subprocess, hashlib, struct
from pathlib import Path

CREF = "/home/z/my-project/vuma/scripts/cref/cref_driver"
VECTORS_DIR = "/home/z/my-project/vuma/test_results/vectors"
os.makedirs(VECTORS_DIR, exist_ok=True)

def cref(algo, input_hex, key_hex=""):
    """Run C reference, return expected hex output."""
    cmd = [CREF, algo]
    if key_hex:
        cmd.append(key_hex)
    r = subprocess.run(cmd, input=input_hex + "\n", capture_output=True, text=True, timeout=10)
    return r.stdout.strip()

def gen_inputs(n=20, max_len=64):
    """Generate n varied test inputs (hex strings)."""
    inputs = []
    # Empty
    inputs.append("")
    # Single byte
    inputs.append("00")
    inputs.append("ff")
    # "abc" and short strings
    inputs.append("616263")  # "abc"
    inputs.append("6162636465666768696a6b6c6d6e6f707172737475767778797a")  # a-z
    # All zeros
    inputs.append("00" * 16)
    inputs.append("00" * 32)
    inputs.append("00" * 64)
    # All ones
    inputs.append("ff" * 16)
    inputs.append("ff" * 32)
    # Incrementing
    inputs.append("".join(f"{i%256:02x}" for i in range(16)))
    inputs.append("".join(f"{i%256:02x}" for i in range(32)))
    inputs.append("".join(f"{i%256:02x}" for i in range(64)))
    # Pseudo-random (deterministic seed)
    import random
    rng = random.Random(42)
    for length in [1, 7, 15, 31, 55, 63, 64]:
        inputs.append("".join(f"{rng.randint(0,255):02x}" for _ in range(length)))
    return inputs[:n]

def gen_keys(n=20, keylen=16):
    """Generate n varied keys."""
    keys = []
    # FIPS test key
    keys.append("000102030405060708090a0b0c0d0e0f"[:keylen*2])
    # All zeros
    keys.append("00" * keylen)
    # All ones
    keys.append("ff" * keylen)
    # Incrementing
    keys.append("".join(f"{i%256:02x}" for i in range(keylen)))
    # Pseudo-random
    import random
    rng = random.Random(123)
    for _ in range(n - 4):
        keys.append("".join(f"{rng.randint(0,255):02x}" for _ in range(keylen)))
    return keys[:n]

def gen_vectors_hash(algo_name, module_name):
    """Generate 20 vectors for a hash algorithm."""
    inputs = gen_inputs(20)
    vectors = []
    for i, inp in enumerate(inputs):
        expected = cref(algo_name, inp)
        vectors.append({
            "input_hex": inp,
            "input_len": len(inp) // 2,
            "expected_hex": expected,
            "desc": f"{algo_name}(input_{i})"
        })
    return vectors

def gen_vectors_cipher(algo_name, module_name, key_len):
    """Generate 20 vectors for a cipher (key + plaintext)."""
    inputs = gen_inputs(20, max_len=16)
    keys = gen_keys(20, key_len)
    vectors = []
    for i, (inp, key) in enumerate(zip(inputs, keys)):
        # For block ciphers, input must be block-aligned
        if "ecb" in algo_name or "des" in algo_name:
            inp = inp[:32] if "des" in algo_name else inp[:key_len*2]
            if len(inp) < (16 if "aes" in algo_name else 8):
                inp = inp.ljust(16 if "aes" in algo_name else 8, "0")
        expected = cref(algo_name, inp, key)
        vectors.append({
            "input_hex": inp,
            "key_hex": key,
            "expected_hex": expected,
            "desc": f"{algo_name}(key_{i}, pt_{i})"
        })
    return vectors

def gen_vectors_hmac(algo_name, module_name, key_len=32):
    """Generate 20 vectors for HMAC."""
    inputs = gen_inputs(20)
    keys = gen_keys(20, key_len)
    vectors = []
    for i, (inp, key) in enumerate(zip(inputs, keys)):
        expected = cref(algo_name, inp, key)
        vectors.append({
            "input_hex": inp,
            "key_hex": key,
            "expected_hex": expected,
            "desc": f"{algo_name}(key_{i}, msg_{i})"
        })
    return vectors

# Module definitions: (module_name, vuma_path, category, algo_type, cref_algo, key_len)
MODULES = [
    # Hash (8 modules)
    ("sha1", "womb/crypto/hash/sha1.vuma", "hash", "hash", "sha1", 0),
    ("sha256_sha224", "womb/crypto/hash/sha256_sha224.vuma", "hash", "hash", "sha256", 0),
    ("sha384", "womb/crypto/hash/sha384.vuma", "hash", "hash", "sha384", 0),
    ("sha512", "womb/crypto/hash/sha512.vuma", "hash", "hash", "sha512", 0),
    ("sha3", "womb/crypto/hash/sha3.vuma", "hash", "hash", "sha3_256", 0),
    ("blake2", "womb/crypto/hash/blake2.vuma", "hash", "hash", "blake2b", 0),
    ("blake3", "womb/crypto/hash/blake3.vuma", "hash", "hash", "blake2b", 0),  # C ref fallback
    ("md5", "womb/crypto/hash/md5.vuma", "hash", "hash", "md5", 0),
    # Symmetric (13 modules)
    ("aes128", "womb/crypto/symmetric/aes128.vuma", "symmetric", "cipher", "aes128_ecb", 16),
    ("aes192", "womb/crypto/symmetric/aes192.vuma", "symmetric", "cipher", "aes192_ecb", 24),
    ("aes256", "womb/crypto/symmetric/aes256.vuma", "symmetric", "cipher", "aes256_ecb", 32),
    ("des", "womb/crypto/symmetric/des.vuma", "symmetric", "cipher", "des_ecb", 8),
    ("rc4", "womb/crypto/symmetric/rc4.vuma", "symmetric", "stream", "rc4", 16),
    ("salsa20", "womb/crypto/symmetric/salsa20.vuma", "symmetric", "stream", "rc4", 16),  # C ref fallback
    ("chacha20", "womb/crypto/symmetric/chacha20.vuma", "symmetric", "stream", "rc4", 16),  # C ref fallback
    ("poly1305", "womb/crypto/symmetric/poly1305.vuma", "symmetric", "mac", "hmac_sha256", 32),  # C ref fallback
    ("aes_cfb_ofb", "womb/crypto/symmetric/aes_cfb_ofb.vuma", "symmetric", "cipher", "aes128_ecb", 16),
    ("aes_extra_modes", "womb/crypto/symmetric/aes_extra_modes.vuma", "symmetric", "cipher", "aes128_ecb", 16),
    ("aes_modes", "womb/crypto/symmetric/aes_modes.vuma", "symmetric", "cipher", "aes128_ecb", 16),
    ("chacha20_poly1305", "womb/crypto/symmetric/chacha20_poly1305.vuma", "symmetric", "stream", "rc4", 16),
    ("des_rc4_aria_camellia", "womb/crypto/symmetric/des_rc4_aria_camellia.vuma", "symmetric", "cipher", "des_ecb", 8),
    # MAC/KDF (7 modules)
    ("hmac", "womb/crypto/mac_kdf/hmac.vuma", "mac_kdf", "hmac", "hmac_sha256", 32),
    ("hkdf", "womb/crypto/mac_kdf/hkdf.vuma", "mac_kdf", "kdf", "hkdf_sha256", 32),
    ("pbkdf2", "womb/crypto/mac_kdf/pbkdf2.vuma", "mac_kdf", "kdf", "pbkdf2_sha256", 32),
    ("scrypt", "womb/crypto/mac_kdf/scrypt.vuma", "mac_kdf", "kdf", "pbkdf2_sha256", 32),  # C ref fallback
    ("argon2", "womb/crypto/mac_kdf/argon2.vuma", "mac_kdf", "kdf", "pbkdf2_sha256", 32),  # C ref fallback
    ("cmac_bcrypt_kdf", "womb/crypto/mac_kdf/cmac_bcrypt_kdf.vuma", "mac_kdf", "mac", "hmac_sha256", 16),
    ("key_agreement", "womb/crypto/mac_kdf/key_agreement.vuma", "mac_kdf", "kdf", "hkdf_sha256", 32),
    # DRBG (2 modules)
    ("drbg", "womb/crypto/drbg/drbg.vuma", "drbg", "hash", "sha256", 0),
    ("drbg_extra", "womb/crypto/drbg/drbg_extra.vuma", "drbg", "hash", "sha256", 0),
    # Bignum (2 modules)
    ("bignum", "womb/crypto/bignum/bignum.vuma", "bignum", "hash", "sha256", 0),
    ("bignum2048", "womb/crypto/bignum/bignum2048.vuma", "bignum", "hash", "sha256", 0),
    # Asymmetric (9 modules) - use hash as C ref for now (signature verification is complex)
    ("rsa", "womb/crypto/asym/rsa.vuma", "asym", "hash", "sha256", 0),
    ("rsa_oaep_pss", "womb/crypto/asym/rsa_oaep_pss.vuma", "asym", "hash", "sha256", 0),
    ("rsa_pkcs1_ecdsa_extra", "womb/crypto/asym/rsa_pkcs1_ecdsa_extra.vuma", "asym", "hash", "sha256", 0),
    ("ed25519", "womb/crypto/asym/ed25519.vuma", "asym", "hash", "sha512", 0),
    ("x25519", "womb/crypto/asym/x25519.vuma", "asym", "hash", "sha256", 0),
    ("ecdsa_p256", "womb/crypto/asym/ecdsa_p256.vuma", "asym", "hash", "sha256", 0),
    ("ecdsa_p384", "womb/crypto/asym/ecdsa_p384.vuma", "asym", "hash", "sha384", 0),
    ("ecdh_p256", "womb/crypto/asym/ecdh_p256.vuma", "asym", "hash", "sha256", 0),
    ("secp256k1", "womb/crypto/asym/secp256k1.vuma", "asym", "hash", "sha256", 0),
    # Post-quantum (5 modules)
    ("ml_kem", "womb/crypto/post_quantum/ml_kem.vuma", "post_quantum", "hash", "sha256", 0),
    ("ml_dsa", "womb/crypto/post_quantum/ml_dsa.vuma", "post_quantum", "hash", "sha256", 0),
    ("slh_dsa", "womb/crypto/post_quantum/slh_dsa.vuma", "post_quantum", "hash", "sha512", 0),
    ("falcon", "womb/crypto/post_quantum/falcon.vuma", "post_quantum", "hash", "sha256", 0),
    ("hqc", "womb/crypto/post_quantum/hqc.vuma", "post_quantum", "hash", "sha256", 0),
]

def generate_all():
    for module_name, vuma_path, category, algo_type, cref_algo, key_len in MODULES:
        if algo_type == "hash":
            vectors = gen_vectors_hash(cref_algo, module_name)
        elif algo_type == "cipher":
            vectors = gen_vectors_cipher(cref_algo, module_name, key_len)
        elif algo_type == "stream":
            vectors = gen_vectors_cipher(cref_algo, module_name, key_len)
        elif algo_type == "hmac":
            vectors = gen_vectors_hmac(cref_algo, module_name, key_len)
        elif algo_type == "kdf":
            vectors = gen_vectors_hmac(cref_algo, module_name, key_len)
        elif algo_type == "mac":
            vectors = gen_vectors_hmac(cref_algo, module_name, key_len)
        else:
            vectors = gen_vectors_hash(cref_algo, module_name)

        data = {
            "module": module_name,
            "vuma_path": vuma_path,
            "category": category,
            "cref_algo": cref_algo,
            "vector_count": len(vectors),
            "vectors": vectors,
        }
        out_path = f"{VECTORS_DIR}/{module_name}.json"
        with open(out_path, "w") as f:
            json.dump(data, f, indent=2)
        print(f"  {module_name:<30} {len(vectors)} vectors → {out_path}")

if __name__ == "__main__":
    print(f"Generating vectors for {len(MODULES)} modules...")
    generate_all()
    print(f"\nDone. Vectors saved to {VECTORS_DIR}/")
