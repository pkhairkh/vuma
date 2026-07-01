# Womb — VUMA Standard Library

VUMA-native library code. 115 `.vuma` files (64,759 lines). **114 compile on x86_64 with `--verification none`**; `womb/core.vuma` is an explicit design spec and is not compilable (its own header states "DESIGN SPEC, NOT COMPILABLE — VUMA compiler CANNOT compile this file").

**Important:** These are VUMA source files, not linked Rust code. They are not integrated into the compilation pipeline as automatic imports. Programs must manually inline the functions they need (the import system exists but has limitations).

## What Actually Works

114 of 115 files compile successfully on x86_64. The following modules have been tested and produce correct results:

| Module | Status | Notes |
|--------|--------|-------|
| `collections/vec.vuma` | ✅ Tested | Heap-backed DynamicVec using `__vuma_alloc` (mmap). Works across function calls. |
| `graph/digraph.vuma` | ✅ Tested | Heap-backed directed graph. Verified 300+ nodes with dynamic grow. |
| `string/utf8.vuma` | ✅ Tested | Dynamic VStr with grow, append, compare. |
| `fs/file.vuma` | ✅ Tested | Raw syscall wrappers (open, read, write, close). |
| `lang/full_lexer.vuma` | ✅ Compiles | Full VUMA lexer with strings, chars, hex, all operators. |
| `lang/full_parser.vuma` | ✅ Compiles | Full recursive descent parser (structs, enums, match, closures, generics). |
| `lang/ir_builder.vuma` | ✅ Compiles | AST→IR lowering with symbol table. |
| `lang/codegen.vuma` | ✅ Compiles | x86_64 instruction encoders. |
| `lang/elf.vuma` | ✅ Compiles | ELF64 writer. |
| `lang/vuma_compiler.vuma` | ✅ Compiles | 506 LOC full VUMA-in-VUMA self-hosting pipeline (lexer→parser→IR→codegen→ELF). |
| `crypto/*.vuma` | ✅ Compiles | 45 crypto modules (SHA, AES, RSA, ECDSA, Ed25519, ML-DSA, ML-KEM, etc.) |
| `encoding/*.vuma` | ✅ Compiles | Base64, hex, URL encoding |
| `net/*.vuma` | ✅ Compiles | TCP, SSH, QUIC, TLS 1.2, TLS 1.3 |
| `core.vuma` | ❌ Not compilable | Explicit design spec only. |

## Known Issues

- `allocate()` creates stack-local memory in the direct path; in the canonical pipeline, allocations > 4096 bytes use the heap. Use `__vuma_alloc()` for guaranteed heap memory that persists across function calls.
- The import system (`import "module.vuma"::{func};`) works but has limitations with complex module graphs.
- The while-loop variable tracking across function calls has a known compiler bug. Sequential code works correctly.
- `womb/core.vuma` is a design spec, not a compilable program.

## Module Categories

### Collections (`womb/collections/`, 4 files)
- `vec.vuma` — Heap-backed DynamicVec (grow, push, pop, get, set)
- `hashmap.vuma` — Open-addressing hash map with 64-bit keys
- `btree_map.vuma` — Ordered map with O(log n) binary search
- `enum_map.vuma` — Tagged union storage for AST/SCG payloads

### Containers (`womb/containers/`, 1 file)
- `containers.vuma` — Generic container abstractions (627 LOC)

### Strings (`womb/string/`, 3 files)
- `string.vuma` — Minimal C-style string helpers (data, len)
- `utf8.vuma` — Dynamic UTF-8 string (VStr) with grow and codepoint decoding
- `string_builder.vuma` — Dynamic string concatenation

> Note: There are also `womb/lib/string.vuma` (POSIX string.h) and `womb/lang/string.vuma` (language-level utilities) — three distinct string modules serving different purposes.

### File I/O (`womb/fs/`, 2 files)
- `file.vuma` — Raw syscall wrappers (open, read, write, close, lseek)
- `high_level.vuma` — read_file, write_file, path manipulation

### Allocation (`womb/alloc/`, 1 file)
- `arena.vuma` — Bump allocator on mmap'd block (opaque arena pattern)

### Graph (`womb/graph/`, 2 files)
- `digraph.vuma` — Heap-backed directed graph with dynamic grow
- `algorithms.vuma` — Topological sort, cycle detection

### IEEE (`womb/ieee/`, 2 files)
- `fp.vuma` — Floating-point helpers
- `ieee_frames.vuma` — IEEE frame helpers

### I/O (`womb/io/`, 1 file)
- `buffered.vuma` — BufReader/BufWriter with 8KB buffer

### CLI (`womb/env/`, 1 file)
- `cli.vuma` — CLI argument parsing via /proc/self/cmdline

### Language (`womb/lang/`, 15 files)
- `tokens.vuma` — Token type definitions
- `lexer.vuma` — Basic VUMA lexer
- `full_lexer.vuma` — Full VUMA lexer (strings, chars, hex, all operators)
- `ast.vuma` — AST node definitions (arena-based)
- `parser.vuma` — Basic recursive descent parser
- `full_parser.vuma` — Full parser (structs, enums, match, closures, generics)
- `ir.vuma` — IR instruction definitions
- `ir_builder.vuma` — AST→IR lowering
- `codegen.vuma` — x86_64 instruction encoders
- `elf.vuma` — ELF64 writer
- `mini_compiler.vuma` — Integration test (stdin→lex→output)
- `minicompiler.vuma` — Minimal compiler variant (103 LOC)
- `self_host_test.vuma` — End-to-end pipeline test
- `string.vuma` — Language-level string utilities (191 LOC)
- `vuma_compiler.vuma` — Full VUMA-in-VUMA self-hosting compiler pipeline (506 LOC)

### Crypto (`womb/crypto/`, 45 files)
45 modules including: SHA-1/3/384/512 (+ sha_variants), AES-128/192/256 (+ modes: CFB, OFB, GCM, CTR, CBC, ECB), ChaCha20, ChaCha20-Poly1305, Salsa20, Poly1305, HMAC, HKDF, PBKDF2, scrypt, Argon2, RSA (+ OAEP/PSS), ECDSA (P-256/P-384), Ed25519, X25519, secp256k1, ECDH P-256, ML-DSA, ML-KEM, SLH-DSA, Falcon, HQC, bignum, bignum2048, BLAKE2, BLAKE3, MD5, CRC, DRBG, KDF/CMAC/bcrypt, key agreement, legacy ciphers, signatures extra.

### Encoding (`womb/encoding/`, 3 files)
Base64, hex, URL encoding/decoding.

### Network (`womb/net/`, 5 files)
- `tcp.vuma` — TCP sockets
- `ssh.vuma` — SSH protocol
- `quic.vuma` — QUIC protocol
- `tls12.vuma` — TLS 1.2
- `tls13.vuma` — TLS 1.3

> Note: DNS, HTTP/HTTP2, WebSocket, email (SMTP), and app protocols (MQTT) live in `womb/lib/`, not `womb/net/`.

### Library (`womb/lib/`, 28 files)
General-purpose standard library modules: stdlib, stdio, math, time, string (POSIX), printf, unicode, json, fileio, socket, dns, dns_extra, http, http2, websocket, email (SMTP), app_protocols (MQTT), net_protocols, asn1, x509, pki, auth, jwt, hpack, deflate, compression_extra, event_loop, threading.

### Codec (`womb/codec/`, 1 file)
Byte-level encoding/decoding utilities (LE/BE store/load, mem_copy, mem_set, mem_cmp).

### Root (`womb/core.vuma`, 1 file)
Design spec only — **not compilable**. Documents intended core semantics.
