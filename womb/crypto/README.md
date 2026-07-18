# womb/crypto/ — VUMA Cryptography Library

The `womb/crypto/` directory contains VUMA's cryptography standard library:
**45 `.vuma` files** implementing symmetric ciphers, hash functions, MACs,
key-derivation functions, asymmetric signature and key-exchange schemes,
and post-quantum primitives. The library covers the same algorithm surface
as a typical `rust-crypto` or `openssl/crypto` crate — AES, SHA-1/256/384/512,
SHA-3, BLAKE2/3, HMAC, Poly1305, ChaCha20, Salsa20, RSA-2048, Ed25519,
ECDSA P-256/P-384/P-521, X25519, ECDH, HKDF, PBKDF2, scrypt, Argon2, ML-KEM,
ML-DSA, SLH-DSA, Falcon, HQC — but is written entirely in VUMA syntax and
has zero external dependencies.

This README is the entry point for the crypto library. For the kernel's
crypto subsystem (a separate, smaller set of 5 skeleton files under
`womb/kernel/crypto/`) see
[`womb/kernel/README.md#crypto`](../kernel/README.md#crypto). For the
test-side KAT harness see [`tests/README.md`](../../tests/README.md).

---

## What's here

45 `.vuma` files, no `main()` — every file is an importable library module
that exposes init/update/final (for streaming primitives) or
encrypt/decrypt/sign/verify functions. Each file's header documents the
NIST/RFC standard it implements, the parameter sets it supports, and the
sibling modules it requires.

| Category | Files | Algorithms |
|----------|-------|------------|
| Symmetric ciphers | 11 | AES-128/192/256 (FIPS 197), AES modes (ECB/CBC/CTR, CFB/OFB, XTS/KW/KWP/GCM-SIV/CMAC), ChaCha20 (RFC 8439), Salsa20/XSalsa20, ChaCha20-Poly1305 AEAD, 3DES/Camellia/ARIA/RC4 |
| Hash functions | 8 | SHA-1 (FIPS 180-4), SHA-256 (via `sha_variants`), SHA-384, SHA-512, SHA-3/Keccak/SHAKE (FIPS 202), BLAKE2b (RFC 7693), BLAKE3, MD5 (RFC 1321) |
| MACs | 2 | HMAC (RFC 2104, generic over SHA-1/256/512), Poly1305 (RFC 8439) |
| KDFs | 6 | HKDF (RFC 5869), PBKDF2 (RFC 8018), scrypt (RFC 7914), Argon2id (RFC 9106), KDF/CMAC/bcrypt (SP 800-108/38B, RFC 6964), key_agreement (3 protocols) |
| Asymmetric | 11 | RSA-2048 (RFC 8017), RSA-OAEP/PSS, ECDSA P-256/P-384/P-521 (FIPS 186-4), Ed25519, Ed448, ECDH P-256, X25519, secp256k1 (Bitcoin/Ethereum), signatures_extra |
| Post-quantum | 5 | ML-KEM (FIPS 203 / Kyber), ML-DSA (FIPS 204 / Dilithium), SLH-DSA (FIPS 205 / SPHINCS+), Falcon-512, HQC |
| DRBG / RNG | 2 | HMAC_DRBG (SP 800-90A), HASH_DRBG + CTR_DRBG (`drbg_extra`) |
| Bignum | 2 | 256-bit multi-precision integer arithmetic, 2048-bit multi-precision integer arithmetic |
| CRC | 1 | CRC-16 (CCITT, Modbus), CRC-32 (IEEE 802.3), CRC-64 (ECMA-182) |
| **Total** | **45** | |

## Legacy vs PMT status

**Important:** the `womb/crypto/` library uses VUMA's **legacy pointer
dialect** — `allocate(...)`, `*ptr`, `free(p)` — not PMT syntax. These are
pre-PMT modules that have not yet been migrated to the 2.0 PMT-only model
(`layout` / `State<T>` / `state_new`). They still compile and run on the
gold-standard test runner (which accepts both dialects for the
`womb/crypto/` library), but they are NOT covered by the three IVE state
verifiers (`StateRead`, `StateWrite`, `StateTransform`).

The migration plan: K13+ will port `womb/crypto/` to PMT one algorithm at a
time, starting with the symmetric ciphers (AES, ChaCha20) and the SHA family.
The KAT tests under `scripts/womb_kat_tests/` and `scripts/real_kat_tests/`
are the regression gate — each algorithm's KAT tests must still pass after
migration. Until then, new code that needs PMT-pure crypto should use the
kernel's `womb/kernel/crypto/` skeletons (which ARE PMT-pure, but currently
only `cipher_encrypt`/`hash_update` are stubs that byte-wise copy or bump a
counter).

## Categories

### Symmetric ciphers

