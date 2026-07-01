# VUMA Changelog

All notable changes to the VUMA project.

---

## [0.2.0-alpha.1] — 2026-06-30

### Added

- **Heap allocation**: `__vuma_alloc` (mmap on 9/10 backends; bump allocator on Wasm32) and `__vuma_free` (munmap on 9/10 backends; no-op on Wasm32) syscall stubs on all 10 backends
- **Womb stdlib**: 115 .vuma files (~65K lines, 114 compilable; `core.vuma` is an explicit design spec) including:
  - Collections: DynamicVec (heap-backed), HashMap, BTreeMap, EnumMap
  - Containers: generic container abstractions (1 file)
  - Strings: UTF-8 VStr, StringBuilder (3 modules across `string/` and `lib/`)
  - File I/O: raw syscalls, high-level read/write, path manipulation
  - Graph: heap-backed digraph with dynamic grow, toposort, cycle detection
  - IEEE: floating-point and IEEE frame helpers (2 files)
  - Language: 15 files — full lexer, full parser, IR builder, x86_64 codegen, ELF writer, plus `vuma_compiler.vuma` (506 LOC full self-hosting pipeline)
  - Crypto: 45 modules (sha1/3/384/512 + sha_variants, aes128/192/256 + modes, hmac, chacha20, poly1305, rsa+oaep/pss, ecdsa p256/p384, ed25519, x25519, secp256k1, ml_dsa, ml_kem, slh_dsa, falcon, hqc, bignum/bignum2048, blake2/blake3, md5, crc, and more)
  - Encoding: Base64, hex, URL
  - Network (`womb/net/`): TCP, SSH, QUIC, TLS 1.2, TLS 1.3 (5 files)
  - Library (`womb/lib/`): 28 files — DNS, HTTP/HTTP2, WebSocket, email (SMTP), app_protocols (MQTT), JSON, X.509, PKI, JWT, ASN.1, HPACK, deflate, threading, event_loop, math, stdio, time, socket, and more
- **Language features**: match expressions, struct field access, enum tagged unions, import system, for-range loops, break/continue, type annotations, dereference in function arguments, function calls in expressions
- **Arena allocator**: bump allocator on mmap'd block (opaque arena pattern)
- **Buffered I/O**: BufReader/BufWriter with 8KB buffer
- **CLI argument parsing**: via /proc/self/cmdline
- **Modular IVE infrastructure**: `src/ive/src/modular.rs` (389 LOC) — IncrementalCache, AbstractRegionTracker, RegionSummary, FunctionSummary, per-function verification (not integrated into main pipeline)
- **Self-hosting POC**: `src/bootstrap/vuma_compiler.vuma` (730 LOC lexer proof-of-concept) and `womb/lang/vuma_compiler.vuma` (506 LOC full pipeline)

### Fixed

- Const reference resolution (`x & MASK32` no longer returns 0)
- If-body return (`if cond { return val; }` no longer dropped)
- Function calls in while loops (recursive descent parser works)
- Deep call chains (5-level recursion works)
- Dereference in function arguments (`id(*(buf + 0))` works)
- Function calls in expressions (`if (is_digit(c) == 1)` works)
- Match expressions with block arms and default arm
- Switch block isolation (each Cmp+CondBranch has own block)
- CRC32 polynomial (3988292384 not 4022334336) — note: still fails on riscv64/ppc64
- SHA-256 K[1] (1899447441 not 1899447443)
- Function calls and Loads in while/if conditions
- User-visible variable name registration for Load/Allocation
- Digraph 8-bit overflow bug (node/edge counts truncated to 8 bits)
- Call-site argument DataFlow edges (labeled "arg0", "arg1", etc.)
- MSG cycle detection (updated for SCC-based topological sort)

### Known Limitations

- Verification has false positives (most programs use `--verification none`)
- Self-hosting started but not complete (lexer POC + full-pipeline attempt exist; not verified end-to-end)
- Type checking not implemented
- While-loop variable tracking bug across function calls
- `vuma-std` Rust crate not linked to VUMA programs
- BD inference M2.3 (generics) deferred
- Concurrent verification limited to single-threaded
- COR runtime not fully integrated (`Option<CORuntime>` in pipeline)
- CLI `vuma emit`/`vuma compile` accept 8 ISAs (missing RISC-V 32, x86_32); all 10 backends exist in codegen
- `womb/core.vuma` is a design spec and is not compilable
- 3 gold-standard test failures: `crc32.vuma` (riscv64, ppc64), `s27_fn_two_args_mod.vuma` (ppc64)

---

## [0.1.0-alpha.1] — 2026-06-28

Initial alpha release.

### Added

- 10 backend architectures: x86_64, AArch64, RISC-V 64, ARM32, MIPS64, PPC64, LoongArch64, x86_32, RISC-V 32, Wasm32
- 5,738-program gold-standard test suite at 99.99% pass rate (57,377/57,380 runs with `--verification none`)
- FFI: 19 Linux syscalls (enum `SyscallName`), `extern "C"` blocks, architecture-specific relocations
- Atomics: `AtomicLoad`, `AtomicStore`, `AtomicCas` on all 10 backends
- FP conversions: `IntToFloat`, `UIntToFloat`, `FloatToInt`, `FloatToUInt`, `FloatToFloat`
- DWARF v4 debug info
- Diagnostic codes (see `src/diagnostics.rs`)
- LLM API (`VumaForLLM`, 7 public methods)
- LSP server (6 capabilities: textDocumentSync, completion, hover, definition, documentSymbol, semanticTokens)
- REPL (`src/vuma/src/repl.rs`, 2,693 LOC, full pipeline)
- Module system with circular import detection (`ResolveError::CircularImport`)
- Package manager (`vuma pkg init/build/add`)
- 11 workspace crates
