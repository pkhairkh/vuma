# PMT Migration Plan — `womb/crypto/` Standard Library

**Task ID:** W50
**Scope:** Audit + migration plan (documentation only — no code changes in this task)
**Date:** 2026-07-19
**Author:** General-purpose sub-agent (W50 audit)
**Status:** Plan complete; migration deferred to follow-on waves (W51+)

---

## 1. Executive Summary

A full audit of `womb/crypto/` was performed against the PMT (Pointer-Managed
Transitions / "PMT-pure") discipline established by the W48 `womb/kernel/crypto/sha.vuma`
and W49 `womb/kernel/crypto/aes.vuma` reference implementations.

**Findings:**

| Metric | Value |
|---|---|
| Total `.vuma` files under `womb/crypto/` | **44** |
| Files matching legacy pointer syntax (`*ptr` / `allocate(` / `free(`) | **44 / 44 (100%)** |
| Files already PMT-pure | **0 / 44 (0%)** |
| Total `*(ptr+offset)` occurrences across all 44 files | ~4,125 |
| Total `allocate(` occurrences across all 44 files | ~1,281 |
| Total `free(` occurrences across all 44 files | ~1,517 |
| Files that `allocate()` but never `free()` (latent leaks) | **2** (`aes256.vuma`, `des_rc4_aria_camellia.vuma`) |

**Conclusion:** Every cryptographic primitive in the VUMA stdlib is currently
written in legacy pointer style. The kernel-side PMT stubs
(`womb/kernel/crypto/{sha,aes,asym,api,hw_trampoline}.vuma`) are the only PMT-pure
crypto code in the tree. Migration of `womb/crypto/` is the prerequisite for
retiring the legacy `Address` / `allocate` / `free` / `*(ptr+off)` runtime path
entirely.

---

## 2. Audit Methodology

Step 1 — Broad sweep (per W50 task spec):

```
grep -r "\*ptr\|allocate(\|free(" womb/crypto/ --include="*.vuma" -l 2>/dev/null | wc -l
→ 44
```

All 44 files matched. Note: the literal pattern `*ptr` (deref of a variable
named `ptr`) does **not** appear in any file — the legacy idiom in `womb/crypto/`
is the parenthesised form `*(ptr + offset)`, plus `allocate(` and `free(`. The
grep alternation matched the latter two in every file.

Step 2 — Per-file occurrence counts were gathered for the three legacy
constructs, so migration effort can be sized per file:

```
for f in $(find womb/crypto -name "*.vuma" | sort); do
  pc=$(grep -o '\*(' "$f" | wc -l)   # *(ptr+offset) dereferences
  ac=$(grep -o 'allocate(' "$f" | wc -l)
  fc=$(grep -o 'free(' "$f" | wc -l)
  echo "$f|$pc|$ac|$fc"
done
```

(Per-file results are tabulated in §5 below.)

Step 3 — The top 5 critical files were inspected line-by-line to confirm the
specific legacy idioms in use and to map them onto PMT equivalents (see §4).

Step 4 — Cross-checked the existing PMT-pure reference implementations in
`womb/kernel/crypto/` to confirm the target idiom (State<T> field access,
`state_new(Layout)`, no `free`).

---

## 3. PMT-pure Definition (target state)

A `.vuma` file is **PMT-pure** iff **all** of the following hold:

1. **No `*ptr` / `*(ptr + offset)` dereferences.** Memory is accessed only
   through typed `State<Layout>` field reads/writes (`ctx.state[i]`,
   `W.w[i]`, `out.data[i]`, etc.) or through PMT load/store intrinsics
   (`__vuma_load_u32`, `__vuma_store_u32`, `__vuma_load_u64`,
   `__vuma_store_u64`) when reading from raw caller-provided byte buffers.
2. **No `allocate(N)` calls.** All arena allocation goes through
   `state_new(LayoutName)`, where `LayoutName` is a declared `layout` with
   fixed-size `[u8; N]` / `[u32; N]` / `[u64; N]` fields.