| File | Algorithm | Notes |
|------|-----------|-------|
| `aes128.vuma` | AES-128 (FIPS 197) | 10 rounds, 176-byte key schedule, encrypt + decrypt + CBC + CTR |
| `aes192.vuma` | AES-192 (FIPS 197) | 12 rounds, 208-byte key schedule |
| `aes256.vuma` | AES-256 (FIPS 197) | 14 rounds, 240-byte key schedule |
| `aes_modes.vuma` | AES-ECB/CBC/CTR (SP 800-38A) | PKCS#7 padding for ECB/CBC |
| `aes_cfb_ofb.vuma` | AES-CFB-128/8/1 + AES-OFB (SP 800-38A) | Full CFB family |
| `aes_extra_modes.vuma` | AES-XTS, AES-KW, AES-KWP, AES-GCM-SIV, AES-CMAC | Disk encryption + key wrap + CMAC |
| `chacha20.vuma` | ChaCha20 (RFC 8439) | 20-round quarter-round stream cipher |
| `chacha20_poly1305.vuma` | ChaCha20-Poly1305 AEAD (RFC 8439 §2.8) | Requires `chacha20` + `poly1305` + `hmac` |
| `salsa20.vuma` | Salsa20 / XSalsa20 (eSTREAM) | 20-round stream cipher, XSalsa20 nonce extension |
| `poly1305.vuma` | Poly1305 One-Time MAC (RFC 8439) | GF(2^130 - 5) with 5 × 26-bit limbs |
| `legacy_ciphers.vuma` | 3DES (TDEA), Camellia, ARIA, RC4 | NIST/RFC compliant, no stubs |

### Hash functions

| File | Algorithm | Output size |
|------|-----------|-------------|
| `sha1.vuma` | SHA-1 (FIPS 180-4) | 160 bits (20 B) |
| `sha_variants.vuma` | SHA-224, SHA-256, SHA-512/224, SHA-512/256 (FIPS 180-4, SP 800-185) | 224/256/224/256 bits |
| `sha384.vuma` | SHA-384 (FIPS 180-4 §6.3.2) | 384 bits (48 B) |
| `sha512.vuma` | SHA-512 (FIPS 180-4 §6.4.2) | 512 bits (64 B) |
| `sha3.vuma` | SHA-3-256/512, SHAKE128/256 (FIPS 202) | 256/512 bits + variable |
| `blake2.vuma` | BLAKE2b (RFC 7693) | up to 512 bits |
| `blake3.vuma` | BLAKE3 | variable, Merkle-tree mode |
| `md5.vuma` | MD5 (RFC 1321) | 128 bits (16 B) |

### MACs

| File | Algorithm | Notes |
|------|-----------|-------|
| `hmac.vuma` | HMAC (RFC 2104 / FIPS 198-1) | Generic over SHA-1, SHA-256, SHA-512 |
| `poly1305.vuma` | Poly1305 (RFC 8439) | Also listed under symmetric (one-time MAC) |

### KDFs

