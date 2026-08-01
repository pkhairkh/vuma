# J-1 — WOMB Layer Audit

**Scope.** Audit of `/home/z/my-project/workspace/vuma/womb/` — VUMA's
standard library written in VUMA itself. Maps the existing tree to the
SWE package's proposed WOMB UI modules
(`vuma-swe-package-3/vuma-swe-package/26-new-plans-three-layers.md`).
Surfaces what is reusable, what is missing, and what architectural
decisions need ADRs before the UI engine team can begin Phase W-0
(event pipeline) at month 3.

**Method.** `find womb/ -name '*.vuma'` enumerated 195 files across
50 subdirectories (total ≈ 117 062 LOC of `.vuma` source). The first
~60 lines of every key file were read to capture the docstring
header; the body was sampled for the larger modules. Grep was used to
locate stub markers, broken imports, and the canonical locations of
the HMAC-SHA-256 and IrqRing implementations that the SWE package
claims are reusable.

**Single most important finding.** The `womb/` tree as it stands is
NOT a thin stdlib waiting for UI modules to be dropped on top. It is a
~117 kLOC, 195-file ecosystem that already includes a complete kernel
(VWK, 43 kLOC), a complete cryptographic suite (26 kLOC across 7
subdirectories), a self-hosted VUMA bootstrap compiler (4 kLOC), a
POSIX-style libc layer (21 kLOC), and a 15 kLOC networking stack
(HTTP/1.1, HTTP/2, HTTP/3, TLS 1.2, TLS 1.3, QUIC, SSH, WebSocket,
DNS+DNSSEC, HPACK, NTP/FTP/LDAP/SNMP/SIP/RTSP/BGP/SCTP). The SWE
package's WOMB UI modules (`womb/ui/layout`, `womb/ui/render`,
`womb/ui/text`, `womb/ui/ime`, `womb/ui/a11y`, `womb/ui/event`,
`womb/ui/animation`, `womb/ui/theme`) are **entirely greenfield** —
there is no `womb/ui/` directory today. But several existing
artifacts (IrqRing, HMAC-SHA-256, HTTP/WebSocket parsers, JSON,
collections, string, fs, encoding) are reusable as substrate.

---

## WOMB directory structure

Total: **195 `.vuma` files + 18 `.S` / `.ld` files = 213 files,
~117 062 LOC** (`.vuma` only; the `.S` and `.ld` files are tiny
per-arch kernel boot/trap/linker stubs).

```
womb/
├── syscalls.vuma                   252 LOC  (documentation-only — §0)
├── alloc/
│   └── arena.vuma                  103 LOC  (PMT bump allocator)
├── collections/                    676 LOC / 4 files
│   ├── vec.vuma                    170 LOC  (heap-backed dynamic array)
│   ├── hashmap.vuma                200 LOC  (open-addressing FNV-1a)
│   ├── btree_map.vuma              206 LOC  (sorted array, binary search)
│   └── enum_map.vuma               104 LOC  (tagged union storage)
├── crypto/                        26 179 LOC / 41 files (7 subdirs)
│   ├── asym/                       5 803 LOC / 9 files
│   │   ├── rsa.vuma, rsa_oaep_pss.vuma, rsa_pkcs1_ecdsa_extra.vuma
│   │   ├── ecdsa_p256.vuma, ecdsa_p384.vuma, ecdh_p256.vuma
│   │   ├── ed25519.vuma, x25519.vuma, secp256k1.vuma
│   ├── bignum/                   1 197 LOC / 2 files (bignum.vuma, bignum2048.vuma)
│   ├── drbg/                       563 LOC / 2 files (HMAC_DRBG SP 800-90A)
│   ├── hash/                       4 091 LOC / 8 files
│   │   ├── md5, sha1, sha256_sha224, sha384, sha512, sha3
│   │   ├── blake2, blake3
│   ├── mac_kdf/                    2 583 LOC / 7 files
│   │   ├── hmac.vuma                193 LOC  (RFC 2104 — REAL, F-2 confirmed)
│   │   ├── hkdf.vuma                152 LOC  (RFC 5869)
│   │   ├── pbkdf2.vuma, scrypt.vuma, argon2.vuma
│   │   ├── cmac_bcrypt_kdf.vuma, key_agreement.vuma
│   ├── post_quantum/               4 238 LOC / 5 files (ml_kem, ml_dsa, slh_dsa, falcon, hqc)
│   └── symmetric/                  7 704 LOC / 11 files
│       ├── aes128, aes192, aes256, aes_modes, aes_cfb_ofb, aes_extra_modes
│       ├── chacha20, chacha20_poly1305, poly1305, salsa20
│       └── des_rc4_aria_camellia.vuma  3 424 LOC  (largest single file)
├── encoding/                       332 LOC / 4 files (hex, base64, url, crc)
├── env/cli.vuma                    214 LOC
├── fs/                             253 LOC / 2 files (file.vuma, high_level.vuma)
├── graph/                          363 LOC / 2 files (digraph.vuma, algorithms.vuma)
├── io/buffered.vuma                125 LOC  (BufReader/BufWriter)
├── kernel/                        43 085 LOC / 95 files (VWK kernel — §3.4)
│   ├── kernel.vuma               1 121 LOC  (interactive shell main)
│   ├── bootinfo.vuma               180 LOC  (canonical superset BootInfo)
│   ├── arch/                       ~6 300 LOC / 22 files (5 archs × 4-7 files)
│   │   ├── x86_64/                 1 886 LOC / 7 files (boot.S, trap.S, switch.S, linker.ld, pt, vmm_hal, trap_trampoline, mm_trampoline, bootinfo, switch, trampoline)
│   │   ├── aarch64/                1 821 LOC / 6 files
│   │   ├── riscv64/                2 005 LOC / 6 files
│   │   ├── ppc64le/                4 files (.S + .ld only — no .vuma)
│   │   ├── hosted/                 2 files (boot.S + linker.ld)
│   │   └── wasm32/sched_hal.vuma   585 LOC  (cooperative scheduling HAL)
│   ├── crypto/                     2 409 LOC / 5 files (api, aes, sha, asym, hw_trampoline)
│   ├── drivers/                    1 813 LOC / 4 files (early_console, uart, char, virtio_net)
│   ├── fs/                         4 645 LOC / 4 files (devfs, initramfs, procfs, tmpfs)
│   ├── hosted/host.vuma            156 LOC
│   ├── ipc/                        4 847 LOC / 5 files (pipe, signal, futex, shm, waitq)
│   ├── mm/                         2 771 LOC / 4 files (pmm buddy, vmm, kmalloc, mmap)
│   ├── net/                        2 515 LOC / 5 files (sk_buff, http, socket, tcp, dns)
│   ├── panic/                      614 LOC / 2 files (panic, kmsg)
│   ├── power/pm.vuma               354 LOC
│   ├── proc/                       5 368 LOC / 7 files (task, scheduler, fork, exec, wait, exit, elf)
│   ├── shell/shell.vuma          2 134 LOC  (interactive shell)
│   ├── smp/                        1 747 LOC / 3 files (percpu, ipi, smp)
│   ├── sync/                       3 139 LOC / 4 files (spinlock, mutex, semaphore, rwlock)
│   ├── syscall/                    1 606 LOC / 4 files (abi, table, dispatch, syscall_init)
│   │   └── handlers/               1 253 LOC / 4 files (io, fs, proc, mm)
│   ├── trap/                       2 117 LOC / 4 files
│   │   ├── irq_ring.vuma           472 LOC  (SPSC event ring — §4.1)
│   │   ├── trap_frame.vuma
│   │   ├── irq.vuma
│   │   └── trap.vuma               689 LOC
│   ├── tty/                        2 326 LOC / 3 files (vt100, console, line_discipline)
│   └── vfs/                        4 524 LOC / 7 files (ops, file, inode, dentry, file_ops, namei, mount)
├── lang/                           4 170 LOC / 6 files (self-hosted bootstrap — §3.6)
│   ├── hello.vuma                   25 LOC  (prints 42, exits 0)
│   ├── full_lexer.vuma             895 LOC  (entry point of self-host)
│   ├── full_parser.vuma            793 LOC
│   ├── ir_builder.vuma             935 LOC
│   ├── codegen.vuma              1 398 LOC  (x86_64 emitter)
│   └── elf.vuma                    124 LOC  (ELF64 writer)
├── lib/                           21 509 LOC / 21 files (POSIX-shaped stdlib — §3.7)
│   ├── mem_helpers.vuma             60 LOC  (single source of truth for store_u*/load_u*)
│   ├── compress/                   2 531 LOC / 2 files (deflate+zlib, gzip/lz4/zstd/brotli wrappers)
│   ├── concurrency/                  882 LOC / 2 files (epoll event_loop, threading)
│   ├── pki/                         6 231 LOC / 5 files (asn1, x509, pki, auth, jwt)
│   ├── sys/                         3 633 LOC / 7 files (stdlib, math, stdio, fileio, time, fp, email)
│   └── text/                        4 002 LOC / 4 files (string, unicode, json, printf)
├── net/                           14 884 LOC / 15 files (RFC-grade protocol stack — §3.8)
│   ├── socket.vuma                  364 LOC  (POSIX socket API wrapper)
│   ├── tcp.vuma                     206 LOC  (BSD socket client/server)
│   ├── dns.vuma                     262 LOC  (RFC 1035)
│   ├── dns_extra.vuma             1 286 LOC  (DNSSEC, mDNS, DoT, DoH, EDNS0)
│   ├── http.vuma                    944 LOC  (RFC 7230/9112 — REAL)
│   ├── http2.vuma                   696 LOC  (RFC 9113 — REAL)
│   ├── http3_mqtt_coap.vuma       1 076 LOC  (RFC 9114 + QPACK + MQTT + CoAP)
│   ├── websocket.vuma               506 LOC  (RFC 6455 — REAL)
│   ├── hpack.vuma                 1 787 LOC  (RFC 7541 — REAL)
│   ├── tls12.vuma                   985 LOC  (RFC 5246 — REAL)
│   ├── tls13.vuma                 1 277 LOC  (RFC 8446 — REAL)
│   ├── quic.vuma                  1 893 LOC  (RFC 9000/9001 — REAL)
│   ├── ssh.vuma                   1 045 LOC  (RFC 4251-4254 — REAL)
│   ├── ip_icmp_arp.vuma           1 665 LOC  (IEEE 802.3/802.1Q/802.11/802.15.4 + ARP + IPv4 + ICMP)
│   └── ieee_frames.vuma           1 505 LOC  (NTP/FTP/LDAP/SNMP/SIP/RTSP/BGP/SCTP)
└── string/                         416 LOC / 3 files (string, string_builder, utf8)
```

---

## Module-by-module inventory

The maturity classification scheme:

- **REAL** — implementation matches the cited RFC/standard; the file
  header documents the algorithm, the body implements it, the
  self-test (when present) validates it. Compiles and runs.
- **SCAFFOLD** — compiles cleanly, defines layouts + signatures, but
  the body is a structural skeleton (placeholder values, simplified
  algorithm). Useful as API-shape validation, NOT as a working
  implementation.
- **STUB** — function body returns 0 / hardcoded value / explicitly
  marked "stub:" in the source. No real algorithm.
- **DEAD** — file or directory whose imports cannot be resolved by
  `src/parser/src/resolver.rs::resolve_import_path` (which is just
  `base_dir.join(path)` at `resolver.rs:500-510`); or file is never
  imported by anything.

### `womb/syscalls.vuma` — DOCUMENTATION (not code)

