# Womb — VUMA Standard Library

VUMA-native library code. All 115 `.vuma` files compile on x86_64 with `--verification none`.

**Important:** These are VUMA source files, not linked Rust code. They are not integrated into the compilation pipeline as automatic imports. Programs must manually inline the functions they need (the import system exists but has limitations).

## What Actually Works

All 115 files compile successfully on x86_64. The following modules have been tested and produce correct results:

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
| `crypto/*.vuma` | ✅ Compiles | 44 crypto modules (SHA, AES, RSA, ECDSA, etc.) |
| `encoding/*.vuma` | ✅ Compiles | Base64, hex, URL encoding |
| `net/*.vuma` | ✅ Compiles | TCP/UDP sockets, DNS, HTTP |

## Known Issues

- `allocate()` creates stack-local memory. Use `__vuma_alloc()` (mmap) for heap memory that persists across function calls.
- The import system (`import "module.vuma"::{func};`) works but has limitations with complex module graphs.
- The while-loop variable tracking across function calls has a known compiler bug. Sequential code works correctly.

## Module Categories

### Collections (`womb/collections/`)
- `vec.vuma` — Heap-backed DynamicVec (grow, push, pop, get, set)
- `hashmap.vuma` — Open-addressing hash map with 64-bit keys
- `btree_map.vuma` — Ordered map with O(log n) binary search
- `enum_map.vuma` — Tagged union storage for AST/SCG payloads

### Strings (`womb/string/`)
- `string.vuma` — C-style string operations (strlen, strcmp, memcpy, etc.)
- `utf8.vuma` — Dynamic UTF-8 string (VStr) with grow and codepoint decoding
- `string_builder.vuma` — Dynamic string concatenation

### File I/O (`womb/fs/`)
- `file.vuma` — Raw syscall wrappers (open, read, write, close, lseek)
- `high_level.vuma` — read_file, write_file, path manipulation

### Allocation (`womb/alloc/`)
- `arena.vuma` — Bump allocator on mmap'd block (opaque arena pattern)

### Graph (`womb/graph/`)
- `digraph.vuma` — Heap-backed directed graph with dynamic grow
- `algorithms.vuma` — Topological sort, cycle detection

### I/O (`womb/io/`)
- `buffered.vuma` — BufReader/BufWriter with 8KB buffer

### CLI (`womb/env/`)
- `cli.vuma` — CLI argument parsing via /proc/self/cmdline

### Language (`womb/lang/`)
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
- `self_host_test.vuma` — End-to-end pipeline test

### Crypto (`womb/crypto/`)
44 modules including: SHA-1/256/384/512, SHA-3, AES-128/192/256, ChaCha20, Poly1305, HMAC, RSA, ECDSA, Ed25519, X25519, bcrypt, scrypt, Argon2, HKDF, PBKDF2, and more.

### Encoding (`womb/encoding/`)
Base64, hex, URL encoding/decoding.

### Network (`womb/net/`)
TCP/UDP sockets, DNS, HTTP, WebSocket, MQTT, SMTP, and more.

### Codec (`womb/codec/`)
Byte-level encoding/decoding utilities (LE/BE store/load, mem_copy, mem_set, mem_cmp).
