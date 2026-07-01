# VUMA Changelog

All notable changes to the VUMA project.

---

## [0.2.0-alpha.1] — 2026-06-30

### Added

- **Heap allocation**: `__vuma_alloc` (mmap) and `__vuma_free` (munmap) syscall stubs on all 10 backends
- **Womb stdlib**: 115 .vuma files (~65K lines) including:
  - Collections: DynamicVec (heap-backed), HashMap, BTreeMap, EnumMap
  - Strings: UTF-8 VStr, StringBuilder
  - File I/O: raw syscalls, high-level read/write, path manipulation
  - Graph: heap-backed digraph with dynamic grow, toposort, cycle detection
  - Language: full lexer, full parser, IR builder, x86_64 codegen, ELF writer
  - Crypto: 44 modules (SHA, AES, RSA, ECDSA, Ed25519, ChaCha20, etc.)
  - Network: TCP/UDP, DNS, HTTP, WebSocket
- **Language features**: match expressions, struct field access, enum tagged unions, import system, for-range loops, break/continue, type annotations, dereference in function arguments, function calls in expressions
- **Arena allocator**: bump allocator on mmap'd block (opaque arena pattern)
- **Buffered I/O**: BufReader/BufWriter with 8KB buffer
- **CLI argument parsing**: via /proc/self/cmdline
- **Modular IVE infrastructure**: IncrementalCache, AbstractRegionTracker, per-function verification (not integrated into main pipeline)

### Fixed

- Const reference resolution (`x & MASK32` no longer returns 0)
- If-body return (`if cond { return val; }` no longer dropped)
- Function calls in while loops (recursive descent parser works)
- Deep call chains (5-level recursion works)
- Dereference in function arguments (`id(*(buf + 0))` works)
- Function calls in expressions (`if (is_digit(c) == 1)` works)
- Match expressions with block arms and default arm
- Switch block isolation (each Cmp+CondBranch has own block)
- CRC32 polynomial (3988292384 not 4022334336)
- SHA-256 K[1] (1899447441 not 1899447443)
- Function calls and Loads in while/if conditions
- User-visible variable name registration for Load/Allocation
- Digraph 8-bit overflow bug (node/edge counts truncated to 8 bits)
- Call-site argument DataFlow edges (labeled "arg0", "arg1", etc.)
- MSG cycle detection (updated for SCC-based topological sort)

### Known Limitations

- Verification has false positives (most programs use `--verification none`)
- Self-hosting not started (individual modules exist, not tested end-to-end)
- Type checking not implemented
- While-loop variable tracking bug across function calls
- `vuma-std` Rust crate not linked to VUMA programs
- BD inference M2.3 (generics) deferred
- Concurrent verification limited to single-threaded
- COR runtime not fully integrated

---

## [0.1.0-alpha.1] — 2026-06-28

Initial alpha release.

### Added

- 10 backend architectures: x86_64, AArch64, RISC-V 64, ARM32, MIPS64, PPC64, LoongArch64, x86_32, RISC-V 32, Wasm32
- 5,738-program gold-standard test suite at 100% pass rate (57,380/57,380 runs with `--verification none`)
- FFI: 19 Linux syscalls, `extern "C"` blocks, architecture-specific relocations
- Atomics: `AtomicLoad`, `AtomicStore`, `AtomicCas` on all 10 backends
- FP conversions: `IntToFloat`, `UIntToFloat`, `FloatToInt`, `FloatToUInt`, `FloatToFloat`
- DWARF v4 debug info
- 66 diagnostic codes
- LLM API (`VumaForLLM`)
- LSP server
- REPL
- Module system with circular import detection
- Package manager (`vuma pkg init/build/add`)
- 11 workspace crates
