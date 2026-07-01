# VUMA Releases

Release summaries with key changes and known limitations.

---

## v0.2.0-alpha.1 — 2026-06-30

### What Changed

- **Womb stdlib expanded** to 115 .vuma files (~65K lines). 114 compile on x86_64 with `--verification none`; `womb/core.vuma` is an explicit design spec and is not compilable.
- **Heap allocation**: `__vuma_alloc` (mmap on 9/10 backends, bump allocator on Wasm32) and `__vuma_free` syscall stubs work across function calls on all 10 backends.
- **Language features added**: match expressions, struct field access, enum tagged unions, import system, for-range loops, break/continue, type annotations, dereference in function arguments, function calls in expressions.
- **Compiler bug fixes**: const reference resolution, if-body returns, function calls in loops, deep call chains, CRC32 polynomial, SHA-256 K[1] constant.
- **10-backend codegen**: 99.99% gold-standard pass rate (57,377/57,380 runs with `--verification none`). 3 failures: `crc32.vuma` on riscv64+ppc64, `s27_fn_two_args_mod.vuma` on ppc64.

### Womb Modules

114 of 115 womb files compile on x86_64 (`core.vuma` is a design spec). Categories include:
- **Collections**: DynamicVec (heap-backed), HashMap, BTreeMap, EnumMap
- **Containers**: generic container abstractions (1 file, undocumented in prior releases)
- **Strings**: UTF-8 VStr with grow, StringBuilder (3 modules across `string/` and `lib/`)
- **File I/O**: raw syscall wrappers, high-level read_file/write_file
- **Graph**: heap-backed digraph with dynamic grow, topological sort, cycle detection
- **IEEE**: floating-point and IEEE frame helpers (2 files, undocumented in prior releases)
- **Language**: 15 files including full_lexer, full_parser, ir_builder, codegen, elf, plus `vuma_compiler.vuma` (506 LOC full self-hosting pipeline)
- **Crypto**: 45 modules (sha1/3/384/512 + sha_variants, aes128/192/256 + modes, hmac, chacha20, poly1305, rsa+oaep/pss, ecdsa p256/p384, ed25519, x25519, secp256k1, ml_dsa, ml_kem, slh_dsa, falcon, hqc, bignum/bignum2048, blake2/blake3, md5, crc, hkdf, pbkdf2, scrypt, argon2, drbg, salsa20, and more)
- **Encoding**: Base64, hex, URL
- **Network (womb/net/)**: TCP, SSH, QUIC, TLS 1.2, TLS 1.3 (5 files)
- **Library (womb/lib/)**: 28 files including DNS, HTTP/HTTP2, WebSocket, email (SMTP), app_protocols (MQTT), JSON, X.509, PKI, JWT, ASN.1, HPACK, deflate, threading, event_loop, math, stdio, time, socket

### Known Limitations

- **Self-hosting**: Started but not complete. `src/bootstrap/vuma_compiler.vuma` (730 LOC lexer POC) and `womb/lang/vuma_compiler.vuma` (506 LOC full pipeline) exist but are not verified end-to-end.
- **Verification**: `--verification normal` has false positives. Most programs use `--verification none`.
- **Standard library**: `vuma-std` Rust crate not linked. Womb modules exist but aren't auto-imported.
- **BD inference (M2.3)**: Complex generic inference deferred.
- **Concurrent verification**: Single-threaded only.
- **Type checking**: Not implemented (parser recognizes syntax but doesn't validate types).
- **While-loop variable tracking**: Known compiler bug when loop body calls functions.
- **CLI ISA coverage**: `vuma emit`/`vuma compile` accept 8 ISAs (missing RISC-V 32, x86_32).
- **3 test failures**: crc32 on riscv64/ppc64, s27_fn_two_args_mod on ppc64.

---

## v0.1.0-alpha.1 — 2026-06-28

Initial alpha release.

### What Worked

- 10 backend architectures at ~100% gold-standard pass rate (57,377/57,380 with `--verification none`)
- Parser, SCG, IVE, BD, codegen, FFI, atomics, DWARF v4 debug info
- 5,738-program gold-standard test suite
- LLM API, LSP server, REPL, package manager
