# VUMA Releases

Release summaries with key changes and known limitations.

---

## v0.2.0-alpha.1 — 2026-06-30

### What Changed

- **Womb stdlib expanded** to 115 .vuma files (~65K lines). All compile on x86_64 with `--verification none`.
- **Heap allocation**: `__vuma_alloc` (mmap) and `__vuma_free` (munmap) syscall stubs work across function calls on all 10 backends.
- **Language features added**: match expressions, struct field access, enum tagged unions, import system, for-range loops, break/continue, type annotations, dereference in function arguments, function calls in expressions.
- **Compiler bug fixes**: const reference resolution, if-body returns, function calls in loops, deep call chains, CRC32 polynomial, SHA-256 K[1] constant.
- **10-backend codegen**: 100% gold-standard pass rate (57,380/57,380 runs with `--verification none`).

### Womb Modules

All 115 womb files compile on x86_64. Categories include:
- **Collections**: DynamicVec (heap-backed), HashMap, BTreeMap, EnumMap
- **Strings**: UTF-8 VStr with grow, StringBuilder
- **File I/O**: raw syscall wrappers, high-level read_file/write_file
- **Graph**: heap-backed digraph with dynamic grow, topological sort, cycle detection
- **Language**: full lexer, full parser, IR builder, x86_64 codegen, ELF writer
- **Crypto**: 44 modules (SHA, AES, RSA, ECDSA, Ed25519, ChaCha20, etc.)
- **Encoding**: Base64, hex, URL
- **Network**: TCP/UDP, DNS, HTTP, WebSocket

### Known Limitations

- **Self-hosting**: Not started. Individual pipeline modules exist but aren't tested end-to-end.
- **Verification**: `--verification normal` has false positives. Most programs use `--verification none`.
- **Standard library**: `vuma-std` Rust crate not linked. Womb modules exist but aren't auto-imported.
- **BD inference (M2.3)**: Complex generic inference deferred.
- **Concurrent verification**: Single-threaded only.
- **Type checking**: Not implemented (parser recognizes syntax but doesn't validate types).
- **While-loop variable tracking**: Known compiler bug when loop body calls functions.

---

## v0.1.0-alpha.1 — 2026-06-28

Initial alpha release.

### What Worked

- 10 backend architectures at 100% gold-standard pass rate (57,380/57,380 with `--verification none`)
- Parser, SCG, IVE, BD, codegen, FFI, atomics, DWARF debug info
- 5,738-program gold-standard test suite
- LLM API, LSP server, REPL, package manager