| File | Algorithm | Notes |
|------|-----------|-------|
| `hkdf.vuma` | HKDF-Extract / -Expand / oneshot (RFC 5869) | SHA-256 + SHA-512 variants |
| `pbkdf2.vuma` | PBKDF2 (RFC 8018 / PKCS#5 v2.1) | HMAC-SHA1 + HMAC-SHA256, configurable iterations |
| `scrypt.vuma` | scrypt (RFC 7914) | PBKDF2-HMAC-SHA256 + Salsa20/8 + BlockMix + ROMix |
| `argon2.vuma` | Argon2id (RFC 9106) | BLAKE2b + G compression (P + GB round) + memory-hard fill |
| `kdf_cmac_bcrypt.vuma` | KDF (SP 800-108), AES-CMAC (SP 800-38B), bcrypt (RFC 6964) | |
| `key_agreement.vuma` | Key agreement protocols | 3 mechanisms (DH-style + KEM-style + hybrid) |

### Asymmetric

| File | Algorithm | Notes |
|------|-----------|-------|
| `rsa.vuma` | RSA-2048 PKCS#1 v1.5 (RFC 8017) | keygen, encrypt/decrypt, sign/verify, CRT |
| `rsa_oaep_pss.vuma` | RSA-OAEP + RSA-PSS (PKCS#1 v2.1) | Requires `bignum2048`, `sha256`, `sha3` |
| `ecdsa_p256.vuma` | ECDSA over NIST P-256 (FIPS 186-4) | Requires `bignum`, `sha256`, `hmac` |
| `ecdsa_p384.vuma` | ECDSA over NIST P-384 (FIPS 186-4) | Requires `sha384`, `hmac` |
| `signatures_extra.vuma` | Ed448 (RFC 8032) + ECDSA P-521 (FIPS 186-4) | Requires `sha3`, `sha512` |
| `ed25519.vuma` | Ed25519 (RFC 8032) | Twisted Edwards over Curve25519, requires `sha512` + `bignum` |
| `ecdh_p256.vuma` | ECDH over NIST P-256 | Requires `ecdsa_p256` (curve params, point ops) |
| `x25519.vuma` | X25519 (RFC 7748) | Montgomery ladder over Curve25519 |
| `secp256k1.vuma` | ECDSA + ECDH over secp256k1 | Bitcoin/Ethereum curve |

### Post-quantum

| File | Algorithm | Parameter sets |
|------|-----------|----------------|
| `ml_kem.vuma` | ML-KEM (FIPS 203 / Kyber) | ML-KEM-512 (level 1), -768 (level 3), -1024 (level 5) |
| `ml_dsa.vuma` | ML-DSA (FIPS 204 / Dilithium) | ML-DSA-44, -65, -87 |
| `slh_dsa.vuma` | SLH-DSA (FIPS 205 / SPHINCS+) | SLH-DSA-SHA2-128s (small) |
| `falcon.vuma` | Falcon | Falcon-512 (n=512, q=12289, level 1) |
| `hqc.vuma` | HQC (Hamming Quasi-Cyclic) | Code-based KEM |

### DRBG / RNG

| File | Algorithm | Notes |
|------|-----------|-------|
| `drbg.vuma` | HMAC_DRBG (SP 800-90A Rev. 1) | NIST-approved, backtracking resistance |
| `drbg_extra.vuma` | HASH_DRBG + CTR_DRBG (SP 800-90A) | Two more NIST-approved mechanisms |

### Bignum

| File | Width | Used by |
|------|-------|---------|
| `bignum.vuma` | 256-bit (4 × u64 limbs, LE) | ECDSA P-256, ECDH, Ed25519, X25519, secp256k1 |
| `bignum2048.vuma` | 2048-bit (32 × u64 limbs, LE) | RSA-2048, RSA-OAEP, RSA-PSS |

### CRC

| File | Variants |
|------|----------|
| `crc.vuma` | CRC-16 (CCITT, Modbus), CRC-32 (IEEE 802.3 — ZIP/PNG/Ethernet), CRC-64 (ECMA-182) |

## KAT tests

Known-answer tests for the crypto library live in two directories:

| Directory | Files | Scope |
|-----------|-------|-------|
| [`scripts/womb_kat_tests/`](../../scripts/womb_kat_tests/) | 86 | Womb-library KAT tests — every algorithm in `womb/crypto/` + `womb/lib/` has at least one |
| [`scripts/real_kat_tests/`](../../scripts/real_kat_tests/) | 127 | Real cross-architecture KAT tests — known-answer vectors verified across multiple backends |

Run them with:

```bash
bash scripts/run_all_kat.sh        # womb KAT tests
bash scripts/run_real_kat.sh       # real cross-arch KAT suite
```

Each KAT test is a `.vuma` program that computes a hash, ciphertext, or
signature and checks it against a known value. Algorithm coverage:
SHA-256, AES-128/192/256, Ed25519, ECDSA P-256/P-384, ML-DSA, ML-KEM,
SLH-DSA, Falcon, HQC, X25519, ChaCha20, Poly1305, Argon2, scrypt, HKDF,
HMAC, RSA-OAEP-PSS, BLAKE2/3, SHA-3/SHAKE, and more.

The test generator scripts
[`scripts/gen_real_kat.py`](../../scripts/gen_real_kat.py) and
[`scripts/generate_all_kat_tests.py`](../../scripts/generate_all_kat_tests.py)
auto-generate the KAT test files from NIST CAVP / RFC test vectors.

## How to use

These modules use the **legacy pointer dialect** (`allocate`, `*ptr`,
`free`). They are pre-PMT and have not yet been migrated. A typical call
pattern:

```vuma
// SHA-256 of a 32-byte message:
msg = allocate(32);
// ... fill msg with bytes ...
digest = allocate(32);
sha256_oneshot(msg, 32, digest);
// digest now contains the 32-byte SHA-256 hash
free(msg);
free(digest);
```

To call a crypto function from a kernel module (PMT-pure), you currently
have two options:

1. Use the kernel's `womb/kernel/crypto/api.vuma` CipherCtx/HashCtx
   interface — but note that `cipher_encrypt` and `hash_update` are stubs
   (byte-wise copy / counter bump). Real algorithm bodies land in K13+.
2. Inline the algorithm directly in your kernel module using flat byte
   arrays + pack/unpack helpers. This is what `examples/sha256d.vuma` and
   `examples/test_sha_manual.vuma` do.

See [`docs/contributing.md` §3 PMT-Only Test Policy](../../docs/contributing.md#3-pmt-only-test-policy)
for the migration note about `womb/crypto/` and `womb/net/` being pre-PMT.

## See also

- [`womb/kernel/README.md#crypto`](../kernel/README.md) — the kernel's
  separate, PMT-pure crypto subsystem (5 skeleton files).
- [`tests/README.md`](../../tests/README.md) — KAT test harness layout.
- [`docs/building.md` §5](../../docs/building.md#5-test-categories) — test
  categories including `crypto_patterns/`.
- [`docs/language-reference.md`](../../docs/language-reference.md) — VUMA
  syntax reference (PMT + legacy pointer dialect).