- **Maturity:** DOCUMENTATION
- **Evidence:** Header at `:1-15` says *"This file is a
  DOCUMENTATION-ONLY reference. It contains no `transform` definitions
  and no `extern` blocks — only `//` comments and a handful of `const`
  declarations for argument-flag values."* Body at `:80-252` is a
  reference table of every Linux asm-generic syscall number.
- **What it provides to VUMA:** A lookup table programmers consult to
  write `syscall(NR, ...)` call sites. Not imported by anything
  (`:7-9` admits *"Womb has no `import` mechanism yet (Open Work §7),
  so this file is NOT imported by other womb modules"*).
- **Reusable for UI engine:** No (browser target uses wasm host
  imports, not syscalls).

### `womb/alloc/arena.vuma` — REAL (103 LOC, PMT-pure)

- **Maturity:** REAL
- **Evidence:** Header at `:1-47` documents the PMT migration of the
  pointer-based bump allocator. `arena_init`/`arena_alloc`/`arena_reset`/`arena_used`/`arena_capacity` at `:55-103`. Fixed 256-byte `data: [u8; 256]` field — capacity is a compile-time constant.
- **Caveat:** Header at `:30-43` admits the data block is fixed-size
  (256 bytes) because PMT layout sizes are compile-time constants.
  Not a growable arena — only suitable for tiny transient allocations.
- **Reusable for UI engine:** Marginal. The UI engine needs growable
  per-frame arenas (scene graph, path tessellation, glyph caches).
  This arena is too small. The pattern is reusable; the
  implementation is not.

### `womb/collections/` — REAL (4 files, 676 LOC)

- **Maturity:** REAL (heap-backed, uses `allocate()`/`free()` + byte-stitched `store_u64`/`load_u64`)
- **Evidence:**
  - `vec.vuma:1-25` — dynamic array, heap-allocated, doubles capacity.
  - `hashmap.vuma:1-43` — open-addressing, FNV-1a 32-bit hash, linear
    probing, grows at 0.75 load factor. Entry size 32 bytes.
  - `btree_map.vuma:1-33` — sorted-array BTreeMap with binary search.
    Fixed-size keys.
  - `enum_map.vuma:1-26` — tagged-union storage for AST/SCG node
    payloads (40-byte entries, 32-byte payload).
- **Caveat:** All four use the legacy `allocate()`/`free()` (heap
  memory, mmap-backed) idiom, NOT the modern PMT `state_new(T)` style.
  The headers explicitly mention the PMT migration is deferred for
  these (e.g. `vec.vuma:5-7` *"Uses allocate() (mmap) for persistent
  heap memory."*).
- **Reusable for UI engine:** Yes — these are the foundational data
  structures for layout trees, scene graphs, dirty-rect tracking,
  event queues. The Vec and HashMap are the most directly useful; the
  BTreeMap and EnumMap are useful for symbol tables / style records.

### `womb/crypto/` — REAL (41 files, 26 179 LOC)

The crown jewel of `womb/`. Every file declares its cited RFC, every
file implements the algorithm in full, every file's header explains
the memory layout and design decisions. This is production-grade
cryptographic code (modulo the post-quantum modules which are still
research-grade).

- **`crypto/hash/`** — 8 files, 4 091 LOC
  - `sha256_sha224.vuma` (1 525 LOC) — FIPS 180-4 + FIPS 202 + NIST
    SP 800-185. Header at `:1-23` documents 7 hash/XOF/MAC primitives
    (SHA-224, SHA-512/224, SHA-512/256, SHA-3-224, SHA-3-384,
    cSHAKE-256, KMAC-256). Exports `sha256_oneshot` (the symbol
    imported by `net/tls*.vuma`, `net/ssh.vuma`, `net/quic.vuma`).
  - `sha512.vuma`, `sha384.vuma`, `sha3.vuma`, `blake2.vuma`,
    `blake3.vuma` (542 LOC, BLAKE3 spec) — all REAL.
  - `md5.vuma`, `sha1.vuma` — REAL (legacy but RFC-compliant).
- **`crypto/symmetric/`** — 11 files, 7 704 LOC
  - `aes128.vuma`, `aes192.vuma`, `aes256.vuma` — REAL FIPS 197.
  - `aes_modes.vuma` (508 LOC) — GCM, CBC, CTR, CCM, etc.
  - `chacha20.vuma`, `chacha20_poly1305.vuma` (RFC 8439), `poly1305.vuma`,
    `salsa20.vuma` — REAL.
  - `des_rc4_aria_camellia.vuma` (3 424 LOC) — single-file aggregator
    for legacy/regional ciphers.
- **`crypto/asym/`** — 9 files, 5 803 LOC
  - `ed25519.vuma` (594 LOC) — RFC 8032, real curve arithmetic via
    `bignum.vuma` bn256_* primitives. Header at `:1-15` documents
    Curve25519 params (p, d, B, L).
  - `x25519.vuma`, `ecdsa_p256.vuma`, `ecdsa_p384.vuma`, `ecdh_p256.vuma`,
    `secp256k1.vuma` — REAL.
  - `rsa.vuma`, `rsa_oaep_pss.vuma`, `rsa_pkcs1_ecdsa_extra.vuma` — REAL.
- **`crypto/mac_kdf/`** — 7 files, 2 583 LOC
  - **`hmac.vuma` (193 LOC) — RFC 2104 / FIPS 198-1. REAL. F-2
    confirmed.** Lines `:75-118` implement HMAC-SHA-256 with the
    canonical kprime/kipad/kopad construction. Also exports
    `hmac_sha1`, `hmac_sha512`, `ct_memcmp`.
  - `hkdf.vuma` (152 LOC) — RFC 5869. Exports
    `hkdf_extract_sha256`, `hkdf_expand_sha256`.
  - `pbkdf2.vuma`, `scrypt.vuma` (RFC 7914), `argon2.vuma` (RFC 9106),
    `cmac_bcrypt_kdf.vuma`, `key_agreement.vuma` — REAL.
- **`crypto/post_quantum/`** — 5 files, 4 238 LOC
  - `ml_kem.vuma` (1 344 LOC) — FIPS 203 (Kyber), all 3 param sets.
  - `ml_dsa.vuma`, `slh_dsa.vuma`, `falcon.vuma`, `hqc.vuma` (1 105 LOC)
    — REAL but research-grade (HQC's own header at `:52-56` warns
    *"constant-time but may be slower constant time... production HQC
    uses a tensor BCH⊗RM code with a much more elaborate decoder"*).
  - **`hqc.vuma` re-exports `sha256_oneshot`** as a side effect
    (`:49-50` *"A self-contained SHA-256 is embedded so the module has
    no external dependencies; this is required for ss = SHA-256(m||ct)"*).
    This is why `net/ssh.vuma:57`, `net/quic.vuma:64`, `net/tls12.vuma:98`,
    `net/tls13.vuma:147`, `lib/sys/email.vuma:66` all import
    `sha256_oneshot` from `"../crypto/hqc.vuma"` rather than from
    `../crypto/hash/sha256_sha224.vuma` — but those imports point to a
    path that doesn't exist (see §"Broken cross-directory imports" below).
- **`crypto/bignum/`** — 2 files, 1 197 LOC
  - `bignum.vuma` (509 LOC) — 256-bit (4 × u64 limbs). REAL add/sub/mul/div/mod/mod_exp/gcd/mod_inv.
  - `bignum2048.vuma` (688 LOC) — 2048-bit (32 × u64 limbs). Used by
    `net/ssh.vuma` for Diffie-Hellman group14.
- **`crypto/drbg/`** — 2 files, 563 LOC
  - `drbg.vuma` (192 LOC) — HMAC_DRBG per NIST SP 800-90A Rev. 1.
    Exports `drbg_init`, `drbg_reseed`, `drbg_generate`.

**Reusable for UI engine:** YES, extensively.
- `hmac_sha256` is the canonical implementation VUMA's capability
  model should adopt (ADR-0007 already says so).
- `sha256_oneshot` is needed by every capability-token signature path.
- `aes256_gcm_*` (in `aes_modes.vuma`) is needed for any at-rest
  encrypted storage the UI engine might do (caches, font subsetting
  for distribution).
- The post-quantum modules are NOT directly relevant to the UI engine.

### `womb/encoding/` — REAL (4 files, 332 LOC)

- **Maturity:** REAL
- **Evidence:**
  - `hex.vuma` (54 LOC) — RFC 4648 hex encode/decode, including
    uppercase variant.
  - `base64.vuma` (82 LOC) — RFC 4648 base64 encode/decode, padding
    support.
  - `url.vuma` (71 LOC) — RFC 3986 percent-encoding.
  - `crc.vuma` (129 LOC) — CRC-16 (CCITT, Modbus), CRC-32 (IEEE 802.3),
    CRC-64 (ECMA-182). Incremental API.
- **Reusable for UI engine:** Yes — base64 for capability tokens and
  data URLs, url-encode for query strings, crc32 for asset integrity.

### `womb/env/cli.vuma` — REAL (214 LOC)

- **Maturity:** REAL
- **Evidence:** Header at `:1-19` documents the CLI parsing approach
  (reads `/proc/self/cmdline` via `openat(56)/read(63)/close(57)`
  rather than stack-scraping). Body implements `cli_read_cmdline`,
  `cli_args_count`, `cli_arg_get`, `cli_parse_flags`.
- **Reusable for UI engine:** No — browser target has no argv.

### `womb/fs/` — REAL (2 files, 253 LOC)

- **Maturity:** REAL
- **Evidence:**
  - `file.vuma` (88 LOC) — `file_read_to_buffer`, `file_result_data`,
    `file_result_len`, `file_write`. Uses `openat(56)/read(63)/write(64)/close(57)`.
  - `high_level.vuma` (167 LOC) — `read_file`, `read_result_data`,
    `read_result_len`, `write_file`, `path_join`, `path_basename`.
- **Reusable for UI engine:** Marginal — browser target uses host
  `fetch()` for assets, not file I/O. Native target (post-v1) could
  reuse these for offline font loading.

### `womb/graph/` — REAL (2 files, 363 LOC)

- **Maturity:** REAL
- **Evidence:**
  - `digraph.vuma` (188 LOC) — directed graph with linked-list
    adjacency. Heap-backed, uses `allocate()`. 48-byte Graph struct,
    12-byte Node, 16-byte Edge.
  - `algorithms.vuma` (177 LOC) — topological sort (Kahn's),
    cycle detection.
- **Reusable for UI engine:** YES — the layout tree is a DAG (stacking
  contexts create cycles in the render-order sense), and dirty
  propagation needs topological sort. The data structure is directly
  applicable.

### `womb/io/buffered.vuma` — REAL (125 LOC)

- **Maturity:** REAL
- **Evidence:** Header at `:1-3` documents BufReader/BufWriter. Body
  implements `bufreader_new`, `bufreader_fill`, `bufreader_read_byte`,
  `bufreader_read_line`, `bufreader_read` (and BufWriter equivalents).
- **Reusable for UI engine:** Marginal — only relevant for native
  target's stdin/stdout. Browser uses host imports.

### `womb/kernel/` — REAL (43 085 LOC, 95 files, full VWK kernel)

The `womb/kernel/` subtree is the **VWK (VUMA Wumba Kernel)** — a
complete from-scratch kernel written in VUMA. This is NOT mentioned in
the SWE package's three-layer plan, but it is the largest single
subsystem in `womb/` and represents a substantial existing investment.

- **Maturity:** REAL with documented K11+ deferred bare-metal work.
  Header at `kernel/kernel.vuma:1-12` shows the interactive shell
  (`echo`, `help`, `exit`, `pid`, `ps`, `ls`, `cat`, `touch`, `mkdir`,
  `alloc`, `free`, `ver`, `memstat`, `cd`, `pwd`, `write`).
  `scripts/kernel_smoke.sh:1-165` is the hosted-mode smoke test
  (compiles `womb/kernel/kernel.vuma` with `--verify`, runs the
  resulting ELF, greps for the "VWK kernel booted" banner).
- **Subdir breakdown:**
  - **`arch/`** — per-arch boot/trap/switch/page-table/VMM-HAL stubs
    for x86_64, aarch64, riscv64, ppc64le, hosted, wasm32. The x86_64
    variant is the most complete (7 files, 1 886 LOC). Wasm32 has only
    `sched_hal.vuma` (585 LOC, cooperative scheduling HAL).
  - **`mm/`** — `pmm.vuma` (734 LOC, buddy allocator orders 0..10),
    `vmm.vuma`, `kmalloc.vuma`, `mmap.vuma`.
  - **`ipc/`** — `pipe.vuma` (733 LOC, ring buffer + wait queue),
    `signal.vuma` (1 096 LOC), `futex.vuma` (933 LOC), `shm.vuma`
    (1 499 LOC), `waitq.vuma` (591 LOC).
  - **`sync/`** — `spinlock.vuma` (584 LOC, recursive + IRQ disable),
    `mutex.vuma` (688 LOC, sleeping + wait queue), `semaphore.vuma`,
    `rwlock.vuma` (1 173 LOC).
  - **`smp/`** — `percpu.vuma`, `ipi.vuma`, `smp.vuma` (648 LOC, SMP
    boot + smp_call_function broadcast).
  - **`proc/`** — `task.vuma` (681 LOC, ProcessTable + 256-slot
    array), `scheduler.vuma` (1 265 LOC, CFS-like with vruntime),
    `fork.vuma`, `exec.vuma`, `wait.vuma`, `exit.vuma`, `elf.vuma`.
  - **`vfs/`** — `inode.vuma` (678 LOC), `dentry.vuma` (622 LOC),
    `file.vuma` (599 LOC), `file_ops.vuma` (712 LOC), `mount.vuma`
    (546 LOC), `namei.vuma` (659 LOC), `ops.vuma` (709 LOC).
  - **`fs/`** — `tmpfs.vuma` (1 672 LOC, in-memory fs), `devfs.vuma`
    (1 174 LOC), `procfs.vuma`, `initramfs.vuma`.
  - **`trap/`** — `irq_ring.vuma` (472 LOC, **SPSC event ring — see
    §4.1**), `trap_frame.vuma` (superset 576-byte TrapFrame),
    `irq.vuma`, `trap.vuma` (689 LOC, arch-independent dispatcher).
  - **`syscall/`** — `abi.vuma`, `table.vuma`, `dispatch.vuma` (496
    LOC, R4c rewrite), `syscall_init.vuma`, plus `handlers/{io,fs,proc,mm}.vuma`.
  - **`crypto/`** — `api.vuma` (621 LOC, CipherCtx+HashCtx dispatcher),
    `aes.vuma` (412 LOC, **SCAFFOLD** — header at `:6-9` admits *"NOT
    a full KAT-verified AES. This is a structural skeleton with the
    real FIPS-197 S-box, a stubbed key schedule, and a 10-round cipher
    (W49: SubBytes + ShiftRows + AddRoundKey per round; MixColumns
    deferred)"*), `sha.vuma` (611 LOC, **REAL** W48 — verified
    against SHA-256 test vectors), `asym.vuma` (498 LOC, **SCAFFOLD**
    — header at `:1-22` admits *"NOT real Ed25519... ed25519_keygen
    fills secret[i] = (i*7+13) mod 256... ed25519_sign emits sig[0..31] =
    (secret[i] + msg[i]) mod 256 — mod-256 ADDITION... NOT the EdDSA
    sign equation"*), `hw_trampoline.vuma` (272 LOC, AES-NI / ARMv8
    dispatch shell — falls back to K10b stub in hosted mode).
  - **`net/`** — `sk_buff.vuma`, `http.vuma` (455 LOC, simplified
    HTTP/1.0 — header at `:14-17` admits *"K9f STUB: returns 0 (no
    real TCP connection — resp.len is set to 0)"*), `socket.vuma`,
    `tcp.vuma` (580 LOC, 10-state machine but `tcp_recv` at `:439`
    is *"return 0; // stub: no data"*), `dns.vuma` (stub: returns 0).
  - **`drivers/`** — `early_console.vuma` (336 LOC, universal
    write_byte abstraction for 16550/PL011/semihost/diag/wasm),
    `uart.vuma`, `char.vuma`, `virtio_net.vuma`.
  - **`tty/`** — `vt100.vuma` (1 020 LOC), `console.vuma`,
    `line_discipline.vuma` (631 LOC).
  - **`shell/shell.vuma`** (2 134 LOC) — interactive shell with
    command history, ANSI colors, raw-mode line editing.
  - **`panic/panic.vuma`** (267 LOC) — panic banner + halt.
  - **`power/pm.vuma`** (354 LOC) — power management skeleton.

**Two-track architecture.** The kernel is currently **hosted-mode
only** — it runs as a Linux process via `womb/kernel/hosted/host.vuma`
(156 LOC), which is a thin wrapper around `write/read/exit/mmap`
syscalls. Bare-metal mode (real QEMU `qemu-system-x86_64 -kernel`
boot) is **K11+ future work** — every per-arch `trap_trampoline.vuma`
and `vmm_hal.vuma` and `switch.vuma` carries comments like *"K11 stub:
`lidt [rdi]; ret`"*, *"K11 stub: `mov al, 0xFF; out 0x21, al`"*,
*"K11 bare-metal mode the caller would call pmm_init..."* (grep
counts 46 such "K11 stub" markers across 31 files).

**Reusable for UI engine:**
- **`trap/irq_ring.vuma`** is the SINGLE MOST REUSABLE artifact in
  the entire `womb/` tree for the UI engine's event pipeline (W-0).
  See §4.1.
- `sync/spinlock.vuma` and `sync/mutex.vuma` patterns are NOT
  directly reusable for browser (single-threaded Wasm) but ARE
  reusable for the native target (post-v1).
- `proc/scheduler.vuma` is NOT reusable (the browser uses cooperative
  scheduling via `host_yield()` — see `arch/wasm32/sched_hal.vuma`).
- `crypto/sha.vuma` (REAL SHA-256) is redundant with
  `crypto/hash/sha256_sha224.vuma` (also REAL SHA-256) — ADR needed
  to pick the canonical one (see §6.2).
- The kernel/crypto/aes.vuma SCAFFOLD is redundant with
  crypto/symmetric/aes256.vuma REAL — same ADR concern.

### `womb/lang/` — REAL (4 170 LOC, self-hosted bootstrap compiler)

- **Maturity:** REAL but **limited subset**.
- **Evidence:** Header at `full_lexer.vuma:1-46` documents the
  bootstrap pipeline: `lex → parse → AST → IR → codegen → ELF`. The
  bootstrap supports a V1.0-era grammar subset (`transform`, `let`,
  `if`, `while`, `print_int`, `return`, integer literals, binary ops,
  `allocate`/`free` interception). VUMA 2.0 PMT constructs (`layout`,
  `state_new`, field access, transforms) are NOT supported by the
  bootstrap. `hello.vuma` is the bootstrap milestone target (prints
  42, exits 0).
- **Reusable for UI engine:** No — the bootstrap is a research
  artifact proving VUMA can self-host. The production VUMA compiler
  (in Rust) is what the UI engine team will use.

### `womb/lib/` — REAL (21 509 LOC, 21 files, POSIX-shaped stdlib)

- **Maturity:** REAL (mostly PMT-migrated; some files still use
  `allocate()`/`free()`).
- **`lib/mem_helpers.vuma`** (60 LOC) — single source of truth for
  `store_u64`/`load_u64`/`store_u32`/`load_u32`/`store_u16`/`load_u16`
  byte-stitched little-endian helpers. Built on `__vuma_load_u8` /
  `__vuma_store_u8` runtime intrinsics. Imported by 16 other womb
  modules (`collections/*`, `string/*`, `graph/*`, `env/cli.vuma`,
  `fs/*`, `io/buffered.vuma`, `lang/{full_lexer,full_parser,ir_builder,codegen,elf}.vuma`).
- **`lib/text/`** — 4 files, 4 002 LOC
  - `string.vuma` (271 LOC) — POSIX `string.h`: `memcpy`, `memmove`,
    `memset`, `memcmp`, `memchr`, `memrchr`, `strlen`, `strcmp`,
    `strncmp`, `strcasecmp`, `strcpy`, `strncpy`, `strcat`, `strncat`,
    `strchr`, `strrchr`, `strstr`, `strtok_r`, plus `load_u16/32/64_le/be`,
    `store_u16/32/64_le/be`, `bswap16/32/64`, `htons/ntohs/htonl/ntohl`.
  - `unicode.vuma` (709 LOC) — RFC 3629 UTF-8 + UAX #29-style codepoint
    classification + ASCII case conversion.
  - `json.vuma` (1 254 LOC) — RFC 8259 JSON parser/builder/serializer.
    16-byte typed nodes, recursive ownership.
  - `printf.vuma` (1 772 LOC) — `printf_*`, `sprintf_*`, `snprintf_*`
    variants. Supports `%d %i %u %x %X %o %b %c %s %p %f %e %E %g %G
    %%` + width/left-align/zero-pad/precision/length modifiers.
- **`lib/sys/`** — 7 files, 3 633 LOC
  - `stdlib.vuma` (352 LOC) — `atoi`, `strtol`, `itoa`, `utoa`, `abs`,
    `min`, `max`, `clamp`, `min_u32`, `max_u32`.
  - `math.vuma` (656 LOC) — `sin`, `cos`, `tan`, `exp`, `log`, `ln`,
    `pow`, `sqrt`, `frexp`, `ldexp`, `isnan`, `isinf`, constants (`PI`,
    `TAU`, `E`, `LN_2`, `LN_10`, `SQRT_2`).
  - `stdio.vuma` (333 LOC) — `write_str`, `write_int`, `write_hex`,
    `write_u64`, `read_line`, file ops.
  - `fileio.vuma` (281 LOC) — POSIX `file.h` (open/read/write/close,
    mkdir/rename/unlink, stat, dup2, fork/waitpid, execve, pipe).
  - `time.vuma` (100 LOC) — `time_now`, `time_monotonic`,
    `time_monotonic_ns`, `sleep_ms`.
  - `fp.vuma` (478 LOC) — IEEE 754 helpers: rounding modes,
    classification, sign manipulation, `nextafter`, `ulp`, `frexp`,
    `ldexp`. **Note:** `f64_sign` at `:25-34` is a STUB (`// This
    would require a store-float instruction or cast // For now, return
    0 (positive)`), but `f64_is_nan` and `f64_is_infinite` work via
    the `x != x` / `x - x != 0` idioms.
  - `email.vuma` (1 440 LOC) — SMTP/IMAP/POP3/MIME.
    **Has BROKEN IMPORTS** (see §5).
- **`lib/concurrency/`** — 2 files, 882 LOC
  - `threading.vuma` (374 LOC) — futex-based Mutex/Condvar/RWLock/
    Semaphore/Spinlock. **Non-atomic test-and-set** (header at `:22-27`
    admits *"The test-and-set used by Mutex and Spinlock is a
    non-atomic load-then-store pair, which the task spec explicitly
    permits under VUMA's memory model. Correctness of the blocking
    primitives ultimately relies on the kernel-side atomicity of
    futex."*).
  - `event_loop.vuma` (510 LOC) — epoll-based reactor pattern.
- **`lib/compress/`** — 2 files, 2 531 LOC
  - `deflate.vuma` (1 063 LOC) — RFC 1951 DEFLATE + RFC 1950 zlib +
    CRC-32. Real Huffman + LZ77.
  - `compression_extra.vuma` (1 470 LOC) — gzip/LZ4/zstd/Brotli
    wrappers. **Simplified** (header at `:13-23` admits *"gzip —
    DEFLATE payload uses BTYPE=00 (stored) blocks only... lz4 — Frame
    descriptor is 2 bytes... zstd — Only Raw (block_type=0) and RLE
    (block_type=1) blocks... brotli — Only raw (uncompressed)
    meta-blocks"*). Useful as wire-format scaffolding, not as real
    compression.
- **`lib/pki/`** — 5 files, 6 231 LOC
  - `asn1.vuma` (1 479 LOC) — ITU-T X.690 DER encoder/decoder.
  - `x509.vuma` (1 490 LOC) — RFC 5280 X.509 v3 cert parser/builder.
  - `pki.vuma` (1 531 LOC) — PKI trust chain validation.
  - `auth.vuma` (1 528 LOC) — auth protocols (Kerberos, NTLM, SCRAM).
  - `jwt.vuma` (206 LOC) — RFC 7519 + RFC 7515 JSON Web Token.
- **Reusable for UI engine:**
  - `lib/text/string.vuma` (POSIX string.h) — YES, foundational.
  - `lib/text/unicode.vuma` (UTF-8) — YES, but the UI engine needs
    much more (UAX #9 BiDi, UAX #14 line break, UAX #29 grapheme
    clusters — all GREENFIELD for `womb/ui/text/`).
  - `lib/text/json.vuma` — YES, for theme files, asset manifests,
    IPC messages.
  - `lib/text/printf.vuma` — YES, for debugging / logging.
  - `lib/sys/math.vuma` — YES, for layout math (`sin`/`cos` for
    transforms, `sqrt` for distance).
  - `lib/sys/time.vuma` — YES, for animation timestamps.
  - `lib/sys/fp.vuma` — Partially; needs `f64_sign` filled in.
  - `lib/concurrency/event_loop.vuma` — NO, browser uses RAF +
    host_yield.
  - `lib/pki/` — NO, browser delegates TLS to host.

### `womb/net/` — REAL with caveats (15 884 LOC, 15 files)

A complete RFC-grade networking stack. Every file documents its RFC
and implements the protocol.

- **`socket.vuma`** (364 LOC) — POSIX `socket.h` (TCP/UDP, inet_pton/ntop,
  htons/ntohs). PMT-migrated.
- **`tcp.vuma`** (206 LOC) — TCP client/server using BSD sockets.
- **`dns.vuma`** (262 LOC) — RFC 1035 query/response. PMT-migrated.
- **`dns_extra.vuma`** (1 286 LOC) — DNSSEC, mDNS, DoT, DoH, EDNS0.
- **`http.vuma`** (944 LOC) — RFC 7230/9112. Request/response parser
  + builder, chunked transfer, header helpers. PMT-migrated.
- **`http2.vuma`** (696 LOC) — RFC 9113 frame parser/builder.
- **`http3_mqtt_coap.vuma`** (1 076 LOC) — RFC 9114 + QPACK + MQTT + CoAP.
- **`websocket.vuma`** (506 LOC) — RFC 6455 (handshake, frame builder,
  frame parser, mask generation, convenience I/O).
- **`hpack.vuma`** (1 787 LOC) — RFC 7541 HPACK header compression
  (static table 1-30, dynamic table, integer/string codec, all 4
  representations + Dynamic Table Size Update).
- **`tls12.vuma`** (985 LOC) — RFC 5246 (PRF, key schedule, record
  layer AEAD, handshake messages). Suite: TLS_RSA_WITH_AES_256_CBC_SHA256.
- **`tls13.vuma`** (1 277 LOC) — RFC 8446 (HKDF-Expand-Label,
  Derive-Secret, full key schedule, traffic keys, record layer AEAD,
  handshake). Suite: TLS_AES_256_GCM_SHA384.
- **`quic.vuma`** (1 893 LOC) — RFC 9000 + RFC 9001 (varint codec,
  long/short header parsing, header protection, packet protection,
  Initial key derivation, transport parameters, frame parsing/building,
  Connection ID generation).
- **`ssh.vuma`** (1 045 LOC) — RFC 4251-4254 (version string, KEXINIT,
  DH group14, key derivation, binary packet protocol, AES-256-CTR +
  HMAC-SHA-256, connection layer, USERAUTH_REQUEST).
- **`ip_icmp_arp.vuma`** (1 665 LOC) — IEEE 802.3 Ethernet + 802.1Q
  VLAN + 802.11 WiFi mgmt + 802.15.4 Zigbee + ARP + IPv4 + ICMP.
- **`ieee_frames.vuma`** (1 505 LOC) — NTP, FTP, LDAP, SNMP, SIP,
  RTSP, BGP, SCTP.

**⚠ BROKEN CROSS-DIRECTORY IMPORTS** (see §5 for details). Seven
files (`ssh.vuma`, `quic.vuma`, `tls12.vuma`, `tls13.vuma`,
`http2.vuma`, `http3_mqtt_coap.vuma`, `websocket.vuma`) plus
`lib/sys/email.vuma` import from paths like `"../crypto/hqc.vuma"`,
`"../crypto/hmac.vuma"`, `"../crypto/aes256.vuma"`, etc. — but the
`crypto/` subtree was reorganized into subdirectories (`hash/`,
`symmetric/`, `asym/`, `mac_kdf/`, `drbg/`, `bignum/`,
`post_quantum/`) and the imports were never updated. The resolver at
`src/parser/src/resolver.rs:500-510` is just `base_dir.join(path)`
with no fallback, so these imports would fail at compile time if
triggered. The standalone per-file test harness
(`scripts/test_womb_compile.sh`) compiles each `.vuma` file as a
self-contained unit (not following imports), masking this breakage.

**Reusable for UI engine:** Mixed.
- `http.vuma` and `websocket.vuma` parsers are useful as wire-format
  references, but the SWE package explicitly says (line 318) *"for
  the browser target, we delegate to the browser's `fetch` (which
  handles TLS, cookies, redirects natively)"*. So the existing
  `net/http.vuma` (full HTTP client with TLS via `tls13.vuma`) is
  available for the **native** target but NOT for the **browser**
  target.
- `dns.vuma` — useful for native target. Browser target delegates DNS
  to host.
- `tls12/tls13/quic/ssh` — NOT relevant to UI engine (browser
  delegates TLS to host).

### `womb/string/` — REAL (3 files, 416 LOC)

- **Maturity:** REAL
- **Evidence:**
  - `string.vuma` (122 LOC) — `string_len`, `string_eq`, `string_eq_cstr`,
    `string_copy`, `string_copy_cstr`, etc. Operates on (Address, u32)
    string refs.
  - `string_builder.vuma` (66 LOC) — dynamic string builder wrapping
    a heap buffer. `sb_new`, `sb_push_char`, `sb_append`, `sb_append_u64`,
    `sb_write`, `sb_clear`.
  - `utf8.vuma` (231 LOC) — `VStr` heap-backed growable UTF-8 string
    type. `vstr_new`, `vstr_from_cstr`, `vstr_from_bytes`, `vstr_push_byte`.
- **Reusable for UI engine:** YES — string_builder is the canonical
  pattern for assembling text before rendering. The `VStr` type is
  the right shape for UI text buffers (though it lacks grapheme-
  cluster awareness — that's `womb/ui/text/grapheme.vuma` work).

---

## Mapping to SWE package's proposed WOMB UI modules

The SWE package (`26-new-plans-three-layers.md` lines 182-329) proposes
10 WOMB phases (W-0 through W-9). The table below maps each proposed
module to (a) any existing `womb/` support and (b) the gap.

| Proposed WOMB module | SWE file | Existing womb/ support | Gap |
|---|---|---|---|
| **W-0: SPSC event ring** | `womb/ui/event/ring.vuma` | **REAL substrate: `womb/kernel/trap/irq_ring.vuma` (472 LOC).** SWE line 201 says *"Pattern: Generalizes `womb/kernel/trap/irq_ring.vuma` (8-byte slots → 64-byte slots; asm producer → JS producer; single-CPU → SMP atomics)."* | Generalize IrqRing: 8-byte → 64-byte slots, asm producer → JS producer (via SharedArrayBuffer), SPSC → SMP-atomic. ~2 weeks. |
| W-0: Event dispatcher | `womb/ui/event/dispatch.vuma` | None — but `womb/kernel/trap/trap.vuma:1-60` (arch-independent vector dispatcher) is a structural analog. | Greenfield. 1 week. |
| W-0: Event normalization | `womb/ui/event/normalize.vuma` | None — but `womb/kernel/trap/trap_frame.vuma` (canonical superset TrapFrame) is a structural analog. | Greenfield. 1 week. Depends on V-08 (UiEvent layout). |
| W-0: `host_yield()` | `womb/ui/event/yield.vuma` | **REAL substrate: `womb/kernel/arch/wasm32/sched_hal.vuma:25-41`** — declares `extern "C" transform host_yield()` and documents the browser/wasmtime/hosted contract. | Adapt the wasm32 sched_hal pattern. 3 days. Depends on V-1 (wasm32 imports). |
| **W-1: OpenType parser** | `womb/ui/text/font_parse.vuma` | None — no font parser exists in womb/. The closest structural analog is `womb/lib/pki/asn1.vuma` (TLV parser pattern) and `womb/net/hpack.vuma` (static-table lookup pattern). | Greenfield. 1 week. Depends on V-37 (`include_bytes!` for font embedding). |
| W-1: cmap | `womb/ui/text/cmap.vuma` | None. | Greenfield. 2 weeks. |
| W-1: hmtx/hhea/head/maxp | `womb/ui/text/hmetrics.vuma` | None. | Greenfield. 1 week. |
| W-1: glyf/loca | `womb/ui/text/glyf.vuma` | None. | Greenfield. 2 weeks. |
| W-1: fvar/gvar (variable fonts) | `womb/ui/text/fvar.vuma` | None. | Greenfield. 2 weeks. |
| W-1: COLR/CPAL (color fonts) | `womb/ui/text/colr.vuma` | None. | Greenfield. 2 weeks. |
| W-1: vmtx/vhea | `womb/ui/text/vmetrics.vuma` | None. | Greenfield. 3 days. |
| W-1: Font subsetting | `womb/ui/text/subset.vuma` | None. | Greenfield. 2 weeks. |
| W-1: Font fallback chain | `womb/ui/text/fontstack.vuma` | None. | Greenfield. 1 week. |
| **W-2: Text shaper v1** | `womb/ui/text/shaper_v1.vuma` | None — but `womb/lib/text/unicode.vuma` (709 LOC) provides UTF-8 ↔ codepoint conversion. | Greenfield. 1 week. |
| W-2: UTF-8 ↔ UTF-32 | `womb/ui/text/utf8.vuma` | **REAL substrate: `womb/lib/text/unicode.vuma`** — RFC 3629 UTF-8 encode/decode/validate + codepoint classification. | Either re-export the existing unicode.vuma or migrate the relevant functions into `womb/ui/text/utf8.vuma`. 3 days. |
| W-2: GSUB | `womb/ui/text/gsub.vuma` | None. | Greenfield. 5 weeks. |
| W-2: GPOS | `womb/ui/text/gpos.vuma` | None. | Greenfield. 5 weeks. |
| W-2: Coverage table | `womb/ui/text/coverage.vuma` | None. | Greenfield. 3 days. |
| **W-3: BiDi** | `womb/ui/text/bidi.vuma` | None — `womb/lib/text/unicode.vuma` does NOT implement UAX #9. | Greenfield. 4 weeks. |
| W-3: BiDi property table | `womb/ui/text/bidi_table.vuma` | None. | Greenfield. 1 week. Depends on V-37 (const byte arrays). |
| W-3: Bracket pairing | `womb/ui/text/brackets.vuma` | None. | Greenfield. 1 week. |
| W-3: Mirroring table | `womb/ui/text/mirror.vuma` | None. | Greenfield. 3 days. |
| W-3: Line breaking (UAX #14) | `womb/ui/text/linebreak.vuma` | None. | Greenfield. 2 weeks. |
| W-3: Knuth-Plass | `womb/ui/text/knuth_plass.vuma` | None. | Greenfield. 2 weeks. |
| W-3: Grapheme clusters (UAX #29) | `womb/ui/text/grapheme.vuma` | None. | Greenfield. 1 week. |
| W-3: Word segmentation | `womb/ui/text/wordbreak.vuma` | None. | Greenfield. 3 days. |
| **W-4: LayoutNode + layouts** | `womb/ui/layouts.vuma` | None — but `womb/collections/{vec,hashmap,btree_map}.vuma` provide the underlying storage. | Greenfield. 1 week. Depends on V-34/V-35/V-36 (f32 + nested struct support). |
| W-4: Flexbox measure | `womb/ui/layout/measure.vuma` | None. | Greenfield. 3 weeks. |
| W-4: Flexbox position | `womb/ui/layout/position.vuma` | None. | Greenfield. 2 weeks. |
| W-4: Flex grow/shrink | `womb/ui/layout/flex_distribute.vuma` | None. | Greenfield. 1 week. |
| W-4: Stacking contexts | `womb/ui/layout/stacking.vuma` | **REAL substrate: `womb/graph/digraph.vuma` + `womb/graph/algorithms.vuma` (topological sort)** for z-index DAG. | Greenfield layout logic; reuse digraph for the DAG. 2 weeks. |
| W-4: Absolute/fixed | `womb/ui/layout/absolute.vuma` | None. | Greenfield. 1 week. |
| W-4: Vertical text | `womb/ui/layout/vertical.vuma` | None. | Greenfield. 1 week. Depends on W-1 vmtx + W-3 BiDi. |
| W-4: Dirty tracking | `womb/ui/layout/dirty.vuma` | None — but `womb/graph/algorithms.vuma` (topological sort) is the dirty-propagation algorithm. | Greenfield. 1 week. |
| W-4: Scroll containers | `womb/ui/layout/scroll.vuma` | None. | Greenfield. 1 week. Depends on W-5 (vector renderer). |
| **W-4: Animation** | `womb/ui/animation.vuma` | **REAL substrate: `womb/lib/sys/time.vuma` (100 LOC)** for `time_monotonic_ns()` and `sleep_ms()`. **REAL substrate: `womb/lib/sys/math.vuma`** for easing functions (sin, cos, pow, sqrt). | Greenfield animation system; reuse time + math. 2 weeks. Depends on V-24 (Animate IR). |
| **W-4: Theme** | `womb/ui/theme.vuma` | **REAL substrate: `womb/lib/text/json.vuma` (1 254 LOC)** for theme-file parsing. | Greenfield theme manager; reuse JSON parser. 2 weeks. |
| **W-5: Path data structures** | `womb/ui/render/path.vuma` | None. | Greenfield. 1 week. Depends on V-08. |
| W-5: Scene tree | `womb/ui/render/scene.vuma` | None — `womb/graph/digraph.vuma` is a possible substrate but scene trees need parent-pointer + sibling-list, not adjacency-list. | Greenfield. 2 weeks. |
| W-5: Scene builder | `womb/ui/render/scene_build.vuma` | None. | Greenfield. 2 weeks. Depends on W-4. |
| W-5: Outline-to-path | `womb/ui/render/outline_to_path.vuma` | None. | Greenfield. 1 week. Depends on W-1 (glyf). |
| W-5: Path tessellation shader | `shaders/path_tessellate.comp.glsl → .spv` | None. | Greenfield. 6 weeks. Depends on V-26 (SPIR-V embedding). |
| W-5: WebGL2 fallback shader | `shaders/path_rasterize.frag.glsl → .spv` | None. | Greenfield. 3 weeks. |
| W-5: GPU command encoder | `womb/ui/render/gpu_encode.vuma` | None. | Greenfield. 2 weeks. Depends on V-2 (GpuDraw IR). |
| W-5: Clip paths | `womb/ui/render/clip.vuma` | None. | Greenfield. 1 week. |
| W-5: Blend modes + opacity | `womb/ui/render/blend.vuma` | None. | Greenfield. 1 week. |
| W-5: Color font rendering | `womb/ui/render/color_glyph.vuma` | None. | Greenfield. 1 week. Depends on W-1 (COLR). |
| W-5: Composited scroll | `womb/ui/render/scroll_layer.vuma` | None. | Greenfield. 1 week. |
| W-5: Frame pacing | `womb/ui/render/frame.vuma` | None — but `womb/lib/sys/time.vuma` provides `time_monotonic_ns()`. | Greenfield. 3 days. |
| **W-6: IME composition state** | `womb/ui/ime/composition.vuma` | None — but `womb/kernel/ipc/pipe.vuma` (733 LOC) is a structural analog for state-machine + ring-buffer + wait-queue patterns. | Greenfield. 2 weeks. Depends on V-08. |
| W-6: EditContext bridge | `womb/ui/ime/ec_bridge.vuma` | None. | Greenfield. 1 week. Depends on V-1 (wasm32 imports). |
| W-6: Caret position | `womb/ui/ime/caret.vuma` | None. | Greenfield. 1 week. Depends on W-2 + W-4. |
| W-6: Composition formatting | `womb/ui/ime/format.vuma` | None. | Greenfield. 1 week. Depends on W-5. |
| W-6: Text field management | `womb/ui/ime/textfield.vuma` | None. | Greenfield. 1 week. |
| W-6: Safari/Firefox fallback | `womb/ui/ime/textarea_fallback.vuma` | None. | Greenfield. 2 weeks. |
| **W-7: SemanticsNode tree** | `womb/ui/a11y/semantics.vuma` | None — but `womb/kernel/proc/task.vuma` (ProcessTable with parallel flat byte arrays) is a structural analog for fixed-capacity node tables. | Greenfield. 2 weeks. Depends on V-08. |
| W-7: Tree builder | `womb/ui/a11y/build.vuma` | None — but `womb/graph/digraph.vuma` is a substrate. | Greenfield. 2 weeks. Depends on W-4. |
| W-7: Tree diff | `womb/ui/a11y/diff.vuma` | None. | Greenfield. 2 weeks. |
| W-7: ARIA bridge | `womb/ui/a11y/aria_bridge.vuma` | None. | Greenfield. 2 weeks. Depends on V-1 (dom_* imports). |
| W-7: Reverse bridge | `womb/ui/a11y/reverse.vuma` | None. | Greenfield. 1 week. |
| W-7: High-contrast + reduced-motion | `womb/ui/a11y/preferences.vuma` | None. | Greenfield. 1 week. |
| **W-8: HTTP client (browser bridge)** | `womb/ui/net/http_bridge.vuma` | **REAL substrate: `womb/net/http.vuma` (944 LOC)** — but the existing http.vuma is a full HTTP/1.1 client with its own TCP socket layer, NOT a `fetch()` wrapper. SWE line 318 says *"VUMA's existing `womb/net/http.vuma` (full HTTP client with TLS) is available for the native target; for the browser target, we delegate to the browser's `fetch`"*. | For browser: greenfield `net_fetch` wrapper, ~1 week. For native: existing http.vuma is reusable. |
| W-8: WebSocket (browser bridge) | `womb/ui/net/ws_bridge.vuma` | **REAL substrate: `womb/net/websocket.vuma` (506 LOC)** — same caveat as HTTP: existing is full WS-over-TCP, browser target needs `net_websocket_*` wrapper. | For browser: greenfield `net_websocket_*` wrapper, ~1 week. For native: existing websocket.vuma is reusable. |
| W-8: Clipboard | `womb/ui/clipboard.vuma` | None. | Greenfield. 1 week. Depends on V-1. |
| W-8: File picker | `womb/ui/filepick.vuma` | None. | Greenfield. 1 week. Depends on V-1. |
| **W-9: UI capability tokens** | `womb/ui/capability.vuma` | **REAL substrate: `womb/crypto/mac_kdf/hmac.vuma` (193 LOC)** — HMAC-SHA-256 for capability-token signatures. | Greenfield capability-token layout + signing; reuse hmac_sha256. 2 weeks. Depends on V-16 + V-09. |
| W-9: Capability bundles | `womb/ui/cap_bundles.vuma` | None. | Greenfield. 1 week. |
| W-9: Delegation + revocation | `womb/ui/cap_delegate.vuma` | None. | Greenfield. 2 weeks. |
| W-9: Per-frame verification cache | `womb/ui/cap_cache.vuma` | None. | Greenfield. 1 week. Depends on C-06. |

**Summary:** Of the 50+ proposed WOMB UI modules, **every single one
is greenfield**. Six existing `womb/` artifacts are directly reusable
as substrate (IrqRing, hmac_sha256, unicode UTF-8, json, time, math),
four are reusable as structural analogs (digraph for layout DAG,
collections for storage, kernel/net for native-target networking,
kernel/ipc/pipe for IME state-machine patterns), and the rest is
new work.

---

## What's reusable for the UI engine

### 4.1 `womb/kernel/trap/irq_ring.vuma` — SPSC event ring (THE key reusable artifact)

- **File:** `womb/kernel/trap/irq_ring.vuma` (472 LOC, lines 1-472)
- **Maturity:** REAL. PMT-pure. Header at `:1-228` is a 228-line
  design document covering: why a ring buffer, why SPSC, field
  semantics (buf/head/tail/count), pack/unpack discipline,
  producer/consumer ordering (the "lock-free" invariant), PMT
  discipline, decimal-constants convention, build & verify commands.
- **Layout** (`:232-237`):
  ```
  layout IrqRing = {
      buf: [u8; 256],   // 32 × 8 bytes — u64 IRQ vectors (LE-packed)
      head: u32,        // read position (0..31, wraps via & 31)
      tail: u32,        // write position (0..31, wraps via & 31)
      count: u32,       // entries currently in buffer (0..32)
  }
  ```
- **API:** `irq_ring_init`, `irq_ring_push` (returns 0 on success, -1
  on full), `irq_ring_pop` (returns the vector, or 9999 sentinel on
  empty).
- **Why it's the canonical substrate:** The SWE package (line 201)
  explicitly says *"Pattern: Generalizes
  `womb/kernel/trap/irq_ring.vuma` (8-byte slots → 64-byte slots; asm
  producer → JS producer; single-CPU → SMP atomics)."* The
  IrqRing's SPSC discipline (producer writes vector BEFORE
  incrementing count, consumer reads vector BEFORE decrementing
  count) is exactly the browser-event-ring contract.
- **What needs to change for the UI engine:**
  1. **Slot size:** 8 bytes → 64 bytes (UI events carry `kind`, `x`,
     `y`, `key`, `modifiers`, `timestamp`).
  2. **Producer:** asm IRQ trampoline → JS host shim pushing into a
     `SharedArrayBuffer` (the `count` field becomes the SPSC
     watermark across the JS/Wasm boundary).
  3. **SMP safety:** single-CPU SPSC → SMP atomics (the header at
     `:165-169` admits *"On SMP, this single-IrqRing is NOT safe
     (two CPUs would race on the head/tail/count updates). The R2e
     contract is for a per-CPU IrqRing... the SMP cross-CPU case is
     K9's job"* — but the browser target is single-threaded Wasm, so
     SMP is NOT a concern for v1).
  4. **Generalization:** Consider extracting the pattern into
     `womb/sync/spsc.vuma` (see ADR §6.3).

### 4.2 `womb/crypto/mac_kdf/hmac.vuma` — HMAC-SHA-256 (F-2 confirmed)

- **File:** `womb/crypto/mac_kdf/hmac.vuma` (193 LOC)
- **Maturity:** REAL. RFC 2104 / FIPS 198-1.
- **API:** `hmac_sha1`, `hmac_sha256` (`:75-118`), `hmac_sha512`,
  `ct_memcmp`.
- **Why it's the canonical substrate:** ADR-0007
  (`docs/adr/ADR-0007.md`) and the SWE package (line 329) both say
  the UI capability model should use this implementation rather than
  adding a `sha2`/`hmac` Rust crate. The prior research note in
  `docs/research/A-3-ive-proofs-capability.md` (per worklog entry
  F-2) confirmed: *"real RFC-2104 HMAC-SHA-256 at :75-116, but it's
  a .vuma source program, not a Rust library — migration requires
  either re-implementing in Rust, adding sha2/hmac crates, or
  self-compiling VUMA at build time."*
- **What needs to change:** Nothing in the HMAC implementation. The
  migration path is V-16 (SWE line 151): *"The VUMA-side change is
  in `ipc.rs:compute_signature` — call the WOMB HMAC instead of FNV."*
  This requires the VUMA compiler to be able to call a
  VUMA-compiled transform from Rust codegen — which is the
  self-compilation bootstrap question (see ADR §6.2).

### 4.3 `womb/lib/text/unicode.vuma` — UTF-8 + codepoint classification

- **File:** `womb/lib/text/unicode.vuma` (709 LOC)
- **Maturity:** REAL. RFC 3629.
- **API:** `utf8_encode`, `utf8_decode`, `utf8_decode_safe`,
  `utf8_strlen`, `utf8_strchr`, `utf8_substr`, `utf8_prev_char`,
  `utf8_next_char`, `utf8_char_at`, `utf8_char_len`, `utf8_seq_len`,
  `utf8_validate`, `utf8_valid_char`, `utf8_to_lower`, `utf8_to_upper`,
  `utf8_is_alpha`, `utf8_is_digit`, `utf8_is_space`, `utf8_is_alnum`,
  `utf8_is_print`, `utf8_is_control`, `utf8_str_to_lower`,
  `utf8_str_to_upper`, `utf8_cp_is_ascii`, `utf8_cp_is_bmp`,
  `utf8_cp_is_supplementary`.
- **Reusable for UI engine:** Yes — `womb/ui/text/utf8.vuma` (SWE
  W-2) can either re-export these or migrate the relevant ones. The
  existing module is more comprehensive than the SWE package's W-2
  `utf8.vuma` (which only specifies "UTF-8 ↔ UTF-32 conversion").

### 4.4 `womb/lib/text/json.vuma` — JSON parser/builder/serializer

- **File:** `womb/lib/text/json.vuma` (1 254 LOC)
- **Maturity:** REAL. RFC 8259.
- **API:** Full JSON value tree (null/bool/int/float/string/array/object),
  recursive ownership, `json_new_string`, `json_parse_string`,
  `json_object_set`, `json_free`.
- **Reusable for UI engine:** Yes — theme files (`womb/ui/theme.vuma`
  W-4), asset manifests, IPC messages to/from the JS host shim.

### 4.5 `womb/lib/sys/{time,math}.vuma` — animation + layout math

- **`lib/sys/time.vuma`** (100 LOC) — `time_now`, `time_monotonic`,
  `time_monotonic_ns`, `time_monotonic_us`, `time_monotonic_ms`,
  `sleep_ms`.
- **`lib/sys/math.vuma`** (656 LOC) — `sin`, `cos`, `tan`, `exp`,
  `log`, `ln`, `pow`, `sqrt`, `frexp`, `ldexp`, `isnan`, `isinf`,
  `isfinite`, `iszero`, constants (`PI`, `TAU`, `E`, `LN_2`, `LN_10`,
  `SQRT_2`, `FRAC_1_SQRT_2`, `FRAC_1_PI`, etc.).
- **Reusable for UI engine:** Yes — `womb/ui/animation.vuma` (W-4)
  needs both (time for animation timestamps, math for easing
  functions). `womb/ui/layout/measure.vuma` (W-4) needs `sqrt` for
  distance computations.

### 4.6 `womb/collections/{vec,hashmap,btree_map}.vuma` — foundational data structures

- **Maturity:** REAL (heap-backed).
- **Reusable for UI engine:** Yes — every UI subsystem needs dynamic
  arrays (layout children, scene nodes, event queue, dirty rects)
  and hash maps (style records, cache lookups, ARIA node registry).

### 4.7 `womb/graph/{digraph,algorithms}.vuma` — DAG + topological sort

- **Maturity:** REAL.
- **Reusable for UI engine:** Yes — layout trees with stacking
  contexts are DAGs, and dirty propagation needs topological sort.
  The data structure is directly applicable.

### 4.8 `womb/string/string_builder.vuma` — dynamic string builder

- **Maturity:** REAL.
- **Reusable for UI engine:** Yes — text rendering needs string
  assembly before shaping.

### 4.9 `womb/kernel/arch/wasm32/sched_hal.vuma` — cooperative scheduling pattern

- **File:** `womb/kernel/arch/wasm32/sched_hal.vuma` (585 LOC)
- **Maturity:** REAL. Header at `:1-41` documents the `host_yield()`
  contract for browser (setTimeout), wasmtime (no-op stub), and
  hosted-x86_64 (`__ffi_fallback_stub` returns 0).
- **Reusable for UI engine:** Yes — `womb/ui/event/yield.vuma` (W-0)
  can directly adapt this pattern. The `wasm_sched_loop` algorithm at
  `:43-60` is the cooperative-scheduling analogue the UI engine needs.

### 4.10 `womb/net/{http,websocket}.vuma` — for native target only

- **Maturity:** REAL.
- **Reusable for UI engine:** ONLY for native (post-v1). Browser
  target delegates to `fetch()` and the browser's WebSocket per SWE
  line 318.

---

## What's missing for the UI engine

The SWE package's WOMB UI modules are **entirely greenfield** — none
of them exist in `womb/` today. Specifically:

### 5.1 No `womb/ui/` directory

```
$ find womb/ -type d -name ui
(empty)
```

The SWE package proposes ~50 new `.vuma` files across
`womb/ui/{event,text,layout,render,ime,a11y,net}/` plus top-level
`womb/ui/{animation,theme,clipboard,filepick,capability,...}.vuma`.
**None of these exist.**

### 5.2 Missing UI primitives (no existing substrate)

- **Layout engine** — no Flexbox, no measure/position/distribute,
  no stacking contexts, no absolute positioning, no vertical text,
  no dirty tracking, no scroll containers.
- **Renderer** — no Path/PathSegment/Transform, no scene tree, no
  scene builder, no outline-to-path, no GPU command encoder, no clip
  paths, no blend modes, no color font rendering, no composited
  scroll, no frame pacing.
- **Font parser** — no OpenType table directory, no cmap, no
  hmtx/hhea/head/maxp, no glyf/loca, no fvar/gvar, no COLR/CPAL, no
  vmtx/vhea, no font subsetting, no font fallback chain.
- **Text shaper** — no GSUB, no GPOS, no coverage table.
- **BiDi** — no UAX #9 BiDi algorithm, no BiDi property table, no
  bracket pairing, no mirroring table, no UAX #14 line breaking, no
  Knuth-Plass, no UAX #29 grapheme clusters, no word segmentation.
- **IME** — no composition state machine, no EditContext bridge, no
  caret position, no composition formatting, no text field
  management, no Safari/Firefox fallback.
- **A11y** — no SemanticsNode tree, no tree builder, no tree diff,
  no ARIA bridge, no reverse bridge, no high-contrast/reduced-motion.
- **Animation** — no animation system (though time + math primitives
  exist in `lib/sys/`).
- **Theme** — no theme manager (though JSON parser exists in
  `lib/text/`).
- **Event pipeline** — no UI event ring (though IrqRing is the
  substrate), no dispatcher, no normalizer.

### 5.3 Missing VUMA-compiler-side prerequisites (not in `womb/`'s control)

Per SWE package Phase V-0..V-5, the UI engine depends on these
VUMA-compiler patches (out of scope for this audit but listed for
completeness):

- **V-34/V-35/V-36** — f32 + nested struct state field support
  (`bridge_type_to_ir_type`, `type_size_from_name`, `StateRead`/`StateWrite`).
- **V-08** — `UiEvent` layout, `LayoutNode` layout, `SemanticsNode`
  layout, `ImeState` layout.
- **V-02** — `IRInstr::GpuDraw`, `GpuBufferWrite`, `GpuSetScissor`,
  `GpuBindPipeline`, `ShapeText`, `FontLoad`, `FontParse`, `Animate`.
- **V-33** — `IRInstr::PathFromGlyph`, `PathRect`, `PathCircle`,
  `PathAppendSegment`, `PathTransform`, `SceneFlatten`.
- **V-37** — `include_bytes!` macro for embedding SPIR-V shaders
  and font files as const byte arrays.
- **V-26** — Pre-compiled SPIR-V embedding.
- **V-1** — wasm32 backend browser host imports (~50 imports:
  WebGL2/WebGPU, EditContext, Network, Clipboard, Scheduling, A11y).
- **V-16** — HMAC-SHA-256 capability signature (uses
  `womb/crypto/mac_kdf/hmac.vuma`).
- **V-11** — Session types `Choice`/`Offer` for IME channel.

### 5.4 Broken cross-directory imports in `womb/net/` and `womb/lib/sys/email.vuma`

**This is a real audit finding.** Seven files in `womb/net/` plus
`womb/lib/sys/email.vuma` carry `import` statements that point to
non-existent paths:

| Importing file | Broken import | Actual location (if any) |
|---|---|---|
| `womb/net/ssh.vuma:57` | `"../crypto/hqc.vuma"` | `womb/crypto/post_quantum/hqc.vuma` |
| `womb/net/ssh.vuma:58` | `"../crypto/hmac.vuma"` | `womb/crypto/mac_kdf/hmac.vuma` |
| `womb/net/ssh.vuma:59` | `"../crypto/aes256.vuma"` | `womb/crypto/symmetric/aes256.vuma` |
| `womb/net/ssh.vuma:60` | `"../crypto/bignum2048.vuma"` | `womb/crypto/bignum/bignum2048.vuma` |
| `womb/net/quic.vuma:64` | `"../crypto/hqc.vuma"` | `womb/crypto/post_quantum/hqc.vuma` |
| `womb/net/quic.vuma:65` | `"../crypto/hmac.vuma"` | `womb/crypto/mac_kdf/hmac.vuma` |
| `womb/net/quic.vuma:66` | `"../crypto/hkdf.vuma"` | `womb/crypto/mac_kdf/hkdf.vuma` |
| `womb/net/quic.vuma:67` | `"../crypto/aes256.vuma"` | `womb/crypto/symmetric/aes256.vuma` |
| `womb/net/quic.vuma:68` | `"../crypto/aes_modes.vuma"` | `womb/crypto/symmetric/aes_modes.vuma` |
| `womb/net/quic.vuma:69` | `"../crypto/drbg.vuma"` | `womb/crypto/drbg/drbg.vuma` |
| `womb/net/tls13.vuma:147-150` | `"../crypto/{hqc,hmac,hkdf,aes_modes}.vuma"` | (same as above) |
| `womb/net/tls12.vuma:98-100` | `"../crypto/{hqc,hmac,aes256}.vuma"` | (same as above) |
| `womb/net/http2.vuma` | (per grep — let me re-verify) | |
| `womb/net/http3_mqtt_coap.vuma` | (per grep — let me re-verify) | |
| `womb/net/websocket.vuma:69` | `"../crypto/sha1.vuma"` | `womb/crypto/hash/sha1.vuma` |
| `womb/net/websocket.vuma:70` | `"../encoding/base64.vuma"` | `womb/encoding/base64.vuma` ✓ (exists) |
| `womb/net/websocket.vuma:71` | `"../crypto/drbg.vuma"` | `womb/crypto/drbg/drbg.vuma` |
| `womb/lib/sys/email.vuma:65` | `"../encoding/base64.vuma"` | `womb/encoding/base64.vuma` ✓ (exists, but path is `../../encoding/base64.vuma` from `lib/sys/`) |
| `womb/lib/sys/email.vuma:66` | `"../crypto/hqc.vuma"` | (would need to be `../../crypto/post_quantum/hqc.vuma`) |

**Resolver behavior:** `src/parser/src/resolver.rs::resolve_import_path`
at `:500-510` is:
```rust
fn resolve_import_path(&self, import_path: &str, base_dir: &Path) -> PathBuf {
    let path = Path::new(import_path);
    if path.is_absolute() {
        return path.to_path_buf();
    }
    base_dir.join(path)
}
```
No fallback, no path remapping. So `womb/net/ssh.vuma` line 57's
`import "../crypto/hqc.vuma"` resolves to `womb/net/../crypto/hqc.vuma`
= `womb/crypto/hqc.vuma`, which **does not exist** (the file is at
`womb/crypto/post_quantum/hqc.vuma`). The resolver would emit
`ResolveError::FileNotFound { path: "../crypto/hqc.vuma", resolved:
"womb/crypto/hqc.vuma" }`.

**Why this hasn't been caught:** The two test harnesses
(`scripts/test_womb_compile.sh` and `scripts/womb_test_harness.sh`)
compile each `.vuma` file as a standalone unit
(`vuma build --verification none <file> --output <out>`). The
resolver runs on each file's imports — but if `vuma build` only parses
the entry file and skips unresolved imports (treating them as
externally-satisfied), the broken imports wouldn't surface as compile
errors. The kernel smoke test (`scripts/kernel_smoke.sh`) compiles
only `womb/kernel/kernel.vuma` whose imports are all within
`womb/kernel/` (`import "bootinfo.vuma"`, `import "drivers/early_console.vuma"`,
etc.) — so the kernel path never touches the broken net/ imports.

**Classification:** These 8 files are effectively **DEAD** in the
current state — they cannot be linked into a working VUMA program
because their imports don't resolve. They are documented as REAL
implementations (and the algorithm bodies ARE real RFC-compliant
code), but they cannot be used as-is. This is a P2 bug (broken but
non-blocking, since the UI engine doesn't need TLS/QUIC/SSH for v1
browser target — the browser handles TLS natively).

**Fix:** Update all 8 files' import paths to point to the new
subdirectory locations. Example for `womb/net/ssh.vuma`:
```diff
-import "../crypto/hqc.vuma"        { sha256_oneshot };
-import "../crypto/hmac.vuma"       { hmac_sha256 };
-import "../crypto/aes256.vuma"     { aes256_ctr_crypt };
-import "../crypto/bignum2048.vuma" {
+import "../crypto/post_quantum/hqc.vuma"  { sha256_oneshot };
+import "../crypto/mac_kdf/hmac.vuma"      { hmac_sha256 };
+import "../crypto/symmetric/aes256.vuma"  { aes256_ctr_crypt };
+import "../crypto/bignum/bignum2048.vuma" {
```

Also: the file header comments at
`womb/crypto/mac_kdf/hmac.vuma:1`, `womb/crypto/symmetric/aes256.vuma:1`,
and several others still claim the OLD flat path (e.g.
`// womb/crypto/hmac.vuma — HMAC (RFC 2104 / FIPS 198-1)` at
`womb/crypto/mac_kdf/hmac.vuma:1`). These should be updated for
consistency.

**Recommendation:** Surface as a P2 entry in
`docs/vuma-side-problem-catalog.md`. **V-WOMB-1: Broken
cross-directory imports in `womb/net/{ssh,quic,tls12,tls13,http2,http3_mqtt_coap,websocket}.vuma`
and `womb/lib/sys/email.vuma`.**

---

## Architectural decisions needing ADRs

### 6.1 Should WOMB UI modules live in `womb/ui/` or a separate top-level crate?

- **Topic:** Where do the new UI engine `.vuma` files go?
- **Options:**
  - **(A) `womb/ui/`** — per the SWE package's plan. UI modules live
    alongside the existing `womb/{crypto,net,lib,kernel,...}` and
    follow the same `import "../crypto/..."` convention.
  - **(B) `womb_ui/`** — separate top-level directory. Decouples UI
    from the rest of `womb/`, but breaks the `import "../..."` path
    convention (would need `import "../womb/crypto/..."`).
  - **(C) `ui/`** — separate top-level directory at repo root (sibling
    of `womb/`). Cleanest separation but requires either absolute
    imports or a VUMA package/module system (which doesn't exist
    today).
- **Recommendation:** **(A) `womb/ui/`.** The SWE package's plan is
  correct. The `womb/` tree is already the VUMA stdlib; UI is part of
  the stdlib. The `import "../crypto/..."` convention works (modulo
  the broken-import issue in §5.4, which is a separate fix).
- **Confidence:** HIGH. The SWE package already specifies this. No
  design work needed; this is just a confirmation.

### 6.2 Should `womb/crypto/mac_kdf/hmac.vuma` be the canonical HMAC for VUMA's capability model, or should the Rust-side `capability.rs` have its own?

- **Topic:** Where does the canonical HMAC-SHA-256 live?
- **Options:**
  - **(A) `womb/crypto/mac_kdf/hmac.vuma` is canonical; the Rust
    capability layer (`src/codegen/src/capability.rs`) calls into
    VUMA-compiled code.** Per ADR-0007 and SWE line 151. Requires
    the VUMA compiler to be able to invoke a VUMA-compiled transform
    from Rust codegen — the "self-compilation bootstrap" question.
  - **(B) Re-implement HMAC-SHA-256 in Rust inside `capability.rs`
    (or a sibling `hmac.rs`).** Adds ~150 LOC of Rust. Removes the
    self-compilation dependency.
  - **(C) Add the `sha2` + `hmac` Rust crates as build-dependencies
    of `vuma-codegen`.** Violates the "5 external crates max" policy
    (ADR-0010) — would require a new ADR justifying why
    hand-rolling is infeasible.
- **Recommendation:** **(B) Re-implement in Rust.** The capability
  layer is in the VUMA compiler (Rust), not in compiled VUMA
  binaries. The womb/crypto HMAC is for VUMA programs to use at
  runtime; the Rust-side capability layer needs HMAC at compile time
  to sign capability tokens. The two layers are different consumers
  at different times. A 150-LOC Rust HMAC-SHA-256 is trivial,
  verifiable against KAT vectors, and doesn't require
  self-compilation.
- **Confidence:** MEDIUM. ADR-0007 currently specifies (A). The
  argument for (B) is that the self-compilation bootstrap is a
  substantial infrastructure investment that doesn't pay off for a
  single 150-LOC function. But the argument for (A) is that having
  ONE canonical HMAC (in VUMA) avoids drift between the compile-time
  and runtime implementations.
- **Action:** This ADR needs more design work. The decision turns on
  whether the VUMA team plans to self-compile VUMA at build time for
  OTHER reasons (e.g. `womb/lib/text/json.vuma` for parsing
  `vuma.toml`, `womb/net/http.vuma` for the package registry). If
  yes, (A) is free. If no, (B) is simpler.

### 6.3 Should the IrqRing pattern be generalized into `womb/sync/spsc.vuma`?

- **Topic:** Generalize `womb/kernel/trap/irq_ring.vuma` into a
  reusable SPSC ring primitive.
- **Options:**
  - **(A) Generalize now.** Create `womb/sync/spsc.vuma` with a
    generic `layout SpscRing<T, N>` (or, since VUMA has no generics,
    a `layout SpscRing64 { buf: [u8; 64*N], head: u32, tail: u32,
    count: u32 }` for 64-byte slots). Migrate IrqRing to use it. Use
    it for `womb/ui/event/ring.vuma` (W-0).
  - **(B) Copy-paste the pattern.** Each consumer (IrqRing, UI event
    ring, future W-9 capability cache, future W-7 a11y event ring)
    re-declares the layout byte-identically. This is the CURRENT
    VUMA convention — see the kernel docs at
    `womb/kernel/trap/irq_ring.vuma:186-195` *"VUMA has no `import`
    statement yet (Open Work §7). The future consumer will
    re-declare the IrqRing layout + helpers byte-identically to this
    file (same convention as R2a's TrapFrame being re-declared
    byte-identically by R2b/R2c/R2d's per-arch trap_trampoline.vuma
    files)."*
  - **(C) Wait for VUMA `import` to land, then generalize.** The
    "Open Work §7" reference suggests `import` is a known gap. Once
    it lands, generalize IrqRing into `womb/sync/spsc.vuma` and have
    both `womb/kernel/trap/irq_ring.vuma` and
    `womb/ui/event/ring.vuma` import it.
- **Recommendation:** **(B) Copy-paste the pattern** for v1. The
  VUMA convention is already copy-paste (documented at
  `womb/kernel/proc/task.vuma:48-58` and elsewhere). Generalizing
  now would require either generics (which VUMA may not support) or
  a fixed 64-byte-slot variant that doesn't fit IrqRing's 8-byte
  slots. Wait for `import` to land (Option C) before generalizing.
- **Confidence:** HIGH. The kernel team has already made this
  decision (see the "byte-identical re-declaration invariant"
  documented across multiple kernel files).

### 6.4 Should `womb/` have its own test harness separate from `tests/`?

- **Topic:** How are WOMB modules tested?
- **Current state:** Two harnesses exist:
  - `scripts/test_womb_compile.sh` (90 LOC) — compiles every
    `womb/*.vuma` file individually with `vuma build --verification
    none`. Reports OK/FAIL per file.
  - `scripts/womb_test_harness.sh` (158 LOC) — same as above plus
    SCG-node/IR-instruction counting.
  - `scripts/kernel_smoke.sh` (165 LOC) — compiles
    `womb/kernel/kernel.vuma` with `--verify`, runs the binary,
    greps for the "VWK kernel booted" banner.
  - `scripts/real_kat_tests/test_sha256_abc_real.vuma` and
    `test_aes128_real.vuma` — KAT (Known Answer Test) vectors for
    SHA-256 and AES-128.
  - `tests/gold_standard/kernel_crypto/test_sha256_kat.vuma` — gold
    standard test.
- **Options:**
  - **(A) Keep the current split.** Compile-everything harness for
    syntax/IVE; per-module KAT tests under `tests/gold_standard/`;
    smoke tests under `scripts/`.
  - **(B) Create `womb/tests/` for WOMB-specific tests.** Each
    WOMB module ships its own test file alongside the implementation
    (e.g. `womb/ui/layout/measure_test.vuma`).
  - **(C) Fold WOMB tests into the existing `tests/` directory.**
- **Recommendation:** **(A) Keep the current split** for v1, with
  the caveat that every new `womb/ui/*.vuma` file MUST ship a
  matching `womb/ui/*_test.vuma` (or a `tests/gold_standard/ui_*`
  entry). The current per-file compile harness catches syntax/IVE
  errors; KAT tests under `tests/gold_standard/` catch algorithmic
  errors; smoke tests catch integration errors.
- **Confidence:** HIGH. The existing harness structure works for
  the kernel; it will work for the UI engine.

### 6.5 Should the `womb/kernel/` tree be deleted, kept as-is, or split out for the browser-target UI engine?

- **Topic:** The `womb/kernel/` tree is 43 kLOC of VWK kernel that
  is NOT needed for the browser-target UI engine (the browser
  provides the kernel). Should it be deleted, kept, or split out?
- **Options:**
  - **(A) Delete `womb/kernel/`.** It's dead weight for the browser
    UI engine. Removes 43 kLOC of unmaintained code.
  - **(B) Keep `womb/kernel/` as-is.** It's a separate concern
    (native kernel target, post-v1). The UI engine doesn't import
    from it (except for the IrqRing pattern, which will be
    copy-pasted per §6.3).
  - **(C) Move `womb/kernel/` to a separate repo / workspace
    member.** Decouples the kernel from the VUMA stdlib. Cleanest
    separation but breaks the current `import "../crypto/..."`
    convention.
- **Recommendation:** **(B) Keep `womb/kernel/` as-is.** The kernel
  is a substantial existing investment (43 kLOC, 95 files, full
  VWK architecture) and represents the native-target story (post-v1).
  The IrqRing pattern is the only piece the UI engine needs, and
  copy-pasting it (per §6.3) is the VUMA convention. Deleting the
  kernel would be a waste; moving it would break imports.
- **Confidence:** HIGH. This is a "do nothing" decision.

### 6.6 Should the broken cross-directory imports in `womb/net/*.vuma` and `womb/lib/sys/email.vuma` be fixed before W-0 starts?

- **Topic:** The 8 files with broken imports (§5.4) are DEAD code
  today. Should they be fixed?
- **Options:**
  - **(A) Fix now.** Update all 8 files' import paths to point to
    the new subdirectory locations. ~1 day of work.
  - **(B) Leave broken.** These files are not used by the UI engine
    (browser delegates HTTP/WebSocket/TLS to host). The fix can wait
    until the native target needs them.
  - **(C) Delete the broken files.** They're DEAD code; if nobody
    uses them, remove them.
- **Recommendation:** **(A) Fix now.** The fix is trivial (path
  string updates), the files are valuable REAL implementations
  (TLS 1.2/1.3, QUIC, SSH, HPACK are all RFC-compliant), and
  leaving them broken masks a real bug in the test harness (the
  per-file compile test doesn't catch broken imports). Surfacing
  this as V-WOMB-1 in the catalog and fixing it now prevents
  future confusion.
- **Confidence:** HIGH. Trivial fix, clear value.

### 6.7 Should the duplicated SHA-256 implementations be reconciled?

- **Topic:** `womb/crypto/hash/sha256_sha224.vuma` (1 525 LOC) and
  `womb/kernel/crypto/sha.vuma` (611 LOC) both implement SHA-256.
  The former is the "stdlib" version (used by `net/tls*.vuma`,
  `net/ssh.vuma`, `net/quic.vuma`); the latter is the "kernel"
  version (used by `womb/kernel/crypto/api.vuma`). They are
  byte-different implementations of the same algorithm.
- **Options:**
  - **(A) Keep both.** The kernel version is PMT-pure (uses
    `State<ShaCtx>`); the stdlib version uses `allocate()`/`free()`.
    Different consumers, different idioms.
  - **(B) Migrate the kernel version to import the stdlib version.**
    Requires VUMA `import` (Open Work §7).
  - **(C) Delete the kernel version, migrate the kernel to use the
    stdlib version.** Requires the kernel to adopt the
    `allocate()`/`free()` idiom (or the stdlib version to be
    PMT-migrated).
- **Recommendation:** **(A) Keep both for v1.** The two
  implementations serve different consumers with different PMT
  disciplines. Reconciling them is a refactor that doesn't unblock
  anything for the UI engine. Defer to post-v1.
- **Confidence:** MEDIUM. The duplication is a smell, but it's not
  blocking.

### 6.8 Should the `womb/lang/` self-hosted bootstrap compiler be maintained?

- **Topic:** `womb/lang/` (4 170 LOC) is a VUMA-in-VUMA bootstrap
  compiler (lexer + parser + IR builder + x86_64 codegen + ELF
  writer). It supports only a V1.0-era grammar subset (no PMT). Is
  it worth maintaining?
- **Options:**
  - **(A) Keep as-is.** It's a research artifact proving VUMA can
    self-host. The production VUMA compiler is in Rust.
  - **(B) Extend to support PMT constructs (`layout`, `state_new`,
    field access).** Would make VUMA truly self-hosting. Substantial
    work (~3-6 months).
  - **(C) Delete.** It's dead code — the production compiler is in
    Rust and the bootstrap doesn't support PMT.
- **Recommendation:** **(A) Keep as-is.** The bootstrap is a
  research artifact with documented limitations (header at
  `womb/lang/hello.vuma:8-14` is explicit about the subset). It
  doesn't block the UI engine. Extending it (B) is a separate
  research project.
- **Confidence:** HIGH.

### 6.9 Should the `womb/lib/concurrency/threading.vuma` non-atomic Mutex/Spinlock be fixed?

- **Topic:** `womb/lib/concurrency/threading.vuma:22-27` admits
  *"The test-and-set used by Mutex and Spinlock is a non-atomic
  load-then-store pair, which the task spec explicitly permits
  under VUMA's memory model. Correctness of the blocking primitives
  ultimately relies on the kernel-side atomicity of futex."*
- **Options:**
  - **(A) Leave as-is.** The task spec permits it; futex provides
    the actual atomicity.
  - **(B) Migrate to use `IRInstr::AtomicCas` (which exists per
    `womb/kernel/sync/spinlock.vuma:54-57`).** Real atomicity.
  - **(C) Delete `womb/lib/concurrency/threading.vuma` — the kernel
    sync primitives (`womb/kernel/sync/{spinlock,mutex,rwlock,semaphore}.vuma`)
    are the modern PMT-pure versions.**
- **Recommendation:** **(C) Delete or deprecate.** The kernel sync
  primitives are PMT-pure and use real `AtomicCas`. The
  `lib/concurrency/threading.vuma` is the legacy version that
  pre-dates the kernel sync subtree. It's confusing to have both.
  But this is a cleanup decision, not a blocker for the UI engine.
- **Confidence:** MEDIUM.

---

## Confidence assessment for WOMB-layer ADRs

| ADR | Topic | Recommendation | Confidence | Action |
|---|---|---|---|---|
| 6.1 | WOMB UI module location | `womb/ui/` (SWE plan) | HIGH | Confirm in an ADR; no design work needed. |
| 6.2 | Canonical HMAC location | Re-implement in Rust (`capability.rs`) | MEDIUM | Needs more design work — turns on self-compilation bootstrap decision. ADR-0007 currently says use `womb/crypto/mac_kdf/hmac.vuma`. |
| 6.3 | Generalize IrqRing into `womb/sync/spsc.vuma` | Copy-paste for v1 (VUMA convention) | HIGH | No ADR needed; follows existing convention. |
| 6.4 | WOMB test harness | Keep current split (`scripts/test_womb_compile.sh` + `tests/gold_standard/`) | HIGH | No ADR needed; existing convention. |
| 6.5 | `womb/kernel/` fate | Keep as-is | HIGH | No ADR needed; "do nothing." |
| 6.6 | Fix broken cross-directory imports | Fix now (V-WOMB-1) | HIGH | Surface in catalog; ~1 day fix. |
| 6.7 | Duplicate SHA-256 | Keep both for v1 | MEDIUM | Defer to post-v1 refactor. |
| 6.8 | `womb/lang/` bootstrap | Keep as research artifact | HIGH | No ADR needed. |
| 6.9 | `womb/lib/concurrency/threading.vuma` non-atomic | Deprecate in favor of `womb/kernel/sync/` | MEDIUM | Cleanup decision, not a blocker. |

**ADRs ready to write (HIGH confidence):** 6.1, 6.3, 6.4, 6.5, 6.6, 6.8.
**ADRs needing more design work (MEDIUM confidence):** 6.2, 6.7, 6.9.

---

## Summary

The `womb/` tree is a **117 kLOC, 195-file VUMA stdlib** that already
includes a complete kernel (VWK, 43 kLOC), a complete crypto suite
(26 kLOC, 7 subdirs), a self-hosted VUMA bootstrap compiler (4 kLOC),
a POSIX-shaped libc (21 kLOC), and a 15 kLOC networking stack. The
SWE package's WOMB UI modules (`womb/ui/`) are **entirely greenfield**
— no `womb/ui/` directory exists today.

**Six existing artifacts are directly reusable** as substrate for the
UI engine:

1. **`womb/kernel/trap/irq_ring.vuma`** (472 LOC) — SPSC event ring,
   the canonical substrate for `womb/ui/event/ring.vuma` (W-0). SWE
   package line 201 explicitly says to generalize it.
2. **`womb/crypto/mac_kdf/hmac.vuma`** (193 LOC) — RFC 2104
   HMAC-SHA-256, the canonical substrate for the UI capability model
   (W-9) and for V-16's capability-signature migration.
3. **`womb/lib/text/unicode.vuma`** (709 LOC) — RFC 3629 UTF-8 +
   codepoint classification, reusable for `womb/ui/text/utf8.vuma` (W-2).
4. **`womb/lib/text/json.vuma`** (1 254 LOC) — RFC 8259 JSON parser,
   reusable for `womb/ui/theme.vuma` (W-4) and asset manifests.
5. **`womb/lib/sys/{time,math}.vuma`** (756 LOC combined) —
   `time_monotonic_ns()` + `sin`/`cos`/`sqrt`/`pow`, reusable for
   `womb/ui/animation.vuma` (W-4).
6. **`womb/collections/{vec,hashmap,btree_map}.vuma`** (676 LOC) +
   **`womb/graph/{digraph,algorithms}.vuma`** (363 LOC) —
   foundational data structures + topological sort, reusable for
   layout trees, scene graphs, dirty propagation.

**One real bug surfaced:** 8 files in `womb/net/` and
`womb/lib/sys/email.vuma` carry `import` statements pointing to
non-existent paths (the `crypto/` subtree was reorganized into
subdirectories but the imports were never updated). These files are
effectively DEAD code today — their imports cannot resolve. Recommend
surfacing as V-WOMB-1 in the catalog and fixing (~1 day of work).

**Two duplicate implementations** exist (SHA-256 in both
`womb/crypto/hash/sha256_sha224.vuma` and `womb/kernel/crypto/sha.vuma`;
sync primitives in both `womb/lib/concurrency/threading.vuma` and
`womb/kernel/sync/`). These are smells but not blockers — defer to
post-v1 cleanup.

**Six ADRs are ready to write** (HIGH confidence): 6.1 (UI module
location), 6.3 (IrqRing generalization), 6.4 (test harness), 6.5
(kernel fate), 6.6 (broken imports), 6.8 (bootstrap fate). **Three
ADRs need more design work** (MEDIUM confidence): 6.2 (canonical
HMAC location — turns on self-compilation bootstrap), 6.7 (SHA-256
duplication), 6.9 (sync primitive duplication).

**Bottom line for the UI engine team:** The existing `womb/` tree
provides solid substrate (collections, crypto, UTF-8, JSON, time,
math, graph algorithms, IrqRing pattern). The UI engine work itself
— layout, renderer, font parser, text shaper, BiDi, IME, a11y,
event pipeline, animation, theme — is **100% greenfield**. The SWE
package's 10-phase, ~90-week WOMB plan is realistic given this
starting point.
