# womb/crypto/ — Cryptographic Primitives

VUMA's cryptographic standard library. All files use **legacy pointer syntax**
(pre-PMT) — migration to PMT is planned but not yet started.

## Structure (7 subdirectories)

### `symmetric/` — Symmetric Ciphers (11 files)
| File | Algorithm | LOC |
|------|-----------|-----|
| `aes128.vuma` | AES-128 (FIPS-197) | 856 |
| `aes192.vuma` | AES-192 | 281 |
| `aes256.vuma` | AES-256 | 223 |
| `aes_modes.vuma` | AES CBC/CTR/XTS/KW/CMAC modes | 508 |
| `aes_cfb_ofb.vuma` | AES CFB/OFB modes | 343 |
| `aes_extra_modes.vuma` | AES GCM/CCM/EAX/OCB | 1001 |
| `chacha20.vuma` | ChaCha20 stream cipher | 229 |
| `chacha20_poly1305.vuma` | ChaCha20-Poly1305 AEAD | 199 |
| `salsa20.vuma` | Salsa20 stream cipher | 362 |
| `poly1305.vuma` | Poly1305 MAC | 278 |
| `des_rc4_aria_camellia.vuma` | DES, RC4, ARIA, Camellia (legacy) | 3424 |

### `hash/` — Hash Functions (8 files)
| File | Algorithm | LOC |
|------|-----------|-----|
| `sha1.vuma` | SHA-1 | 206 |
| `sha256_sha224.vuma` | SHA-256 + SHA-224 | 1525 |
| `sha384.vuma` | SHA-384 | 414 |
| `sha512.vuma` | SHA-512 | 414 |
| `sha3.vuma` | SHA-3 (Keccak) | 419 |
| `blake2.vuma` | BLAKE2 | 288 |
| `blake3.vuma` | BLAKE3 | 541 |
| `md5.vuma` | MD5 (legacy, do not use for security) | 284 |

### `mac_kdf/` — MACs and KDFs (7 files)
| File | Algorithm | LOC |
|------|-----------|-----|
| `hmac.vuma` | HMAC (RFC 2104) | 192 |
| `hkdf.vuma` | HKDF (RFC 5869) | 151 |
| `pbkdf2.vuma` | PBKDF2 (RFC 8018) | 203 |
| `scrypt.vuma` | scrypt (RFC 7914) | 241 |
| `argon2.vuma` | Argon2 (RFC 9106) | 437 |
| `cmac_bcrypt_kdf.vuma` | CMAC, bcrypt, KDF | 602 |
| `key_agreement.vuma` | Key agreement protocols | 757 |

### `asym/` — Asymmetric Cryptography (9 files)
| File | Algorithm | LOC |
|------|-----------|-----|
| `rsa.vuma` | RSA (PKCS#1 v1.5) | 559 |
| `rsa_oaep_pss.vuma` | RSA-OAEP + RSA-PSS | 520 |
| `ecdsa_p256.vuma` | ECDSA on P-256 | 607 |
| `ecdsa_p384.vuma` | ECDSA on P-384 | 782 |
| `ed25519.vuma` | Ed25519 (RFC 8032) | 593 |
| `ecdh_p256.vuma` | ECDH on P-256 | 178 |
| `x25519.vuma` | X25519 (RFC 7748) | 554 |
| `secp256k1.vuma` | secp256k1 (Bitcoin curve) | 473 |
| `rsa_pkcs1_ecdsa_extra.vuma` | RSA PKCS#1 + ECDSA extras | 1537 |

### `post_quantum/` — Post-Quantum Cryptography (5 files)
| File | Algorithm | LOC |
|------|-----------|-----|
| `ml_kem.vuma` | ML-KEM (Kyber, FIPS 203) | 1343 |
| `ml_dsa.vuma` | ML-DSA (Dilithium, FIPS 204) | 584 |
| `slh_dsa.vuma` | SLH-DSA (SPHINCS+, FIPS 205) | 302 |
| `falcon.vuma` | Falcon | 905 |
| `hqc.vuma` | HQC | 1104 |

### `drbg/` — Deterministic Random Bit Generators (2 files)
| File | Algorithm | LOC |
|------|-----------|-----|
| `drbg.vuma` | HMAC-DRBG (NIST SP 800-90A) | 191 |
| `drbg_extra.vuma` | Hash-DRBG + CTR-DRBG | 372 |

### `bignum/` — Big Number Arithmetic (2 files)
| File | Description | LOC |
|------|-------------|-----|
| `bignum.vuma` | 1024-bit big number arithmetic | 508 |
| `bignum2048.vuma` | 2048-bit big number arithmetic | 689 |

## KAT Tests
Run `scripts/run_all_kat.sh` to execute all KAT (Known Answer Test) vectors.
Run `scripts/run_real_kat.sh` for real-world test vectors.

## Migration Status
All 44 files use legacy pointer syntax (`*(ptr+offset)`, `allocate`, `free`).
PMT migration is planned — the kernel has stubs in `womb/kernel/crypto/`
waiting for real algorithm bodies.

## Naming Convention
- Files: `snake_case.vuma`
- Algorithm files: lowercase algorithm name (e.g., `aes128`, not `AES128`)
- Mode files: `algorithm_modes.vuma` (e.g., `aes_modes.vuma`)
- Compound files: `algorithm_algorithm.vuma` (e.g., `chacha20_poly1305.vuma`)
