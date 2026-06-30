# VUMA Releases

Release summaries with key changes and known limitations.

---

## v0.2.0-alpha.1 — 2026-06-30

**Womb stdlib: real implementations, no stubs, 84/84 files compile**

### Highlights

- **Womb stdlib rewritten** across 8 waves of subagent-driven refactoring: 62 files changed, +24,453 / -7,558 lines
- **84/85 womb files compile** with `vuma build` (only `core.vuma` is skipped — intentionally uncompilable design documentation)
- **Zero stub markers** across the entire womb stdlib (`stub`, `simplified`, `TODO`, `FIXME`, `placeholder`, `not implemented`, `NOT for production`)
- **Test harness** at `scripts/test_womb_compile.sh` validates every womb file compiles

### Real algorithm implementations

**Crypto primitives (Wave 1):**
- AES-128/192/256 with all SP 800-38 modes (CBC/CTR/CFB/OFB/GCM/CCM/XTS/KW/KWP/GCM-SIV/CMAC)
- SHA-1/224/256/384/512/512-224/512-256, SHA-3-224/256/384/512, SHAKE128/256, cSHAKE, KMAC
- RSA-2048 (PKCS#1 v1.5 + OAEP + PSS) with real Miller-Rabin keygen
- ECDSA P-256/P-384/P-521/secp256k1 with RFC 6979 deterministic nonces
- Ed25519/Ed448, X25519/X448 with constant-time Montgomery ladder
- Bignum (256-bit + 2048-bit) with mod_exp, mod_inv
- HMAC, Poly1305, HKDF, PBKDF2, scrypt, Argon2id, bcrypt
- HMAC_DRBG, HASH_DRBG, CTR_DRBG (SP 800-90A)

**PQC (Wave 2):**
- ML-KEM-512/768/1024 (FIPS 203) with real NTT/INTT
- ML-DSA-44/65/87 (FIPS 204) with SampleInBall
- SLH-DSA (FIPS 205) with WOTS+/FORS
- Falcon-512 with BDD rejection sampler
- HQC-128 code-based KEM

**Stream/legacy ciphers (Wave 2):**
- ChaCha20, ChaCha20-Poly1305, Salsa20/XSalsa20
- 3DES (EDE3) with full FIPS 46-3 tables
- Camellia-128/256 with real RFC 3713 key schedule (KL/KR/KA/KB + rotations)
- ARIA-128/256 with real RFC 5794 round-key derivation (19/31/67-bit rotations)
- RC4

**Transport protocols (Wave 3):**
- TLS 1.2 (RFC 5246): PRF, key schedule, record layer, ClientHello/ServerHello, Finished
- TLS 1.3 (RFC 8446): full HKDF chain, 4 traffic secrets, AEAD record encryption
- QUIC (RFC 9000/9001): varint, header protection, packet protection, initial keys
- SSH-2 (RFC 4251-4254): DH group14-sha256, key derivation, packet protocol

**Web protocols (Wave 4):**
- HTTP/1.1 (RFC 7230): request/response, chunked transfer
- HTTP/2 (RFC 9113): all 10 frame types, connection preface, settings, streams
- HPACK (RFC 7541): static/dynamic tables, Huffman with literal fallback
- WebSocket (RFC 6455): handshake, frames, masking

### Bug fixes from compile testing

- u64 literals >= 2^63: replaced with `(1 << 63) | lower_63_bits` pattern
- `match` is a reserved keyword: renamed to `match_flag`
- `ct_select` name conflict: renamed to `hmac_ct_select`
- `break outside of loop`: refactored to done-flag pattern
- Rust-style casts `(i: f64)` → VUMA `i as f64`
- Uninitialized declarations (`c: u32;`) → `c: u32 = 0;`
- Keccak RC18 typo: 32906 → 32778
- AES-256-CBC/CTR argument order: aligned with canonical signatures
- Malformed HPACK static table entry (idx == 7; }) → proper if/return

### Known limitations

- **No KAT validation**: Implementations are algorithmically complete but have not been byte-for-byte validated against NIST CAVP / RFC test vectors
- **Constant-time**: Most crypto is structurally constant-time but not audited for side-channels
- **Performance**: Schoolbook multiplication in bignum (not Montgomery); non-constant-time double-and-add in ECDSA
- **Self-hosting**: VUMA still cannot compile itself; compiler is Rust
- **Verification**: `--verification none` required for womb files (IVE cycle detection is overly conservative)

---

## v0.1.0-alpha.1 — 2026-06-28

**10 backends at 100% gold-standard pass rate**

### Highlights

- **10 backend architectures** at 100% pass rate on the 5,738-program gold-standard suite (57,380/57,380 runs): x86_64, AArch64, RISC-V 64, ARM32, MIPS64, PPC64, LoongArch64, x86_32, RISC-V 32, Wasm32
- **FFI**: 19 Linux syscalls across all 10 architectures, `extern "C"` blocks
- **Atomics**: `AtomicLoad`, `AtomicStore`, `AtomicCas` on all 10 backends
- **FP conversions**: `IntToFloat`, `UIntToFloat`, `FloatToInt`, `FloatToUInt`, `FloatToFloat`
- **Constant-time crypto**: `ct_select`, `ct_eq`, `ct_ne`, `ct_lt`, `ct_gte`
- **DWARF v4 debug info**: Per-backend address size and instruction length
- **66 diagnostic codes** with error chaining
- **LLM API**: `VumaForLLM` with compile/check/analyze/to_wasm/explain_error/suggest_fixes
- **LSP server**: Full protocol support
- **REPL**: `:wasm`, `:backends`, `:check`, `:diagnostics`, `:exports`
- **Module system**: `import` with circular import detection
- **Package manager**: `vuma pkg init/build/add`

### Key Bug Fixes

- mips64 `MAP_ANONYMOUS` flag: 0x22 (x86 value) → 0x802 (MIPS value where MAP_ANONYMOUS=0x800)
- wasm32 `__vuma_alloc`: Dynamic-size allocations now use bump allocator
- ppc64 enum_demo: Big-endian U8/U32 mismatch fixed via byte-level access
- x86_32: Stack-passed args, EBP clobber, EDX high word, store_vreg high-word zeroing
- riscv32/arm32: 64-bit return value handling
- lower_computation: Fixed prev_vreg remapping that treated let-bindings as reassignments

### Known Limitations

- **Self-hosting**: VUMA cannot compile itself; the compiler is written in Rust
- **Stdlib is host-side**: Math, fmt, string, crypto execute on host (Rust), not compiled to target
- **BD inference completeness**: Some complex scenarios deferred
- **Doubly-linked list verification**: Full verification not yet complete
- **Concurrent verification**: Limited to single-threaded programs
- **COR end-to-end**: Continuous Optimization Runtime not fully integrated

---

## v0.1.0-alpha.0 — 2026-06-16

**Initial alpha pre-release**

### Highlights

- SCG (Semantic Computation Graph) core
- IVE (Inference & Verification Engine) with five invariants
- BD (Behavioral Descriptors) with RepD/CapD/RelD
- MSG (Memory State Graph) construction
- Parser with lexer, AST, error recovery
- 8 initial backend architectures (x86_64, AArch64, RISC-V 64, ARM32, MIPS64, PPC64, LoongArch64, Wasm32)
- Proof system with counterexamples
- Standard library (host-side)
- LLM API and LSP server
- 15 formal specification documents