3. **No `free(ptr)` calls.** Arena lifetime is managed by the runtime
   (state objects are GC'd when the owning scope returns); explicit `free`
   is a no-op or removed entirely.
4. **No raw `Address`-typed parameters** for buffers that conceptually have
   structure. Use `State<Layout>` pass-by-reference instead. (Raw `Address`
   may be retained only at FFI boundaries — none exist in `womb/crypto/`.)
5. **IVE (Intermediates Verification Engine) passes.** `compile_dump … --verify`
   reports `IVE: Pass`.

**Reference implementations** (already in tree, already IVE-Pass):

| File | Wave | Algorithm | LOC |
|---|---|---|---|
| `womb/kernel/crypto/sha.vuma` | W48 | SHA-256 (FIPS-180-4, 64-round, real KAT-matching) | 610 |
| `womb/kernel/crypto/aes.vuma` | W49 | AES-128 (10-round cipher, SubBytes+ShiftRows+AddRoundKey) | 412 |

These two files are the **canonical templates** for the migration. Every
`womb/crypto/` file should end up structurally resembling one of them.

---

## 4. Top 5 Critical Files — Deep-dive

The W50 task specified the five most important crypto files. All five are
**legacy**, all five use `allocate(`/`free(`, and four of the five use the
`*(ptr+offset)` form extensively. None use the literal `*ptr` variable-deref
form.

### 4.1 `hash/sha256_sha224.vuma` — SHA-256 + SHA-224 (FIPS-180-4)

| Legacy construct | Count |
|---|---|
| `*(ptr+offset)` | 134 |
| `allocate(` | 20 |
| `free(` | 20 |

**Status: LEGACY.** Largest hash file (1,526 LOC). Allocates 4 distinct scratch
buffers (`wbuf`=256B message schedule, `ctx`=112/216B streaming context,
`tmp`=64B, `tb`=200B test buffer). Uses helper functions `load_u32_be` /
`store_u32_be` that internally dereference `*(ptr+off)` byte-by-byte and pack
into u32. The W48 `womb/kernel/crypto/sha.vuma` already implements a PMT-pure
SHA-256 with the same algorithm — **this file is the #1 migration candidate
because the reference implementation already exists**; the migration is largely
a port of the W48 kernel stub back into `womb/crypto/`, plus addition of the
SHA-224 variant (different IV, 28-byte truncated output).

### 4.2 `symmetric/aes128.vuma` — AES-128 (FIPS-197)

| Legacy construct | Count |
|---|---|
| `*(ptr+offset)` | 127 |
| `allocate(` | 12 |
| `free(` | 13 |

**Status: LEGACY.** 856 LOC. Allocates `roundkeys`=176B, `state`=16B,
`padded`/`prev`/`xored`/`ct_block`/`pt_block`=16B each. Block processing uses
`*(ptr + off)` for both S-box lookup and state-byte manipulation. The W49
`womb/kernel/crypto/aes.vuma` already implements a PMT-pure AES-128 with
SubBytes+ShiftRows+AddRoundKey — **same situation as SHA-256**: the kernel
stub is the reference, and the stdlib file should be ported to match. The W49
"Next actions" already enumerate the follow-ons (MixColumns, real Rijndael key
schedule, inverse cipher) — these are deferred to W51+ regardless of PMT
migration status.

### 4.3 `asym/ed25519.vuma` — Ed25519 (RFC 8032)

| Legacy construct | Count |
|---|---|
| `*(ptr+offset)` | 229 |
| `allocate(` | 67 |
| `free(` | 98 |

**Status: LEGACY.** 593 LOC. **Highest allocate/free count of any file in the
top 5.** Ed25519 is a heavy allocator because point arithmetic over the
curve 2^255-19 creates many transient 32-byte field-element buffers (`t1..t5`,
`num_x`, `num_y`, etc.), each allocated and freed per operation. The 98
`free(` calls exceed the 67 `allocate(` calls because some allocs have
multiple free paths (success + error branches). Migration here is the most
invasive of the top 5 — every field-element scratch variable becomes a
`State<Fe25519>` with a `[u64; 5]` or `[u32; 10]` field. **No kernel-side
PMT reference exists yet** for Ed25519; this file should be migrated **after**
the `bignum/` module (which provides the field-arithmetic primitives Ed25519
depends on).

### 4.4 `mac_kdf/hmac.vuma` — HMAC (RFC 2104 / FIPS 198-1)

| Legacy construct | Count |
|---|---|
| `*(ptr+offset)` | 44 |
| `allocate(` | 18 |
| `free(` | 18 |

**Status: LEGACY.** 193 LOC. The simplest of the top 5. Signature is
`fn hmac_sha1(key: Address, keylen: u32, msg: Address, msglen: u32, out: Address)`
— note the **raw `Address` parameters**, which all become `State<ByteBuf>`
(or dedicated layouts `State<HmacKey>`, `State<HmacMsg>`, `State<HmacOut>`)
under PMT. Allocates 6 scratch buffers per call (`kprime`, `kipad`, `kopad`,
`inner`, `inner_hash`, `outer`). HMAC depends directly on the underlying hash
(SHA-1, SHA-256, SHA-512), so it should be migrated **immediately after**
`hash/`. The W48 worklog entry already flagged HMAC-SHA-256 as a natural
follow-on to the W48 SHA-256 landing — migration order is unambiguous.

### 4.5 `symmetric/chacha20_poly1305.vuma` — ChaCha20-Poly1305 AEAD (RFC 8439)

| Legacy construct | Count |
|---|---|
| `*(ptr+offset)` | 21 |
| `allocate(` | 6 |
| `free(` | 6 |

**Status: LEGACY.** 199 LOC. Smallest of the top 5 by both LOC and legacy
construct count. Allocates `zero`=32B, `poly_key`=32B, `mac_input`/`computed_tag`
variable-length. Migrates cleanly: each buffer becomes a `State<ChachaBuf>`
/ `State<PolyKey>` / `State<PolyTag>` layout. Depends on `chacha20.vuma`
(upstream) and `poly1305.vuma` (upstream) — both of which must be migrated
first (or in the same wave). The AEAD construction itself is trivial once the
two underlying primitives are PMT-pure.

### Top-5 Summary Table

| File | LOC | `*(` | `alloc(` | `free(` | Status | Migration Phase |
|---|---|---|---|---|---|---|
| `hash/sha256_sha224.vuma` | 1526 | 134 | 20 | 20 | LEGACY | Phase 1 (hash) |
| `symmetric/aes128.vuma` | 856 | 127 | 12 | 13 | LEGACY | Phase 2 (sym) |
| `asym/ed25519.vuma` | 593 | 229 | 67 | 98 | LEGACY | Phase 5 (asym) |
| `mac_kdf/hmac.vuma` | 193 | 44 | 18 | 18 | LEGACY | Phase 4 (mac_kdf) |
| `symmetric/chacha20_poly1305.vuma` | 199 | 21 | 6 | 6 | LEGACY | Phase 2 (sym) |

---

## 5. Full File Inventory — All 44 Files

Every file below is **LEGACY** (uses `*(ptr+offset)`, `allocate(`, and/or
`free(`). None are PMT-pure. Counts are occurrence counts (`grep -o | wc -l`),
not line counts — a single line with `free(x); free(y); free(z);` contributes 3
to the `free(` column.

### 5.1 `hash/` — 8 files (Phase 1: migrate first)

| File | Algorithm | LOC | `*(` | `alloc(` | `free(` |
|---|---|---|---|---|---|
| `hash/sha1.vuma` | SHA-1 | 206 | 13 | 2 | 2 |
| `hash/md5.vuma` | MD5 (legacy) | 284 | 13 | 2 | 2 |
| `hash/sha256_sha224.vuma` | SHA-256 + SHA-224 | 1526 | 134 | 20 | 20 |
| `hash/sha384.vuma` | SHA-384 | 414 | 21 | 2 | 2 |
| `hash/sha512.vuma` | SHA-512 | 414 | 21 | 2 | 2 |
| `hash/sha3.vuma` | SHA-3 (Keccak) | 419 | 52 | 5 | 5 |
| `hash/blake2.vuma` | BLAKE2 | 288 | 29 | 4 | 4 |
| `hash/blake3.vuma` | BLAKE3 | 541 | 47 | 26 | 26 |
| **subtotal** | | **4192** | **330** | **63** | **63** |

### 5.2 `symmetric/` — 11 files (Phase 2)

| File | Algorithm | LOC | `*(` | `alloc(` | `free(` |
|---|---|---|---|---|---|
| `symmetric/chacha20.vuma` | ChaCha20 | 229 | 16 | 4 | 4 |
| `symmetric/salsa20.vuma` | Salsa20 | 362 | 17 | 10 | 10 |
| `symmetric/poly1305.vuma` | Poly1305 MAC | 278 | 100 | 14 | 14 |
| `symmetric/chacha20_poly1305.vuma` | ChaCha20-Poly1305 AEAD | 199 | 21 | 6 | 6 |
| `symmetric/aes128.vuma` | AES-128 | 856 | 127 | 12 | 13 |
| `symmetric/aes192.vuma` | AES-192 | 281 | 55 | 12 | 13 |
| `symmetric/aes256.vuma` | AES-256 | 223 | 126 | 11 | **0** ⚠ |
| `symmetric/aes_modes.vuma` | AES CBC/CTR/XTS/KW/CMAC | 508 | 114 | 31 | 36 |
| `symmetric/aes_cfb_ofb.vuma` | AES CFB/OFB | 343 | 50 | 14 | 14 |
| `symmetric/aes_extra_modes.vuma` | AES GCM/CCM/EAX/OCB | 1001 | 200 | 54 | 66 |
| `symmetric/des_rc4_aria_camellia.vuma` | DES/RC4/ARIA/Camellia | 3424 | 190 | 30 | **0** ⚠ |
| **subtotal** | | **7704** | **1016** | **198** | **176** |

⚠ `aes256.vuma` and `des_rc4_aria_camellia.vuma` call `allocate(` but never
`free(` — see §7 (Special Cases).

### 5.3 `asym/` — 9 files (Phase 5)

| File | Algorithm | LOC | `*(` | `alloc(` | `free(` |
|---|---|---|---|---|---|
| `asym/ed25519.vuma` | Ed25519 | 593 | 229 | 67 | 98 |
| `asym/x25519.vuma` | X25519 | 554 | 53 | 32 | 32 |
| `asym/ecdh_p256.vuma` | ECDH on P-256 | 178 | 10 | 13 | 29 |
| `asym/ecdsa_p256.vuma` | ECDSA on P-256 | 607 | 186 | 83 | 115 |
| `asym/ecdsa_p384.vuma` | ECDSA on P-384 | 782 | 304 | 91 | 123 |
| `asym/secp256k1.vuma` | secp256k1 (Bitcoin) | 473 | 154 | 89 | 142 |
| `asym/rsa.vuma` | RSA (PKCS#1 v1.5) | 559 | 72 | 45 | 58 |
| `asym/rsa_oaep_pss.vuma` | RSA-OAEP + RSA-PSS | 520 | 71 | 35 | 35 |
| `asym/rsa_pkcs1_ecdsa_extra.vuma` | RSA PKCS#1 + ECDSA extras | 1537 | 588 | 174 | 219 |
| **subtotal** | | **5803** | **1667** | **629** | **851** |

### 5.4 `mac_kdf/` — 7 files (Phase 4)

| File | Algorithm | LOC | `*(` | `alloc(` | `free(` |
|---|---|---|---|---|---|
| `mac_kdf/hmac.vuma` | HMAC | 193 | 44 | 18 | 18 |
| `mac_kdf/hkdf.vuma` | HKDF | 151 | 18 | 8 | 8 |
| `mac_kdf/pbkdf2.vuma` | PBKDF2 | 203 | 45 | 12 | 12 |
| `mac_kdf/scrypt.vuma` | scrypt | 241 | 37 | 9 | 9 |
| `mac_kdf/argon2.vuma` | Argon2 | 437 | 104 | 12 | 12 |
| `mac_kdf/cmac_bcrypt_kdf.vuma` | CMAC / bcrypt / KDF | 602 | 59 | 24 | 24 |
| `mac_kdf/key_agreement.vuma` | Key agreement protocols | 757 | 309 | 53 | 59 |
| **subtotal** | | **2584** | **616** | **136** | **142** |

### 5.5 `bignum/` — 2 files (Phase 3: before asym)

| File | Description | LOC | `*(` | `alloc(` | `free(` |
|---|---|---|---|---|---|
| `bignum/bignum.vuma` | 1024-bit big number arithmetic | 508 | 41 | 16 | 17 |
| `bignum/bignum2048.vuma` | 2048-bit big number arithmetic | 689 | 51 | 28 | 33 |
| **subtotal** | | **1197** | **92** | **44** | **50** |

### 5.6 `drbg/` — 2 files (Phase 6)

| File | Algorithm | LOC | `*(` | `alloc(` | `free(` |
|---|---|---|---|---|---|
| `drbg/drbg.vuma` | HMAC-DRBG (NIST SP 800-90A) | 191 | 44 | 8 | 8 |
| `drbg/drbg_extra.vuma` | Hash-DRBG + CTR-DRBG | 372 | 30 | 21 | 21 |
| **subtotal** | | **563** | **74** | **29** | **29** |

### 5.7 `post_quantum/` — 5 files (Phase 7: last — most complex)

| File | Algorithm | LOC | `*(` | `alloc(` | `free(` |
|---|---|---|---|---|---|
| `post_quantum/ml_kem.vuma` | ML-KEM (Kyber, FIPS 203) | 1343 | 86 | 55 | 57 |
| `post_quantum/ml_dsa.vuma` | ML-DSA (Dilithium, FIPS 204) | 584 | 70 | 40 | 38 |
| `post_quantum/slh_dsa.vuma` | SLH-DSA (SPHINCS+, FIPS 205) | 302 | 54 | 17 | 17 |
| `post_quantum/falcon.vuma` | Falcon | 905 | 26 | 27 | 51 |
| `post_quantum/hqc.vuma` | HQC | 1104 | 94 | 43 | 43 |
| **subtotal** | | **4238** | **330** | **182** | **206** |

### 5.8 Grand Totals

| Category | Files | LOC | `*(` | `alloc(` | `free(` |
|---|---|---|---|---|---|
| hash | 8 | 4,192 | 330 | 63 | 63 |
| symmetric | 11 | 7,704 | 1,016 | 198 | 176 |
| mac_kdf | 7 | 2,584 | 616 | 136 | 142 |
| bignum | 2 | 1,197 | 92 | 44 | 50 |
| asym | 9 | 5,803 | 1,667 | 629 | 851 |
| drbg | 2 | 563 | 74 | 29 | 29 |
| post_quantum | 5 | 4,238 | 330 | 182 | 206 |
| **TOTAL** | **44** | **26,281** | **4,125** | **1,281** | **1,517** |

---

## 6. Migration Priority / Phasing

The W50 task spec directs: **hash first, then symmetric, then asym**. The
remaining four categories (mac_kdf, bignum, drbg, post_quantum) are slotted
by dependency.

| Phase | Category | Files | Rationale |
|---|---|---|---|
| **1** | `hash/` | 8 | Foundation — no dependencies. Every other category depends on at least one hash. W48 `kernel/crypto/sha.vuma` is the PMT-pure reference for SHA-256, so `sha256_sha224.vuma` is the lowest-risk starting point. |
| **2** | `symmetric/` | 11 | Standalone (no upstream crypto deps). W49 `kernel/crypto/aes.vuma` is the PMT-pure reference for AES-128. ChaCha20/Poly1305/Salsa20 are simple stream/MAC primitives with clean PMT mappings. |
| **3** | `bignum/` | 2 | Prerequisite for `asym/`. 1024/2048-bit multi-precision arithmetic underpins RSA, ECDSA, ECDH, and all PQ schemes. Migrate before asym so asym can use PMT-pure bignum primitives. |
| **4** | `mac_kdf/` | 7 | Depends on `hash/` (HMAC/HKDF/PBKDF2/scrypt/Argon2 all call into SHA-1/256/512). Slot after hash so the hash APIs are already PMT-pure. |
| **5** | `asym/` | 9 | Most complex. Depends on `bignum/` (Phase 3) and `hash/` (Phase 1). Ed25519/X25519 use a custom 2^255-19 field (can reuse bignum or have their own FE layout). RSA/ECDSA need bignum modular exponentiation. |
| **6** | `drbg/` | 2 | Depends on `hash/` + `symmetric/` (HMAC-DRBG uses HMAC; CTR-DRBG uses AES). Slot after both are done. |
| **7** | `post_quantum/` | 5 | Most complex. ML-KEM/ML-DSA/SLH-DSA/Falcon/HQC depend on hash (SHAKE/SHA-2/SHA-3), symmetric (AES for some), bignum, and polynomial arithmetic (NTT). Migrate last. |

**Within each phase**, suggested intra-phase ordering (easiest → hardest):

- **Phase 1 (hash):** `sha1` → `md5` → `sha384` → `sha512` → `sha256_sha224` → `sha3` → `blake2` → `blake3`. (SHA-256 has the W48 reference — port first after the trivial SHA-1/MD5 warm-ups. SHA-3/BLAKE2/BLAKE3 have sponge/tree structures needing dedicated layouts.)
- **Phase 2 (sym):** `chacha20` → `salsa20` → `poly1305` → `chacha20_poly1305` → `aes128` → `aes192` → `aes256` → `aes_modes` → `aes_cfb_ofb` → `aes_extra_modes` → `des_rc4_aria_camellia`. (ChaCha/Poly first — simple; AES-128 has the W49 reference; modes build on the block cipher; the legacy DES/RC4/ARIA/Camellia conglomerate is last because it's 3,424 LOC of mixed ciphers.)
- **Phase 3 (bignum):** `bignum` (1024-bit) → `bignum2048` (2048-bit extends 1024).
- **Phase 4 (mac_kdf):** `hmac` → `hkdf` → `pbkdf2` → `scrypt` → `argon2` → `cmac_bcrypt_kdf` → `key_agreement`. (HMAC is the base; HKDF/PBKDF2 build on HMAC; scrypt/Argon2 are memory-hard and the most invasive.)
- **Phase 5 (asym):** `ed25519` → `x25519` → `ecdh_p256` → `ecdsa_p256` → `ecdsa_p384` → `secp256k1` → `rsa` → `rsa_oaep_pss` → `rsa_pkcs1_ecdsa_extra`. (Curve25519 family first — self-contained field; NIST curves next; RSA last because it needs the most bignum work.)
- **Phase 6 (drbg):** `drbg` (HMAC-DRBG) → `drbg_extra` (Hash/CTR-DRBG).
- **Phase 7 (PQ):** `slh_dsa` (hash-only, simplest PQ) → `ml_dsa` → `ml_kem` → `falcon` → `hqc`.

---

## 7. Migration Strategy — Mechanical Transformations

This section documents the three concrete code transformations required to
convert a legacy `womb/crypto/` file to PMT-pure. **No file is migrated in
W50 — these are the recipes for the follow-on waves.**

### 7.1 `*(ptr + offset)` → typed load/store

The legacy idiom dereferences a byte at `ptr + offset`:

```vuma
// LEGACY
b: u32 = *(ptr + off);              // byte load
*(ptr + off) = (val >> 24) & 255;   // byte store
w: u32 = load_u32_be(buf, i * 4);   // helper that internally does 4× *(buf+off+k)
```

**PMT target — two equivalent forms:**

**(a) Preferred: typed `State<Layout>` field access** (matches W48/W49 kernel stubs):

```vuma
// PMT-pure — field access
b: u32 = ctx.buf[off];              // byte-granular index on [u8; N]
ctx.buf[off] = (val >> 24) & 255;
w: u32 = W.w[i];                    // typed-array scaling on [u32; N] — reads u32 at slot i
```

This requires declaring a `layout` with fixed-size array fields sized to the
buffer's maximum extent (e.g. `layout Sha256Ctx = { state: [u32; 8], buf: [u8; 64], len: u32, total: u64 }`).

**(b) Fallback: PMT load/store intrinsics** (when the buffer is a raw
caller-provided byte span not owned by a State<T>, e.g. at the FFI edge —
none currently exist in `womb/crypto/`):

```vuma
// PMT-pure — intrinsics
b: u32 = __vuma_load_u8(buf, off);
__vuma_store_u8(buf, off, (val >> 24) & 255);
w: u32 = __vuma_load_u32(buf, i * 4);   // little-endian by default
w_be: u32 = __vuma_load_u32_be(buf, i * 4);  // if BE variant exists
```

The W48/W49 kernel stubs demonstrate that form (a) suffices for all internal
crypto state — **form (b) is only needed if/when raw `Address` parameters
must be retained at a public API boundary**. The recommendation is to
eliminate raw `Address` entirely from `womb/crypto/` and use form (a)
everywhere.

**Endianness note:** SHA-2 family loads/stores u32 in **big-endian** byte
order (FIPS-180-4 §5.1.1/§5.1.2); ChaCha20/Poly1305 use **little-endian**
(RFC 8439 §2.3). The legacy `load_u32_be` / `store_u32_be` helpers exist
precisely because `*(ptr+off)` is byte-granular and endianness must be
encoded in the helper. Under PMT, `State<[u8;N]>` field access is still
byte-granular, so the helpers are still needed — but they should be
reimplemented to take `State<Layout>` instead of `Address`, and centralized
in a shared `womb/kernel/crypto/bytes.vuma` (per W48 "Next actions" item 5).

### 7.2 `allocate(N)` → `state_new(Layout)`

The legacy idiom allocates N raw bytes:

```vuma
// LEGACY
wbuf = allocate(256);          // 64-word SHA-256 message schedule
ctx = allocate(112);           // SHA-256 streaming context
roundkeys = allocate(176);     // AES-128 expanded key (11 × 16 bytes)
```

**PMT target:**

```vuma
// PMT-pure
layout Sha256W = { w: [u32; 64] }                    // 256 bytes
layout Sha256Ctx = { state: [u32; 8], buf: [u8; 64], len: u32, total: u64 }
layout AesKeys = { rk: [u8; 176] }

let W = state_new(Sha256W);
let ctx = state_new(Sha256Ctx);
let roundkeys = state_new(AesKeys);
```

Every `allocate(N)` becomes a `state_new(LayoutName)` where the layout
declares the same total byte count as structured fields. Variable-length
allocations (`allocate(block_size + msglen)` in HMAC) require either:

- a fixed maximum-size layout (`layout HmacInner = { data: [u8; MAX_BLOCK + MAX_MSG] }`) with a separate length field, **or**
- restructuring the algorithm to stream the data through a fixed-size block buffer (preferred — matches the SHA-256 streaming pattern in W48).

### 7.3 `free(ptr)` → no-op (removed)

The legacy idiom explicitly frees memory:

```vuma
// LEGACY
free(wbuf);
free(ctx);
free(roundkeys); free(state);
```

**PMT target:** remove the `free` calls entirely. The PMT arena manages
object lifetime by scope — when the function returns, all `state_new`-ed
objects become unreachable and are GC'd. There is no `free` primitive in
the PMT runtime.

```vuma
// PMT-pure — nothing. The free() calls are deleted.
```

**Migration tactic:** `free(x);` lines are simply removed. Paired frees
(`free(a); free(b); free(c);` at function exit) are removed as a block.
Conditional frees in error paths (`if err { free(x); return -1; }`) are
removed along with their guard — the early `return` alone is sufficient.

### 7.4 Additional PMT transformation: `Address` parameters → `State<Layout>`

Not part of the W50 task's stated strategy but required for true PMT purity:

```vuma
// LEGACY
fn hmac_sha1(key: Address, keylen: u32, msg: Address, msglen: u32, out: Address)

// PMT-pure
fn hmac_sha1(key: State<ByteBuf>, keylen: u32,
             msg: State<ByteBuf>, msglen: u32,
             out: State<ByteBuf>)
```

Or, more typed:

```vuma
layout HmacKey   = { data: [u8; MAX_KEY] }
layout HmacMsg   = { data: [u8; MAX_MSG] }
layout HmacOut   = { data: [u8; MAX_DIGEST] }
fn hmac_sha1(key: State<HmacKey>, keylen: u32, msg: State<HmacMsg>, msglen: u32, out: State<HmacOut>)
```

---

## 8. Special Cases

### 8.1 Files that `allocate()` but never `free()` — latent leaks

Two files call `allocate(` but have **zero** `free(` calls:

| File | `alloc(` | `free(` | Notes |
|---|---|---|---|
| `symmetric/aes256.vuma` | 11 | 0 | AES-256 cipher. All 11 allocations are leaked on every call — either an oversight or the file relies on process exit to reclaim. Under PMT this is **automatically fixed** (arena GC reclaims when scope returns). |
| `symmetric/des_rc4_aria_camellia.vuma` | 30 | 0 | DES/RC4/ARIA/Camellia conglomerate (3,424 LOC). Same situation — 30 allocations leaked per call. PMT migration fixes this as a side effect. |

These two files are the strongest **correctness** argument for migration
(even setting aside PMT discipline): every cipher invocation currently leaks
11–30 buffers. Under the legacy runtime this is tolerable for short-lived
processes but fatal for long-running ones (e.g. a TLS server). The PMT
arena eliminates the leaks without any explicit free logic.

### 8.2 `free(` count > `allocate(` count

Several files have more `free(` calls than `allocate(` calls (e.g.
`ed25519.vuma`: 67 alloc / 98 free; `ecdsa_p384.vuma`: 91 alloc / 123 free;
`rsa_pkcs1_ecdsa_extra.vuma`: 174 alloc / 219 free). This is because the
same allocation is freed on multiple code paths (success return + multiple
error returns). Under PMT, **all** these free calls are removed — the arena
handles reclamation uniformly regardless of which return path is taken.
This also eliminates a class of double-free and use-after-free bugs that
the legacy pattern is structurally vulnerable to.

### 8.3 `load_u32_be` / `store_u32_be` / `load_u32_le` helpers

Many `womb/crypto/` files define local byte-pack/unpack helpers that wrap
`*(ptr+off)`:

```vuma
fn load_u32_be(buf: Address, off: u32) -> u32 {
    b0: u32 = *(buf + off);
    b1: u32 = *(buf + off + 1);
    b2: u32 = *(buf + off + 2);
    b3: u32 = *(buf + off + 3);
    return (b0 << 24) | (b1 << 16) | (b2 << 8) | b3;
}
```

Under PMT, these helpers should be:

1. Reimplemented to take `State<Layout>` (or raw `Address` if kept at an FFI edge — none in `womb/crypto/`).
2. **Centralized** in a shared `womb/kernel/crypto/bytes.vuma` module to eliminate the per-file duplication (the same helper exists verbatim in `sha256_sha224.vuma`, `hmac.vuma`, `aes128.vuma`, and at least 8 other files). The W48 worklog already flagged this as "Next actions" item 5.

### 8.4 Variable-length allocations

`mac_kdf/hmac.vuma` allocates `inner = allocate(block_size + msglen)` where
`msglen` is a runtime parameter. PMT layouts require compile-time-constant
sizes. Two resolution paths:

- **Stream the data** through a fixed-size block buffer (preferred — matches
  the SHA-256 streaming API: `init` / `update` / `final`). HMAC's `inner` is
  just `kipad || msg`; instead of materializing the concatenation, call
  `sha_update(ctx, kipad, block_size)` then `sha_update(ctx, msg, msglen)`.
  This eliminates the variable-length allocation entirely.
- **Max-size layout** with a separate length field (`layout HmacInner = { data: [u8; MAX_HMAC_INPUT], len: u32 }`). Simpler but wastes memory and
  requires choosing `MAX_HMAC_INPUT`.

The streaming approach is strongly preferred — it aligns with how the W48
SHA-256 reference already works.

---

## 9. Risk Assessment

| Risk | Severity | Mitigation |
|---|---|---|
| Algorithm regressions during port | **High** | Each migrated file must retain its existing KAT (Known Answer Test) vectors. Run `scripts/run_all_kat.sh` before and after migration — byte-identical digests/ciphertexts required. |
| Variable-length alloc → fixed-layout overflow | Medium | Prefer streaming APIs (§8.4). For any fixed-max layout, add an explicit length check + error return. |
| Endianness errors in u32/u64 pack helpers | Medium | Keep the existing `load_u32_be`/`store_u32_be` semantics verbatim — only change the parameter type from `Address` to `State<Layout>`. Add KAT vectors that exercise BE/LE boundaries. |
| Performance regression from arena GC vs. manual free | Low | The PMT arena is bump-allocated and GC'd by scope; for crypto workloads (small fixed-size buffers, short-lived scopes) this is at worst parity with `malloc`/`free`. The 2 currently-leaking files (`aes256`, `des_rc4_aria_camellia`) will actually **improve** (no unbounded growth). |
| Mass-migration introduces bugs across many files at once | High | **Migrate one file per wave** (W51 = sha256_sha224, W52 = aes128, etc.). Each wave ships independently with its own IVE check + KAT run. Do not batch. |
| `bignum/` migration blocks `asym/` | Medium | Sequence Phase 3 (bignum) strictly before Phase 5 (asym). If bignum slips, asym slips — do not migrate asym against legacy bignum (creates a PMT↔legacy bridge that defeats the purpose). |

---

## 10. Estimated Effort

Rough sizing, assuming one experienced implementer working full-time on
the VUMA crypto stack. Per-file effort scales with LOC × legacy-construct
density, with a premium for algorithmic complexity (PQ > asym > KDF > sym > hash).

| Phase | Category | Files | LOC | Est. waves | Est. person-days |
|---|---|---|---|---|---|
| 1 | hash | 8 | 4,192 | 8 (1/file) | 10–14 |
| 2 | symmetric | 11 | 7,704 | 11 (1/file) | 16–22 |
| 3 | bignum | 2 | 1,197 | 2 | 4–6 |
| 4 | mac_kdf | 7 | 2,584 | 7 | 10–14 |
| 5 | asym | 9 | 5,803 | 9 | 18–25 |
| 6 | drbg | 2 | 563 | 2 | 3–4 |
| 7 | post_quantum | 5 | 4,238 | 5 | 12–18 |
| **Total** | | **44** | **26,281** | **44** | **73–103** |

Assuming one wave per week, the full migration is a **~44-week** effort
(roughly 11 months at one file per week). This is consistent with the
existing cadence (W48 = SHA-256, W49 = AES-128 — each wave ports one
algorithm with full IVE + KAT verification).

**Acceleration options:**
- Files with PMT-pure kernel-side references (`sha256_sha224` ← W48, `aes128` ← W49) can be ported in **half** the estimated time — the algorithm is already worked out, only the API surface (stdlib vs. kernel) differs.
- The legacy `des_rc4_aria_camellia.vuma` conglomerate (3,424 LOC, 4 ciphers) could be **split** into separate `des.vuma`, `rc4.vuma`, `aria.vuma`, `camellia.vuma` files during migration — improves maintainability but adds a renaming/naming-convention wave.
- Trivial files (`sha1`, `md5`, `sha384`, `sha512` — all <500 LOC, ≤2 allocs) could be **batched 2-per-wave** to compress Phase 1 from 8 waves to 4.

---

## 11. Acceptance Criteria (per file)

A `womb/crypto/` file is considered **migrated to PMT-pure** when **all** of
the following are true:

1. `grep -E '\*ptr|\*\(.*\+|allocate\(|free\(' <file>` returns **zero matches** (excluding comments).
2. `grep -E 'Address' <file>` returns **zero matches** in function signatures (raw `Address` only permitted in FFI shims — none in `womb/crypto/`).
3. Every buffer is a `State<Layout>` with declared fixed-size array fields.
4. Every allocation uses `state_new(Layout)`.
5. `compile_dump <file> /tmp/<name>.bin x86_64 --verify` reports `IVE: Pass`.
6. The file's KAT vectors (run via `scripts/run_all_kat.sh`) produce byte-identical output to the pre-migration version.
7. The file header comment is updated to document the PMT-pure status (replace "legacy pointer syntax" with "PMT-pure (State<T> + state_new)") and reference the wave that performed the migration.

---

## 12. Out of Scope (for W50)

This W50 task is **audit + plan only**. The following are explicitly **not**
done in W50 and are deferred:

- Migrating any of the 44 files (deferred to W51+).
- Creating the shared `womb/kernel/crypto/bytes.vuma` helper module (W48 "Next actions" item 5; belongs in a follow-on wave).
- Splitting `des_rc4_aria_camellia.vuma` into 4 files (optional acceleration, defer to the Phase 2 wave that touches it).
- Implementing the deferred AES components (MixColumns, real Rijndael key schedule, inverse cipher — W49 "Next actions" items 1–4). These are **algorithm** gaps, not PMT gaps; they are orthogonal to this migration plan and should be addressed in their own waves regardless of PMT status.
- Adding new KAT vectors. The existing vectors in `scripts/run_all_kat.sh` are the regression baseline; no new vectors are added by this plan.

---

## 13. Next Actions

1. **W51:** Migrate `womb/crypto/hash/sha256_sha224.vuma` to PMT-pure by porting the W48 `womb/kernel/crypto/sha.vuma` reference. Add the SHA-224 variant (different IV, 28-byte truncated output). Acceptance: §11 criteria 1–7, KAT-matching against pre-migration output. This is the single highest-value migration because the reference implementation already exists.
2. **W52:** Migrate `womb/crypto/symmetric/aes128.vuma` by porting the W49 `womb/kernel/crypto/aes.vuma` reference. Acceptance: §11 criteria 1–7.
3. **W53:** Create `womb/kernel/crypto/bytes.vuma` shared load/store helpers (`load_le_u32`, `store_le_u32`, `load_be_u32`, `store_be_u32`, and u64 variants). Refactor W48/W49 stubs to use them. This unblocks all subsequent hash/sym migrations.
4. **W54–W57:** Continue Phase 1 (hash) — `sha1`, `md5`, `sha384`, `sha512` (batch 2-per-wave where possible).
5. **W58+:** Phase 2 (symmetric) onward, following the intra-phase ordering in §6.

**Reviewers:** this document should be revisited at the start of each
follow-on wave to confirm the phase ordering still reflects dependency
reality (e.g. if a new bignum primitive lands earlier than expected, Phase 3
can be pulled forward).

---

*End of W50 PMT Migration Plan.*
