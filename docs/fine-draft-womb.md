# WOMB Layer — Fine Draft (Final Engineering Plan)

**Status**: Final draft.
**Scope**: Layer 2 (UI engine libraries, written in VUMA, in `womb/`).
**Date**: 2026-08-01.
**Owners**: WOMB UI engine team (writes VUMA); VUMA compiler team (provides the compiler/runtime patches WOMB depends on).
**ADRs locked**: ADR-0013 (three-layer architecture), ADR-0019 (WOMB UI modules in `womb/ui/`; IrqRing generalizes to `womb/sync/`), ADR-0020 (fix `womb/net/*.vuma` imports — V-WOMB-1), ADR-0022 (hand-written SPIR-V, supersedes ADR-0018 MLIR approach).
**Cross-layer drafts**: VUMA-layer (Layer 1) and VEEE-layer (Layer 3) are tracked in separate drafts. This draft covers WOMB only; cross-layer concerns are referenced where they impose constraints.

---

## 1. Executive summary

**WOMB is Layer 2 of the three-layer VUMA architecture (ADR-0013).** It is the UI engine libraries written in VUMA and lives in `womb/`. Layer 1 (VUMA, the compiler) compiles and verifies WOMB; Layer 3 (VEEE, the UX language) compiles to VUMA AST and calls WOMB primitives.

**What exists today** (per the Wave J audit, `docs/research/J-1-womb-layer.md`):
- 195 `.vuma` files across 50 subdirectories, ~117 062 LOC.
- A complete VWK kernel (43 kLOC, 95 files), a complete crypto suite (26 kLOC, 41 files, 7 subdirs), a self-hosted VUMA bootstrap compiler (4 kLOC, 6 files), a POSIX-shaped libc (21 kLOC, 21 files), an RFC-grade networking stack (15 kLOC, 15 files), plus collections/string/graph/encoding/fs/io/env libraries.
- **No `womb/ui/` directory.** Every UI engine module is greenfield.

**What is reusable as substrate** (six artifacts, per J-1 §3):
1. `womb/kernel/trap/irq_ring.vuma` (472 LOC) — the SPSC event ring that WOMB's event pipeline will generalize.
2. `womb/crypto/mac_kdf/hmac.vuma` (193 LOC) — RFC 2104 HMAC-SHA-256 for the UI capability model and the VUMA capability-model bootstrap (ADR-0007).
3. `womb/lib/text/unicode.vuma` (709 LOC) — RFC 3629 UTF-8 + codepoint classification for `womb/ui/text/utf8.vuma`.
4. `womb/lib/text/json.vuma` (1 254 LOC) — RFC 8259 JSON parser for theme files, asset manifests, and IPC.
5. `womb/lib/sys/{time,math}.vuma` (756 LOC combined) — `time_monotonic_ns()` and `sin`/`cos`/`pow`/`sqrt` for animation + layout math.
6. `womb/collections/{vec,hashmap,btree_map}.vuma` (676 LOC) and `womb/graph/{digraph,algorithms}.vuma` (363 LOC) — foundational data structures + topological sort for layout trees, scene graphs, dirty propagation.

**What is greenfield** (per ADR-0019): eight UI sub-modules under `womb/ui/` — `event/`, `layout/`, `render/`, `text/`, `ime/`, `a11y/`, `animation.vuma`, `theme.vuma` — totalling ~50 new `.vuma` files, ~10-15 kLOC. None of these exist today.

**Headline engineering decisions** (this draft):
1. WOMB UI modules live in `womb/ui/` (ADR-0019 Decision 1).
2. The SPSC ring pattern is extracted into `womb/sync/spsc.vuma`; `womb/kernel/trap/irq_ring.vuma` and `womb/ui/event/ring.vuma` both become thin wrappers (ADR-0019 Decision 2).
3. The WOMB renderer uses **pre-compiled hand-written SPIR-V** embedded as VUMA const byte arrays (V-26) — no MLIR, no Cranelift, no runtime shader compilation (ADR-0022).
4. The C host runtime is a thin ~500 LOC shim wrapping OS-provided libraries (SDL2 for window/event loop, Vulkan/Metal system libs for GPU, libibus/libatspi for IME/a11y on Linux, libcurl/libwebsockets for HTTP/WebSocket on platforms where the existing `womb/net/*.vuma` is not used). Total host binary ~7 MB.
5. **V-WOMB-1 is fixed first.** The 8 broken `womb/net/*.vuma` imports (ADR-0020) are a one-day mechanical fix; without it, the network layer is dead code.
6. Every `womb/ui/*.vuma` file ships with a matching `womb/ui/*_test.vuma` (or a `tests/gold_standard/ui_*` entry). The existing `scripts/test_womb_compile.sh` harness grows to compile-import each UI module's transitive closure.
7. Effort per the SWE package's `16-build-vs-buy.md` matrix: ~50 person-weeks pure VUMA (build) + ~17 person-weeks port (from rustybuzz/unicode-bidi) + ~32 person-weeks wrap (host imports) = **~99 person-weeks sequential, ~11 months at 3-person parallelism, ~9 months at 5-person parallelism**.

**Bottom line**: The existing `womb/` tree is solid substrate. The UI engine work itself — layout, renderer, font parser, shaper, BiDi, IME, a11y, event pipeline, animation, theme — is 100% greenfield. The SWE package's ~99-week WOMB plan is realistic given this starting point.

---

## 2. Current WOMB inventory (from J-1 audit)

Source: `docs/research/J-1-womb-layer.md`. Numbers from `find womb/ -name '*.vuma'` and per-file line counts at commit `6dc97e18` (2026-08-01).

### 2.1 Top-level structure (117 kLOC, 195 .vuma files + 18 .S/.ld)

```
womb/
├── syscalls.vuma              252 LOC  (documentation-only — Linux asm-generic table)
├── alloc/arena.vuma           103 LOC  (PMT bump allocator; fixed 256-byte data block)
├── collections/               676 LOC / 4 files (vec, hashmap, btree_map, enum_map)
├── crypto/                  26 179 LOC / 41 files / 7 subdirs
│   ├── asym/                5 803 LOC / 9 files (ed25519, x25519, ecdsa_*, ecdh_p256, secp256k1, rsa*)
│   ├── bignum/              1 197 LOC / 2 files (bignum 256-bit + bignum2048)
│   ├── drbg/                  563 LOC / 2 files (HMAC_DRBG, NIST SP 800-90A)
│   ├── hash/                4 091 LOC / 8 files (sha256/224, sha512, sha384, sha3, blake2, blake3, md5, sha1)
│   ├── mac_kdf/             2 583 LOC / 7 files (hmac, hkdf, pbkdf2, scrypt, argon2, cmac_bcrypt_kdf, key_agreement)
│   ├── post_quantum/        4 238 LOC / 5 files (ml_kem, ml_dsa, slh_dsa, falcon, hqc)
│   └── symmetric/           7 704 LOC / 11 files (aes128/192/256, aes_modes, chacha20, chacha20_poly1305, poly1305, salsa20, des_rc4_aria_camellia)
├── encoding/                   332 LOC / 4 files (hex, base64, url, crc)
├── env/cli.vuma                214 LOC
├── fs/                         253 LOC / 2 files (file, high_level)
├── graph/                      363 LOC / 2 files (digraph, algorithms — topological sort + cycle detection)
├── io/buffered.vuma            125 LOC (BufReader/BufWriter)
├── kernel/                  43 085 LOC / 95 files — VWK (VUMA Wumba Kernel), hosted-mode only
│   ├── arch/               ~6 300 LOC / 22 files (x86_64, aarch64, riscv64, ppc64le, hosted, wasm32)
│   ├── crypto/              2 409 LOC / 5 files (api, aes-SCAFFOLD, sha-REAL, asym-SCAFFOLD, hw_trampoline)
│   ├── drivers/             1 813 LOC / 4 files (early_console, uart, char, virtio_net)
│   ├── fs/                  4 645 LOC / 4 files (devfs, initramfs, procfs, tmpfs)
│   ├── hosted/host.vuma       156 LOC
│   ├── ipc/                  4 847 LOC / 5 files (pipe, signal, futex, shm, waitq)
│   ├── mm/                   2 771 LOC / 4 files (pmm buddy, vmm, kmalloc, mmap)
│   ├── net/                  2 515 LOC / 5 files (sk_buff, http, socket, tcp, dns) — kernel-side, mostly STUB
│   ├── panic/                  614 LOC / 2 files (panic, kmsg)
│   ├── power/pm.vuma           354 LOC
│   ├── proc/                 5 368 LOC / 7 files (task, scheduler, fork, exec, wait, exit, elf)
│   ├── shell/shell.vuma     2 134 LOC (interactive shell)
│   ├── smp/                  1 747 LOC / 3 files (percpu, ipi, smp)
│   ├── sync/                 3 139 LOC / 4 files (spinlock, mutex, semaphore, rwlock) — PMT-pure, AtomicCas
│   ├── syscall/              1 606 LOC / 4 files (abi, table, dispatch, syscall_init) + handlers/{io,fs,proc,mm}
│   ├── trap/                 2 117 LOC / 4 files (trap, trap_frame, irq, irq_ring — §4.1 substrate)
│   ├── tty/                  2 326 LOC / 3 files (vt100, console, line_discipline)
│   └── vfs/                  4 524 LOC / 7 files (ops, file, inode, dentry, file_ops, namei, mount)
├── lang/                     4 170 LOC / 6 files — self-hosted VUMA bootstrap (research artifact, no PMT)
├── lib/                     21 509 LOC / 21 files — POSIX-shaped stdlib
│   ├── mem_helpers.vuma        60 LOC (single source of truth for store_u*/load_u*)
│   ├── compress/             2 531 LOC / 2 files (deflate+zlib REAL, gzip/lz4/zstd/brotli simplified)
│   ├── concurrency/            882 LOC / 2 files (epoll event_loop, threading — legacy non-atomic)
│   ├── pki/                  6 231 LOC / 5 files (asn1, x509, pki, auth, jwt)
│   ├── sys/                  3 633 LOC / 7 files (stdlib, math, stdio, fileio, time, fp, email)
│   └── text/                 4 002 LOC / 4 files (string, unicode, json, printf)
├── net/                     14 884 LOC / 15 files — RFC-grade protocol stack (BUT see V-WOMB-1)
│   ├── socket.vuma, tcp.vuma, dns.vuma, dns_extra.vuma
│   ├── http.vuma (944), http2.vuma (696), http3_mqtt_coap.vuma (1 076)
│   ├── websocket.vuma (506), hpack.vuma (1 787)
│   ├── tls12.vuma (985), tls13.vuma (1 277), quic.vuma (1 893), ssh.vuma (1 045)
│   └── ip_icmp_arp.vuma (1 665), ieee_frames.vuma (1 505)
└── string/                    416 LOC / 3 files (string, string_builder, utf8)
```

### 2.2 Maturity profile

| Subsystem | Maturity | Notes |
|---|---|---|
| `crypto/` | REAL | Production-grade RFC-compliant code. Post-quantum modules are research-grade. |
| `encoding/`, `string/`, `collections/`, `graph/` | REAL | Heap-backed, some still use `allocate()`/`free()` (legacy) rather than `state_new(T)` (PMT). |
| `lib/text/{string,unicode,json,printf}.vuma`, `lib/sys/{math,time}.vuma` | REAL | Foundational — directly reusable as WOMB substrate. |
| `lib/compress/deflate.vuma` | REAL | RFC 1951 + zlib + CRC-32. |
| `lib/compress/compression_extra.vuma` | SCAFFOLD | gzip/LZ4/zstd/brotli wrappers — simplified (stored blocks only). |
| `lib/pki/` | REAL | asn1, x509, pki, auth, jwt. Not needed for browser target; relevant for native. |
| `lib/concurrency/threading.vuma` | LEGACY | Non-atomic Mutex/Spinlock; `womb/kernel/sync/` is the PMT-pure modern replacement (J-1 §6.9 — deprecate). |
| `net/{http,http2,tls12,tls13,quic,ssh,websocket,hpack}.vuma` | REAL **but DEAD** | V-WOMB-1: imports point to pre-reorg `crypto/` paths. Algorithms are RFC-compliant but files cannot be linked until imports are fixed. |
| `kernel/crypto/{aes,asym}.vuma` | SCAFFOLD | Stub key schedule, mod-256 ed25519 — NOT real Ed25519. Redundant with the REAL `crypto/symmetric/aes256.vuma` and `crypto/asym/ed25519.vuma`. |
| `kernel/crypto/sha.vuma` | REAL | Real SHA-256, but redundant with `crypto/hash/sha256_sha224.vuma` (J-1 §6.7 — defer reconciliation to v2). |
| `kernel/net/{http,socket,tcp,dns}.vuma` | STUB | Header comments admit K9f/K9 stubs returning 0. |
| `lang/` | REAL (limited subset) | VUMA-in-VUMA bootstrap; V1.0 grammar only, no PMT. Research artifact (J-1 §6.8 — keep). |

### 2.3 Six directly-reusable artifacts (J-1 §3, ADR-0019)

| Artifact | LOC | Role in WOMB |
|---|---|---|
| `womb/kernel/trap/irq_ring.vuma` | 472 | SPSC event ring — generalize to `womb/sync/spsc.vuma`; `womb/ui/event/ring.vuma` wraps it (W-0). |
| `womb/crypto/mac_kdf/hmac.vuma` | 193 | RFC 2104 HMAC-SHA-256 — capability-token signing (W-9) and VUMA capability-model bootstrap (ADR-0007). |
| `womb/lib/text/unicode.vuma` | 709 | RFC 3629 UTF-8 — re-exported or migrated into `womb/ui/text/utf8.vuma` (W-3/W-4). |
| `womb/lib/text/json.vuma` | 1 254 | RFC 8259 JSON — theme files, asset manifests, host-shim IPC (W-4 theme, W-0 dispatch). |
| `womb/lib/sys/{time,math}.vuma` | 756 | `time_monotonic_ns()`, `sin`/`cos`/`pow`/`sqrt` — animation (W-7), layout math. |
| `womb/collections/{vec,hashmap,btree_map}.vuma` + `womb/graph/{digraph,algorithms}.vuma` | 1 039 | Dynamic arrays, hash maps, BTreeMap, directed graph + topological sort — layout trees, scene graphs, dirty propagation, ARIA node registries. |

### 2.4 WOMB-side bugs (catalog entries)

| ID | Severity | Status | Effort | ADR |
|---|---|---|---|---|
| V-WOMB-1 | P1 (blocks WOMB net layer) | Open (verified by Wave J) | 1 day | ADR-0020 |
| V-A2-4 | P3 (dead-code backend arms for `IRInstr::Channel*`/`StarkProof`/etc.) | Open | 3 weeks | (deferred) |

V-WOMB-1 (§6.1) is the only P1 WOMB-side bug. V-A2-4 (§6.2) is a P3 cleanup: the `=> {}` arms in 14+ backends are dead code because `ipc_lowering.rs` lowers Call-form builtins (`channel_send`, `channel_recv`, `stark_prove`, etc.) before the backend ever sees them. These builtins are what WOMB's `womb/ui/event/`, `womb/ui/ime/`, and `womb/ui/capability.vuma` (W-9) build on. P3 cleanup — not blocking, but worth doing once W-0 lands so the backend dead-code doesn't mask future regressions.

---

## 3. WOMB UI module plan (from ADR-0019)

### 3.1 Directory layout (ADR-0019 Decision 1)

```
womb/ui/
├── event/
│   ├── ring.vuma          # SPSC UiEvent ring — wraps womb/sync/spsc.vuma
│   ├── dispatch.vuma      # Event dispatch (state ring first, stream ring second)
│   ├── normalize.vuma     # UiEvent layout normalization + payload decode
│   └── yield.vuma         # host_yield() extern (browser/native/hosted contracts)
├── layout/
│   ├── flex.vuma          # Flexbox — measure, position, distribute (f32 coordinates)
│   ├── stacking.vuma      # Stacking contexts, z-index DAG (uses womb/graph/algorithms)
│   ├── position.vuma      # position: absolute/fixed; containing-block walk
│   ├── scroll.vuma        # Scroll containers (composited; momentum + bounce)
│   ├── dirty.vuma         # Dirty flag propagation (topological sort)
│   ├── vertical.vuma      # writing-mode: vertical-rl / vertical-lr (depends on W-3 vmtx)
│   ├── linebreak_knuth_plass.vuma  # Optimal line breaking (depends on W-3 BiDi/UAX #14)
│   └── node.vuma          # LayoutNode layout + measure/position entry points
├── render/
│   ├── path.vuma          # PathSegment, Path, Transform, Rect data structures
│   ├── scene.vuma         # SceneNode tree (parent-pointer + sibling-list, not adjacency-list)
│   ├── scene_build.vuma   # LayoutNode tree → SceneNode tree (depends on W-1 layout)
│   ├── outline_to_path.vuma  # glyf/CFF outline → PathSegment[] (depends on W-3 font parser)
│   ├── gpu_dispatch.vuma  # Host import: Vulkan/Metal/WebGPU (extern "C" transforms)
│   ├── gpu_encode.vuma    # Flatten scene → path buffer; allocate GPU ring; dispatch compute
│   ├── clip.vuma          # Clip paths (arbitrary shapes, not just scissor rects)
│   ├── blend.vuma         # Per-node blend modes + opacity (uniform-driven in compute shader)
│   ├── color_glyph.vuma   # COLR/CPAL color-font path builder (depends on W-3 COLR)
│   ├── scroll_layer.vuma  # Composited scroll: translate path coordinates, no texture
│   ├── frame.vuma         # Frame pacing (vsync, time_monotonic_ns)
│   └── shaders_spirv.vuma # Generated const [u8; N] arrays (V-26) for the path-tessellation compute shader
├── text/
│   ├── font_parse.vuma    # OpenType table directory + head/hhea/maxp/OS_2/name
│   ├── cmap.vuma          # cmap formats 0/4/6/12/13/14 (incl. variation sequences)
│   ├── hmtx.vuma, vmtx.vuma  # Horizontal + vertical metrics (V-31)
│   ├── glyf.vuma          # TrueType outlines + loca index
│   ├── fvar.vuma, gvar.vuma  # Variable-font axes + per-glyph deltas (V-17)
│   ├── colr.vuma, cpal.vuma  # Color-font layers + palettes (V-18)
│   ├── shaper_v1.vuma     # cmap + hmtx naive shaper (60% of languages; Latin/Cyrillic/Greek/CJK)
│   ├── shaper_v2.vuma     # GSUB + GPOS (port from rustybuzz — 95% of languages)
│   ├── bidi.vuma          # UAX #9 BiDi (port from unicode-bidi)
│   ├── linebreak.vuma     # UAX #14 line breaking
│   ├── grapheme.vuma, wordbreak.vuma  # UAX #29 grapheme + word segmentation
│   ├── hint.vuma          # TrueType hinting (simplified, pixel-snap only)
│   ├── subset.vuma        # Font subsetting (BMP for v1, supplementary for v2)
│   ├── fontstack.vuma     # Font fallback chain (8-font stack)
│   └── utf8.vuma          # Re-export of womb/lib/text/unicode.vuma (or migration target)
├── ime/
│   ├── composition.vuma   # IME state machine (ImeState layout + composition lifecycle)
│   ├── bridge.vuma        # OS IME host imports (extern "C"; IBus/IMM32/IMK)
│   ├── caret.vuma         # Caret position (f32, depends on W-2 + W-1)
│   ├── format.vuma        # Composition formatting (underline ranges; depends on W-5 renderer)
│   └── textfield.vuma     # Text-field management (insert/replace/select)
├── a11y/
│   ├── semantics.vuma     # SemanticsNode tree (f32 bounds)
│   ├── build.vuma         # LayoutNode tree → SemanticsNode tree (depends on W-1)
│   ├── diff.vuma          # Tree diff (state/value/bounds/children changes)
│   ├── bridge.vuma        # OS a11y host imports (extern "C"; AT-SPI/UIA/NSA11y)
│   ├── reverse.vuma       # Reverse bridge: a11y actions → UI callbacks
│   └── preferences.vuma   # High-contrast + reduced-motion preference polling
├── animation.vuma         # Easing curves (linear, ease-in/out, cubic-bezier), keyframes
├── theme.vuma             # Theme manager + auto-switch (light/dark/high-contrast)
├── clipboard.vuma         # Clipboard host imports + capability gating
├── filepick.vuma          # File picker host imports + async result events
├── net/
│   ├── http_bridge.vuma   # HTTP — browser target: fetch() wrapper; native target: re-export womb/net/http.vuma
│   └── ws_bridge.vuma     # WebSocket — browser target: net_websocket_* wrapper; native: re-export womb/net/websocket.vuma
└── capability.vuma        # UI capability tokens (HMAC-SHA-256 signing via womb/crypto/mac_kdf/hmac.vuma)
```

### 3.2 Module → SWE RFC → build-vs-buy → effort → VUMA-side deps

The table below is the master cross-reference. "Build" = pure VUMA; "Port" = translate algorithm from a Rust/C reference; "Wrap" = `extern "C"` host import to an OS-provided library. Effort estimates are person-weeks from the SWE package's `16-build-vs-buy.md` matrix.

| WOMB module | W-ID (SWE) | RFC | Decision | Effort | VUMA-side deps |
|---|---|---|---|---|---|
| `womb/sync/spsc.vuma` (new) | W-0 substrate | RFC-03 | Build (VUMA) — ADR-0019 Decision 2 | 1 week | V-03 (IVE soundness for nested layouts) |
| `womb/ui/event/ring.vuma` | W-0 | RFC-03 | Build (wraps spsc.vuma) | 3 days | V-34 (f32 state fields — DONE) |
| `womb/ui/event/dispatch.vuma` | W-0 | RFC-03 | Build (VUMA) | 2 weeks | V-34 (DONE) |
| `womb/ui/event/normalize.vuma` | W-0 | RFC-03 | Build (VUMA) | 1 week | V-34 (DONE) |
| `womb/ui/event/yield.vuma` | W-0 | RFC-03 | Wrap (host_yield) | 3 days | (none — extern already works) |
| `womb/ui/layout/*` (flex, stacking, position, scroll, dirty, vertical, linebreak_knuth_plass, node) | W-1 | RFC-04 | Build (VUMA) | **12 weeks total** | V-34 (DONE), V-46 (resolve_state_array_access for `[LayoutNode; N]` — deferred), V-03 (IVE soundness for nested layouts) |
| `womb/ui/render/path.vuma`, `scene.vuma`, `scene_build.vuma`, `outline_to_path.vuma`, `gpu_encode.vuma`, `clip.vuma`, `blend.vuma`, `color_glyph.vuma`, `scroll_layer.vuma`, `frame.vuma` | W-2 (VUMA side) | RFC-21 | Build (VUMA) | **3 weeks total** | V-26 (const byte arrays for SPIR-V embedding — deferred), V-33 (Path IR instructions — deferred) |
| `womb/ui/render/gpu_dispatch.vuma` | W-2 (host import) | RFC-21, RFC-02 | Wrap (Vulkan/Metal/WebGPU) | 4 weeks (Vulkan) + 3 weeks (Metal) | (none — extern already works) |
| `womb/ui/render/shaders_spirv.vuma` (generated) | W-2 (shader build) | RFC-21, ADR-0022 | Build (Python script invoking glslangValidator at build time) | **2 weeks** | V-26 (deferred — needed to embed the generated bytes as `[u8; N]`) |
| Path-tessellation compute shader (~500 LOC GLSL → .spv) | W-2 (shader source) | RFC-21 | Build (VUMA + SPIR-V) | **6 weeks** | (none — pure GLSL, build-time only) |
| `womb/ui/text/font_parse.vuma`, `cmap.vuma`, `hmtx.vuma`, `vmtx.vuma`, `glyf.vuma`, `fvar.vuma`, `gvar.vuma`, `colr.vuma`, `cpal.vuma`, `fontstack.vuma` | W-3 (font parser) | RFC-06 | Build (VUMA) — study `ttf-parser` | **6 weeks total** | V-34 (DONE), V-26 (deferred — for font subsetting byte arrays), V-A2-3 (SIMD vectorizer fix — deferred, accelerates cmap lookups) |
| `womb/ui/text/shaper_v1.vuma` | W-3 (shaper v1) | RFC-07 | Build (VUMA) | **2 weeks** | V-34 (DONE), V-A2-3 (deferred) |
| `womb/ui/text/shaper_v2.vuma` (GSUB + GPOS) | W-3 (shaper v2/v3) | RFC-08 | Port (from rustybuzz) | **10 weeks** | V-34 (DONE), V-A2-3 (deferred — SIMD accelerates GSUB/GPOS lookups) |
| `womb/ui/text/bidi.vuma` (UAX #9) | W-3 (BiDi) | RFC-09 | Port (from unicode-bidi) | **5 weeks** | V-26 (deferred — for BiDi property table embedding) |
| `womb/ui/text/{linebreak, grapheme, wordbreak}.vuma` (UAX #14/#29) | W-3 | RFC-09 | Build (VUMA, table-driven) | **3 weeks** (linebreak) + **2 weeks** (grapheme + wordbreak) | V-26 (deferred — for property tables) |
| `womb/ui/text/hint.vuma` (TrueType hinting) | W-3 | RFC-06 | Build (VUMA, simplified — pixel-snap only) | **2 weeks** | V-34 (DONE) |
| `womb/ui/text/subset.vuma` | W-3 | RFC-06 | Build (VUMA) | **2 weeks** | V-26 (deferred — for the subset's `[u8; N]` output) |
| `womb/ui/ime/composition.vuma` | W-5 | RFC-10 | Build (VUMA) | **1 week** | V-11 (session types Choice/Offer for IME channel — deferred) |
| `womb/ui/ime/bridge.vuma` (Linux IBus) | W-5 | RFC-10 | Wrap (libibus) | **2 weeks** | (none — extern) |
| `womb/ui/ime/bridge.vuma` (Windows IMM32) | W-5 | RFC-10 | Wrap (imm32.dll) | **2 weeks** | (none — extern) |
| `womb/ui/ime/bridge.vuma` (macOS IMK) | W-5 | RFC-10 | Wrap (InputMethodKit) | **2 weeks** | (none — extern) |
| `womb/ui/ime/{caret, format, textfield}.vuma` | W-5 | RFC-10 | Build (VUMA) | **3 weeks** | V-34 (DONE) |
| `womb/ui/a11y/semantics.vuma` + `build.vuma` + `diff.vuma` + `reverse.vuma` + `preferences.vuma` | W-6 | RFC-11 | Build (VUMA) | **4 weeks** | V-34 (DONE), V-03 (IVE soundness for nested SemanticsNode trees) |
| `womb/ui/a11y/bridge.vuma` (Linux AT-SPI) | W-6 | RFC-11 | Wrap (libatspi) | **2 weeks** | (none — extern) |
| `womb/ui/a11y/bridge.vuma` (Windows UIA) | W-6 | RFC-11 | Wrap (uiautomationcore.dll) | **2 weeks** | (none — extern) |
| `womb/ui/a11y/bridge.vuma` (macOS NSAccessibility) | W-6 | RFC-11 | Wrap (AppKit) | **2 weeks** | (none — extern) |
| `womb/ui/animation.vuma` | W-7 | RFC-04 §6 (built-in animation) | Build (VUMA) | **2 weeks** | V-34 (DONE) |
| `womb/ui/theme.vuma` | W-7 | RFC-11 §multi-theme | Build (VUMA + C host) | **2 weeks** | V-34 (DONE) |
| `womb/ui/clipboard.vuma` | W-7 | RFC-12 | Wrap (OS APIs) | **9 days** | (none — extern) |
| `womb/ui/filepick.vuma` | W-7 | RFC-12 | Wrap (OS dialogs) | **9 days** | (none — extern) |
| `womb/ui/net/http_bridge.vuma` | W-8 | RFC-12 | Wrap (libcurl on native; fetch() on browser) | **1 week** | V-WOMB-1 (DONE — fix unblocks `womb/net/http.vuma` reuse on native) |
| `womb/ui/net/ws_bridge.vuma` | W-8 | RFC-12 | Wrap (libwebsockets on native; browser WS API) | **1 week** | V-WOMB-1 (DONE — unblocks `womb/net/websocket.vuma` reuse on native) |
| `womb/ui/capability.vuma` (UI capability tokens) | W-9 | RFC-13 | Build (VUMA — calls `womb/crypto/mac_kdf/hmac.vuma`) | **2 weeks** | V-16 (HMAC-SHA-256 capability signature — ADR-0007), V-09 (capability-model IR plumbing) |
| `womb/ui/cap_bundles.vuma` + `cap_delegate.vuma` + `cap_cache.vuma` | W-9 | RFC-13 | Build (VUMA) | **4 weeks** | V-16, C-06 (per-frame verification cache) |

**Effort totals** (per `16-build-vs-buy.md`):
- Pure VUMA (build): ~50 person-weeks (~13 months)
- Port (from rustybuzz/unicode-bidi): ~17 person-weeks (~4 months)
- Wrap (host imports): ~32 person-weeks (~8 months)
- **Sequential total**: ~99 person-weeks (~25 months)
- **With 3-person parallelism**: ~11 months
- **With 5-person parallelism**: ~9 months

The vector engine (RFC-21) reduced total effort by ~8 person-weeks vs. the original pixel-engine (RFC-05) proposal: deleted glyph atlas + rasterizer (3w), simplified hinting (3w), simplified scroll/color-font/HiDPI (5w), added path tessellation shader (6w). Net −5w; plus the simplified hinting (−3w) = −8w total.

---

## 4. Module-by-module fine draft

### 4.1 `womb/ui/event/` (W-0, RFC-03)

**RFC**: `vuma-swe-package/03-rfc-event-pipeline.md`.
**Build-vs-buy**: Build (VUMA) for ring + dispatch + normalize; Wrap for `host_yield()`.
**Effort**: 1 week (spsc generalization + ring wrapper) + 2 weeks (dispatch) + 1 week (normalize) + 3 days (yield) = **~4 weeks**.
**VUMA-side deps**: V-34 (f32 state fields — DONE).

#### 4.1.1 `womb/sync/spsc.vuma` (new — ADR-0019 Decision 2)

The existing `womb/kernel/trap/irq_ring.vuma` (472 LOC, J-1 §4.1) is a working SPSC ring buffer for 8-byte IRQ vectors. Per ADR-0019 Decision 2, the pattern is extracted into `womb/sync/spsc.vuma` parameterized by slot size and slot count (VUMA has no generics, so the layout is byte-stitched with a const-element-size field).

```vuma
// womb/sync/spsc.vuma — generalized SPSC ring
// Slot size and slot count are compile-time constants per-instantiation.
// Producers write the slot bytes BEFORE publishing count; consumers read slot
// bytes BEFORE decrementing count. Acquire/release ordering on `count` provides
// the SPSC invariant across the JS/Wasm boundary (browser) and across cores
// (native, via __atomic_load_n / __atomic_store_n host imports).
layout SpscRing = {
    buf: [u8; 16384],   // 256 × 64-byte slots (UiEvent) — parameterized at instantiation
    head: u32,          // read position (wraps via & (count - 1))
    tail: u32,          // write position (wraps via & (count - 1))
    count: u32,         // entries currently in buffer
    slot_size: u32,     // bytes per slot (8 for IrqRing, 64 for UiEvent)
    slot_count: u32,    // power of 2 (32 for IrqRing, 256 for UiEvent stream ring)
}
```

`womb/kernel/trap/irq_ring.vuma` becomes a thin wrapper that instantiates `SpscRing` with `slot_size = 8, slot_count = 32`. `womb/ui/event/ring.vuma` becomes a thin wrapper with `slot_size = 64, slot_count = 256` (stream ring) and `slot_count = 32` (state ring).

**Why generalize now (vs. J-1 §6.3 "copy-paste for v1")**: ADR-0019 Decision 2 overrides J-1's recommendation. The generalization is ~100 LOC; the copy-paste alternative would duplicate 472 LOC across kernel + UI + future W-9 capability cache + future W-6 a11y event ring. A single tested implementation is better.

#### 4.1.2 Two-ring design (RFC-03 §two-ring-design)

The WOMB event pipeline uses **two rings** (per RFC-03):

- **State ring** (32 slots × 64 bytes = 2 KB): `pointerdown`, `pointerup`, `keydown`, `keyup`, `focus`, `blur`, `resize`, `move`, `close`, `visibility`, `dpr_change`, `theme_change`, `ime_compositionstart`, `ime_compositionend`, `ime_textupdate`, `ime_textformatupdate`, `file_picker_result`, `fetch_result`, `websocket_message`, `a11y_action`, `clipboard_paste`. **Never coalesced.** On full: overwrite oldest (state events are urgent — a lost `pointerdown` breaks the UI).
- **Stream ring** (256 slots × 64 bytes = 16 KB): `pointermove`, `wheel`. **Coalesced per `pointer_id`** — before pushing a new `pointermove`, scan for an existing one with the same `pointer_id`; if found, overwrite in place. On full: drop new event (the latest position is already in the ring).

Both rings are `SpscRing` instantiations. The state ring's `push` overrides the full-slot policy to overwrite-oldest; the stream ring's `push` adds the per-`pointer_id` scan.

#### 4.1.3 `UiEvent` layout (64 bytes, f32 coordinates)

Per RFC-03 §UiEvent-layout. All coordinates are f32 (subpixel). Defined in `womb/ui/event/normalize.vuma`:

```vuma
layout UiEvent = {
    // Common header (16 bytes)
    kind: u32,
    timestamp: u32,
    modifiers: u32,
    payload_kind: u32,        // 0=inline, 1=side_buffer

    // Inline payload (40 bytes)
    inline_payload: [u8; 40],

    // Side buffer reference (8 bytes)
    side_offset: u32,
    side_length: u32,
}
// Total: 64 bytes per slot
```

`payload_kind = 1` is used for variable-length payloads (e.g. IME `textupdate` with > 36 bytes of text). The side buffer is a 256 KB ring in the C host runtime; VUMA reads via `host_side_buffer_read(offset, dst, len)`.

Event kind taxonomy: state ring kinds 1-30 (pointer, keyboard, focus, window, IME, async results); stream ring kinds 100-110 (`pointermove`, `wheel`). Inline payload layouts per RFC-03 §inline-payload-formats.

#### 4.1.4 Dispatch (`womb/ui/event/dispatch.vuma`)

```vuma
transform ui_drain_events(engine: State<UiEngine>) {
    let event = state_new(UiEvent);

    // Drain state ring first (urgent)
    while ring_pop_state(engine.state_ring, event) == 1 {
        ui_dispatch_event(engine, event);
    }

    // Then drain stream ring (coalesced)
    while ring_pop_stream(engine.stream_ring, event) == 1 {
        ui_dispatch_event(engine, event);
    }
}

transform ui_dispatch_event(engine: State<UiEngine>, event: State<UiEvent>) {
    let kind = event.kind;
    if kind == EVENT_POINTER_DOWN { ui_on_pointer_down(engine, event); }
    else if kind == EVENT_POINTER_UP { ui_on_pointer_up(engine, event); }
    // ... etc through all 30 state kinds + 2 stream kinds ...
    else if kind == EVENT_POINTER_MOVE { ui_on_pointer_move(engine, event); }
    else if kind == EVENT_WHEEL { ui_on_wheel(engine, event); }
}
```

Hit-testing (`ui_hit_test` in RFC-03 §hit-testing) walks the layout tree depth-first with f32 comparison, respecting z_index (stacking contexts). Depends on W-1 layout.

#### 4.1.5 `host_yield()` (`womb/ui/event/yield.vuma`)

Adapts `womb/kernel/arch/wasm32/sched_hal.vuma:25-41` (J-1 §4.9). Browser target: `host_yield()` is `setTimeout(0)`; native target: no-op (VUMA binary owns its thread, host event loop runs separately); hosted-x86_64: `__ffi_fallback_stub` returns 0.

```vuma
extern "C" {
    transform host_yield();
}

transform ui_main_loop() -> i32 {
    let engine = state_new(UiEngine);
    ui_init(engine);
    while ui_should_run(engine) == 1 {
        ui_drain_events(engine);
        ui_update_layout(engine);
        ui_render(engine);
        ui_dispatch_cmds(engine);
        host_yield();
    }
    return 0;
}
```

#### 4.1.6 Test plan (per RFC-03 §test-plan)

1. State ring overflow: push 100 state events without draining — ring holds 32, overwrites oldest.
2. Stream ring coalescing: push 100 `pointermove` events rapidly — consumer sees ≤ 32 (one per slot, latest per `pointer_id`).
3. Memory ordering: producer on core 0, consumer on core 1 — no torn reads.
4. Side buffer: push IME `textupdate` with 200 bytes — uses side buffer, consumer reads full text.
5. Two-ring priority: push 10 stream events + 1 state event — state event drained first.
6. f32 precision: push `pointermove` at (100.5, 200.25) — consumer reads exact f32 values.

Each test lives at `womb/ui/event/*_test.vuma` and is registered in `scripts/test_womb_compile.sh` (extended to follow imports — see §8 step 1).

---

### 4.2 `womb/ui/layout/` (W-1, RFC-04)

**RFC**: `vuma-swe-package/04-rfc-layout-engine.md`.
**Build-vs-buy**: Build (VUMA) — study CSS Flexbox spec; Taffy/Yoga as reference.
**Effort**: **12 weeks** (per `16-build-vs-buy.md`).
**VUMA-side deps**: V-34 (f32 state fields — DONE), V-46 (`resolve_state_array_access` for `[LayoutNode; N]` — deferred), V-03 (IVE soundness for nested layouts).

#### 4.2.1 f32 coordinates (L-05, decided)

All layout coordinates are **f32** (not i32). Per RFC-04 §f32-coordinates:
- Subpixel text positioning (12.5px font sizes, 1.5× DPR displays).
- Smooth scrolling (scroll offset is f32).
- Smooth animation (interpolated values are f32).
- CSS uses f32 throughout — diverging makes porting harder.

VUMA-side support: V-34 adds `"f32" => IRType::F32, "f64" => IRType::F64` arms to `bridge_type_to_ir_type` (`src/pipeline.rs:6506-6516`). Without this, `node.measured_w + 1.0` would emit integer `ADD` instead of `ADDSD`. V-34 is DONE (3-day fix, ADR-0001).

The f32 PMT Lean proof (V-14, ADR-0006) is **deferred to v2**. v1 uses runtime `__float_overflow_trap` (exit 142) only — no formal verification of f32 arithmetic. Documented as a known gap.

#### 4.2.2 Two-pass layout

1. **Measure pass** (bottom-up): compute `measured_w` / `measured_h` based on content + constraints.
2. **Layout pass** (top-down): given final position + size, position children.

```vuma
transform layout_measure(node_offset: u32, available_w: f32, available_h: f32) -> Size {
    if node_offset == 0 { /* return zero Size */ }
    let node = state_at_offset::<LayoutNode>(node_offset);
    if node.dirty == 0 && node.measured_w > 0.0 { /* return cached */ }
    if node.node_kind == 1 { return measure_text_node(node, available_w); }   // calls shaper
    else if node.node_kind == 2 { return measure_container(node, available_w, available_h); }
    // ...
}

transform layout_position(node_offset: u32, x: f32, y: f32, width: f32, height: f32) {
    // Position children according to flex_direction, justify_content, align_items
}
```

#### 4.2.3 Flex grow/shrink distribution (`womb/ui/layout/flex.vuma`)

Per RFC-04 §flex-grow-shrink-distribution. Sums `flex_grow` over children, distributes free space proportionally. Uses f32 throughout.

#### 4.2.4 Stacking contexts (`womb/ui/layout/stacking.vuma`, V-21 / L-07)

CSS-compliant z-ordering. Each `z_index != 0` creates a stacking context. Render order:
1. Background (negative z_index, sorted).
2. Non-positioned siblings (z_index == 0, document order).
3. Positioned siblings (z_index > 0, sorted).
4. Child stacking contexts (recursively).

The stacking-context DAG uses `womb/graph/digraph.vuma` (188 LOC, J-1 §3.5) + `womb/graph/algorithms.vuma` (topological sort, 177 LOC) — directly applicable per J-1 §4.7.

#### 4.2.5 Position absolute/fixed (`womb/ui/layout/position.vuma`, V-22 / L-09)

Per RFC-04 §position-absolute-fixed. Walks up the tree to find the nearest positioned ancestor (containing block). If none, uses the viewport. `position: fixed` always uses the viewport (the containing-block walk terminates at the root).

#### 4.2.6 Vertical text (`womb/ui/layout/vertical.vuma`, V-23 / L-10)

`writing-mode: vertical-rl` / `vertical-lr`. Shaper uses `vmtx` table (vertical advances) instead of `hmtx`. Glyphs stacked top-to-bottom; lines advance right-to-left (vertical-rl) or left-to-right (vertical-lr). Depends on W-3 `vmtx.vuma` (RFC-06 §vmtx-lookup).

#### 4.2.7 Knuth-Plass line breaking (`womb/ui/layout/linebreak_knuth_plass.vuma`, V-30 / L-11)

Per RFC-04 §knuth-plass. ~2 kLOC. O(n²) worst case, typically O(n) for reasonable paragraph lengths (most opportunities pruned early). Minimizes sum of squared inter-word spaces for visually balanced paragraphs. Depends on W-3 `linebreak.vuma` (UAX #14) for break-opportunity enumeration.

#### 4.2.8 Composited scroll with momentum (`womb/ui/layout/scroll.vuma`, V-25 / L-06)

Per RFC-04 §composited-scroll. Velocity tracked via exponential moving average; friction (0.95 per-frame multiplier); dampened bounce at edges (iOS/macOS convention). The renderer (W-2) draws the scroll layer's children at `(-scroll_x, -scroll_y)` offset, clipped to the viewport — **no GPU texture** (vector renderer translates path coordinates in-place).

#### 4.2.9 Dirty tracking (`womb/ui/layout/dirty.vuma`, L-03)

Per RFC-04 §dirty-tracking. `layout_mark_dirty(node)` sets `dirty = 1` and propagates up the parent chain. Next measure pass re-measures dirty nodes, skips clean ones. Uses `womb/graph/algorithms.vuma` (topological sort) for the dirty-propagation order.

#### 4.2.10 `LayoutNode` layout (V-08)

```vuma
layout LayoutNode = {
    parent_offset: u32,
    first_child_offset: u32,
    next_sibling_offset: u32,
    node_kind: u8,            // 1=text, 2=container, 3=image, ...
    writing_mode: u8,         // 0=horizontal, 1=vertical-rl, 2=vertical-lr
    position: u8,             // 0=static, 1=absolute, 2=fixed
    overflow: u8,             // 0=visible, 1=hidden, 2=scroll, 3=auto
    flex_grow: f32,
    flex_shrink: f32,
    flex_basis: f32,
    layout_x: f32, layout_y: f32,        // final position
    measured_w: f32, measured_h: f32,    // measured size
    padding_left: f32, padding_right: f32, padding_top: f32, padding_bottom: f32,
    border_radius: f32,
    z_index: i32,
    top: f32, bottom: f32, left: f32, right: f32,  // for position: absolute/fixed
    text_offset: u32, text_length: u32,
    font_family_offset: u32, font_size: f32,
    axes_offset: u32,        // V-17 variable-font axes
    bg_color: u32,           // ARGB
    a11y_role: u8, a11y_state: u32,
    dirty: u8,
    _pad: [u8; 3],
}
```

State arrays of `LayoutNode` (e.g. `[LayoutNode; 4096]` for the layout arena) require V-46 (`resolve_state_array_access` for non-primitive element types — currently returns `(1, None)` for unknown element types, breaking `state_at_offset::<LayoutNode>(i)` semantics). V-46 is deferred (1 week, no ADR yet) but blocks high-density layout arrays. Workaround for v1: store `LayoutNode`s in a `Vec<LayoutNode>` (heap-backed, byte-stitched via `store_u64`/`load_u64`).

#### 4.2.11 Test plan (per RFC-04 §test-plan)

1. f32 precision: layout at (100.5, 200.25) — verify exact f32 values preserved.
2. Flex grow: 2 children with `flex_grow: 1.0` in 100px row — each is 50px.
3. Stacking context: 3 siblings with z_index 0, 5, -3 — verify render order (-3, 0, 5).
4. Position absolute: child with `position: absolute, top: 10, left: 20` — positioned at (parent.x + 20, parent.y + 10).
5. Vertical text: Japanese text with `writing_mode: vertical-rl` — verify top-to-bottom, right-to-left.
6. Knuth-Plass: paragraph at width 200 — verify balanced lines (compare against greedy).
7. Composited scroll: scroll a list of 100 items — verify momentum + bounce at edges.
8. Dirty tracking: mutate a leaf, re-layout — only that subtree re-measures.

---

### 4.3 `womb/ui/render/` (W-2, RFC-21)

**RFC**: `vuma-swe-package/21-rfc-vector-engine.md` (supersedes `05-rfc-renderer.md`).
**Build-vs-buy**: Build (VUMA + SPIR-V) for the path-tessellation compute shader (6w); Build (VUMA) for the VUMA-side scene/path/clip/blend (~3w); Build (Python script + glslangValidator) for SPIR-V build tooling (2w); Wrap (Vulkan system lib) for GPU host on Linux/Windows (4w); Wrap (Metal framework) for GPU host on macOS (3w).
**Effort**: **3 weeks (VUMA side) + 6 weeks (shader) + 2 weeks (SPIR-V build tooling) + 4 weeks (Vulkan wrap) + 3 weeks (Metal wrap) = 18 weeks**.
**VUMA-side deps**: V-26 (const byte arrays — deferred, blocks SPIR-V embedding), V-33 (Path IR instructions — deferred, optional optimization).

#### 4.3.1 Why vector (not pixel) — RFC-21 §why-vector

The renderer is **vector-based**, not pixel-based. The scene is a tree of paths (Bézier curves, lines, arcs). The GPU tessellates and fills paths in real-time via a compute shader. **No glyph atlas, no bitmap rasterization, no per-DPR textures.** Same scene renders crisp at any zoom, any DPR, any animation transform.

The pixel pipeline (RFC-05, superseded) fights the rest of the architecture: f32 coordinates, variable fonts, color fonts, composited scroll, animation, subpixel HiDPI are all vector concepts forced into a pixel pipeline. The vector engine resolves all of these by treating everything as paths.

#### 4.3.2 Scene tree = paths, not quads (RFC-21 §scene-tree)

```vuma
layout PathSegment = {
    segment_kind: u8,    // 0=line, 1=quadratic_bezier, 2=cubic_bezier, 3=arc
    _pad: [u8; 3],
    x0: f32, y0: f32,    // start point
    x1: f32, y1: f32,    // control point 1 (cubic) / end point (line)
    x2: f32, y2: f32,    // control point 2 (cubic) / unused
    x3: f32, y3: f32,    // end point (cubic/quadratic) / unused
}

layout Path = {
    segments_offset: u32,    // offset to PathSegment[] in ___pmt_buffer
    segment_count: u32,
    is_closed: u8,
    fill_rule: u8,           // 0=non-zero, 1=even-odd
    _pad: [u8; 2],
    bounds: Rect,
}

layout SceneNode = {
    parent_offset: u32,
    first_child_offset: u32,
    next_sibling_offset: u32,
    transform: Transform,    // 2x3 affine
    path_offset: u32,        // 0 = no path (pure container)
    fill_color: u32,         // ARGB; 0 = no fill
    stroke_color: u32,       // ARGB; 0 = no stroke
    stroke_width: f32,
    node_kind: u8,           // 1=path, 2=text, 3=image, 4=clip, 5=composite
    blend_mode: u8,          // 0=normal, 1=multiply, 2=screen, ...
    opacity: u8,             // 0-255
    _pad: [u8; 1],
    clip_parent_offset: u32,
}

layout Transform = {
    a: f32, b: f32,    // [a b] = scale.x, skew.x
    c: f32, d: f32,    // [c d] = skew.y, scale.y
    e: f32, f: f32,    // [e f] = translate.x, translate.y
}
```

Note: `womb/graph/digraph.vuma` is NOT a substrate for the scene tree (scene trees need parent-pointer + sibling-list, not adjacency-list). Scene tree is greenfield.

#### 4.3.3 Text as paths (no glyph atlas — RFC-21 §text-as-paths)

Text glyphs are paths extracted from the font's `glyf` table (or CFF table for OpenType). No bitmap atlas, no rasterization. The renderer tessellates glyph outlines the same as any other path. Variable-font deltas (V-17) apply to outlines before tessellation; color-font layers (V-18) are paths with palette fill colors.

This deletes: glyph atlas (R-03), glyph rasterizer (CPU), SDF text rendering (R-04), per-DPR atlas variants, color-font multi-quad rendering (R-11), dirty-rectangle rendering (R-08 — compute shader re-tessellates the whole scene per frame).

#### 4.3.4 GPU tessellation (RFC-21 §gpu-tessellation) — compute shader

The GPU tessellates all paths in the scene via a compute shader. Reference: **vello** (Linebender, Apache-2.0, ~10 kLOC). The compute shader is ~500 LOC of GLSL, compiled to SPIR-V at build time. The shader:
1. Walks the scene tree, applying transforms to each path.
2. Tessellates each path into coverage primitives (triangle strips or per-pixel coverage).
3. Fills with the path's fill color, applying blend mode + opacity.
4. Writes to the framebuffer (via storage image — no vertex/fragment pipeline needed).

```vuma
transform render_scene(scene_root: u32, device: i32, ring: State<GpuRingBuffer>) {
    let path_buffer = scene_flatten(scene_root);              // womb/ui/render/gpu_encode.vuma
    let size = path_buffer.byte_size;
    let (buf_idx, offset) = gpu_ring_alloc(ring, size, 4);
    vk_buffer_write_offset(device, ring.buffers[buf_idx], offset, path_buffer as u32, size);
    vk_bind_path_buffer(device, ring.buffers[buf_idx], offset, path_buffer.path_count, path_buffer.segment_count);
    vk_bind_viewport_uniform(device, /* w, h, dpr */);
    vk_dispatch_compute(device, path_compute_pipeline, (path_buffer.path_count + 31) / 32, 1, 1);
}
```

#### 4.3.5 Pre-compiled SPIR-V (ADR-0022, V-26) — `womb/ui/render/shaders_spirv.vuma`

Per ADR-0022 (supersedes ADR-0018 MLIR approach). The compute shader is hand-written GLSL, compiled to SPIR-V at build time by `glslangValidator` (BSD-3, build-time only — NOT a runtime dependency). A Python script wraps the .spv bytes as VUMA const byte arrays:

```makefile
# Makefile
shaders/%.spv: shaders/%.glsl
	glslangValidator -V -o $@ $<

generate_spirv_consts: $(SHADERS_SPV)
	python3 scripts/spirv_to_vuma.py $(SHADERS_SPV) > womb/ui/render/shaders_spirv.vuma
```

```vuma
// womb/ui/render/shaders_spirv.vuma (generated)
const PATH_TESSELLATE_SPIRV: [u8; 8192] = [0x03, 0x02, 0x23, 0x07, /* ... */];
const QUAD_VERT_SPIRV: [u8; 1024] = [/* ... */];
const QUAD_FRAG_SPIRV: [u8; 768] = [/* ... */];
const TEXT_VERT_SPIRV: [u8; 1280] = [/* ... */];
const TEXT_FRAG_SPIRV: [u8; 896] = [/* ... */];
```

**V-26 dependency**: V-26 adds parser support for `Lit::Bytes(Vec<u8>)` and `Expr::ArrayLit`, plus `.rodata` lowering for const byte arrays. **V-26 is deferred** (2 weeks, no ADR yet) — it blocks SPIR-V embedding and font-subsetting byte-array output. Without V-26, the SPIR-V bytes cannot be embedded in the VUMA binary; the only workaround is to load them at runtime from disk via `womb/fs/file.vuma` (`file_read_to_buffer`), which is acceptable for development but not for production binaries.

#### 4.3.6 `womb/ui/render/gpu_dispatch.vuma` (host import — ADR-0022 §what-WOMB-provides)

```vuma
extern "C" {
    transform vk_create_compute_pipeline_spirv(
        device: Address, spirv: Address, spirv_len: i64
    ) -> i64;
    transform vk_cmd_bind_pipeline(cmd: Address, pipeline: i64) -> i32;
    transform vk_cmd_dispatch(cmd: Address, x: i32, y: i32, z: i32) -> i32;
    transform vk_buffer_write_offset(device: i32, buf: i32, offset: u64, src: Address, len: u64) -> i32;
    transform vk_bind_path_buffer(device: i32, buf: i32, offset: u64, path_count: u32, segment_count: u32) -> i32;
    transform vk_bind_viewport_uniform(device: i32, w: f32, h: f32, dpr: f32) -> i32;
    transform vk_dispatch_compute(device: i32, pipeline: i64, x: u32, y: u32, z: u32) -> i32;
    // ~150 more vk_* / mtl_* / wgpu_* functions per RFC-02 §GPU
}
```

The C host runtime dispatches to `gpu_vulkan.c` (Linux/Windows) or `gpu_metal.m` (macOS). Native Metal on macOS (no MoltenVK translation overhead) per RFC-02 §cross-platform-GPU.

#### 4.3.7 Composited scroll (`womb/ui/render/scroll_layer.vuma`, R-12 / V-25)

Vector rendering means scroll just translates path coordinates — no separate texture, no re-render of content:

```vuma
transform scroll_layer_apply(scroll: State<ScrollLayer>, scene_root: u32) {
    let scroll_transform = transform_translate(-scroll.scroll_x, -scroll.scroll_y);
    scene_apply_transform_recursive(scene_root, scroll_transform);
    // The clip rect (viewport) is set as a scissor in the path shader
}
```

No `vk_begin_render_to_texture` / `vk_end_render_to_texture`.

#### 4.3.8 Clipping (`womb/ui/render/clip.vuma`, R-05) — clip paths, not scissor rects

Vector engines use **clip paths** (any path can be a clip), not just axis-aligned scissor rects. Stack-based clipping: a path is filled only where ALL clip paths in the stack cover it. Enables arbitrary clip shapes (circles, rounded rects, paths) — CSS `clip-path` becomes free.

#### 4.3.9 Blend modes + opacity (`womb/ui/render/blend.vuma`, R-06) — per-node, in shader

Each `SceneNode` carries `blend_mode` and `opacity`. The compute shader applies them per-path: tessellate → read framebuffer → apply blend → apply opacity → write back. One compute pipeline handles all blend modes via a uniform — no separate pipelines per blend mode (unlike the pixel engine).

#### 4.3.10 Color fonts (`womb/ui/render/color_glyph.vuma`, R-11 / V-18)

Each COLR layer is a glyph outline (path) with a fill color. Rendered the same as any other path — no special multi-layer rendering logic. Depends on W-3 `colr.vuma` + `cpal.vuma`.

#### 4.3.11 HiDPI — free

The same scene renders at any DPR. The viewport uniform tells the compute shader the target resolution. No per-DPR atlas, no re-rasterization on DPR change.

#### 4.3.12 Persistent GPU ring + triple-buffer (R-09, V-28)

Path data is uploaded to GPU buffers from a persistent ring (V-28). Triple-buffer + fence management. Same as the pixel engine; the only difference is the buffer holds `PathSegment[]` instead of `QuadVertex[]`.

#### 4.3.13 Test plan (per RFC-21 §test-plan)

1. Crispness: render text at 8px, 12px, 16px, 24px, 32px — verify crisp at all sizes.
2. Zoom: render scene at 0.5×, 1×, 2×, 4× — verify crisp at all zooms.
3. DPR: render at 1×, 1.5×, 2×, 3× DPR — verify crisp at all DPRs.
4. Animation scale: 0.5×→2×→0.5× — verify no blur during animation.
5. Scroll: scroll a 1000-item list — verify no texture re-render, crisp at all positions.
6. Variable font: animate weight axis 400→700 — verify outlines morph smoothly.
7. Color font: render emoji — verify crisp at all sizes.
8. Clip path: clip to a circle — verify arbitrary clip shapes work.
9. Performance: 10 000 path segments at 1080p — verify < 3 ms.
10. Memory: verify no glyph atlas allocated (path data only).

---

### 4.4 `womb/ui/text/` (W-3, RFC-06 / RFC-07 / RFC-08 / RFC-09)

**RFCs**: `06-rfc-font-parser.md`, `07-rfc-text-shaper-v1.md`, `08-rfc-text-shaper-v2-v3.md`, `09-rfc-bidi.md`.
**Build-vs-buy**: Build (VUMA) for font parser, shaper v1, hinting, subsetting, linebreak, grapheme, wordbreak; Port (from rustybuzz) for shaper v2 (GSUB + GPOS); Port (from unicode-bidi) for BiDi.
**Effort**: 6w (font parse) + 2w (shaper v1) + 10w (shaper v2/v3) + 5w (BiDi) + 3w (UAX #14 linebreak) + 2w (UAX #29 grapheme + wordbreak) + 2w (hinting simplified) + 2w (subsetting) = **~32 weeks**.
**VUMA-side deps**: V-34 (DONE), V-A2-3 (SIMD vectorizer fix — deferred; accelerates cmap / GSUB / GPOS lookups), V-26 (const byte arrays — deferred; needed for BiDi property tables, font-subset output, font byte embedding).

#### 4.4.1 `font_parse.vuma` + table parsers (W-3, RFC-06)

Per RFC-06 §tables-parsed. v1 parses:

| Table | Tag | Purpose |
|---|---|---|
| `cmap` | 0x636D6170 | Codepoint → glyph ID (formats 0/4/6/12/13/14, incl. variation sequences) |
| `hmtx` | 0x686D7478 | Horizontal metrics |
| `hhea` | 0x68686561 | Horizontal header |
| `head` | 0x68656164 | Font header (`units_per_em`, `indexToLocFormat`) |
| `maxp` | 0x6D617870 | Maximum profile (`numGlyphs`) |
| `glyf` | 0x676C7966 | Glyph outlines (TrueType) |
| `loca` | 0x6C6F6361 | Glyph location index |
| `name` | 0x6E616D65 | Font name strings |
| `OS/2` | 0x4F532F32 | OS/2 metrics |
| `vmtx` | 0x766D7478 | Vertical metrics (V-31) |
| `vhea` | 0x76686561 | Vertical header (V-31) |
| `fvar` | 0x66766172 | Variable font axes (V-17) |
| `gvar` | 0x67766172 | Variable font glyph deltas (V-17) |
| `COLR` | 0x434F4C52 | Color glyph layers (V-18) |
| `CPAL` | 0x4350414C | Color palettes (V-18) |
| `GSUB` | 0x47535542 | Glyph substitution (v2 shaper, RFC-08) |
| `GPOS` | 0x47504F53 | Glyph positioning (v3 shaper, RFC-08) |
| `GDEF` | 0x47444546 | Glyph definition (v2/v3 shaper) |

v2 adds `CFF ` (PostScript outlines), `sbix` (Apple bitmap color glyphs), `SVG ` (SVG-in-font color glyphs).

The parser uses `__vuma_load_u8/u16/u24/u32/i16` runtime intrinsics (defined in `womb/lib/mem_helpers.vuma`, J-1 §3.6). PMT-pure — every load is bounds-checked against the font's `data_length`. Malformed fonts trap with exit 134 (OOB), not UB.

#### 4.4.2 `shaper_v1.vuma` (W-3, RFC-07)

cmap lookup + hmtx/vmtx lookup + variable-font delta application + f32 advances + UTF-8↔UTF-32 conversion. **No GSUB/GPOS (v2/v3), no BiDi (separate module).** Covers ~60% of languages: Latin, Cyrillic, Greek, CJK, basic emoji (cmap format 12), emoji variation sequences (cmap format 14), variable fonts, vertical CJK.

```vuma
layout ShapedGlyph = {
    glyph_id: u32,
    cluster: u32,
    x_advance: f32,   // subpixel
    y_advance: f32,
    x_offset: f32,
    y_offset: f32,
}  // 20 bytes

transform shape_v1(text: State<TextRun>) -> State<ShapedRun> {
    let out = state_new(ShapedRun);
    // For each codepoint:
    //   1. cmap_lookup(cp, variation_selector=0)
    //   2. hmtx_get_advance (or vmtx for vertical)
    //   3. Convert font units → pixels: (advance_units as f32) * (font_size / units_per_em)
    //   4. Write ShapedGlyph (f32 advances, 0 offsets)
    // Variable-font deltas apply at rasterization time (renderer calls font_apply_gvar_deltas)
}
```

UTF-8 ↔ UTF-32 conversion re-exports `womb/lib/text/unicode.vuma` (J-1 §4.3) — that module is more comprehensive than the SWE package's W-2 `utf8.vuma` spec (which only requires "UTF-8 ↔ UTF-32 conversion"). The existing module has `utf8_encode`, `utf8_decode`, `utf8_decode_safe`, `utf8_strlen`, `utf8_strchr`, `utf8_substr`, `utf8_prev_char`, `utf8_next_char`, `utf8_char_at`, `utf8_char_len`, `utf8_seq_len`, `utf8_validate`, `utf8_valid_char`, `utf8_to_lower`, `utf8_to_upper`, classification predicates, and string-wide variants.

#### 4.4.3 `shaper_v2.vuma` (W-3, RFC-08) — Port from rustybuzz

GSUB (glyph substitution — ligatures, contextual substitutions, etc.) + GPOS (glyph positioning — kerning, mark attachment, etc.). Per `16-build-vs-buy.md`: **Port from rustybuzz** (10 weeks). rustybuzz is the Rust port of HarfBuzz; we port the algorithm to VUMA (no Rust crate dependency, per ADR-0010's 5-crate policy and the SWE package's "pure VUMA kernel" constraint).

Coverage: 95% of languages. Adds Arabic, Hebrew, Hindi/Devanagari, Thai, etc. — everything that needs substitution/positioning rules.

GSUB/GPOS lookups are the hot path; V-A2-3 (SIMD vectorizer fix — deferred, 2 weeks) accelerates coverage-table scans and binary-search lookups. Without V-A2-3, shaper v2 still works but is ~2-3× slower on long runs.

#### 4.4.4 `bidi.vuma` (W-3, RFC-09) — Port from unicode-bidi

UAX #9 bidirectional algorithm. Per `16-build-vs-buy.md`: **Port from unicode-bidi** (5 weeks). Includes:
- BiDi property table (`bidi_table.vuma`) — depends on V-26 (const byte arrays for the property table).
- Bracket pairing (`brackets.vuma`).
- Mirroring table (`mirror.vuma`).
- UAX #14 line breaking (`linebreak.vuma`) — 3 weeks, table-driven, build from UAX #14.
- Knuth-Plass optimal line breaking lives in `womb/ui/layout/linebreak_knuth_plass.vuma` (W-1, §4.2.7), which depends on `linebreak.vuma` for break-opportunity enumeration.
- UAX #29 grapheme clusters (`grapheme.vuma`, 1 week) + word segmentation (`wordbreak.vuma`, 3 days).

#### 4.4.5 `hint.vuma` (W-3, RFC-06 §hinting) — simplified

TrueType hinting interpreter. Per ADR-0022 / RFC-21: **simplified to pixel-snap only** (~2 weeks, not 5 weeks/5 kLOC). The vector renderer is inherently crisp at any size; hinting is only needed for pixel-snap at small sizes (≤ 16px). The full ~200-opcode interpreter (per RFC-06 §hinting-interpreter, ~5 kLOC, ported from FreeType's `ttinterp.c`) is deferred to v2.

Timeout: 10 000 instructions per glyph (prevents malformed fonts from causing infinite loops); on timeout, trap via `__hinting_timeout_trap()` (or skip hinting for that glyph).

#### 4.4.6 `subset.vuma` (W-3, RFC-06 §font-subsetting)

Font subsetting — reduces NotoSansCJK from 20 MB to ~200 KB for typical apps (a few thousand CJK characters). v1: BMP only (65536-bit codepoint set, 8192 bytes); v2: extend for supplementary planes.

Depends on V-26 — the subset output is a `[u8; N]` byte array. Without V-26, the subset lives in a heap-allocated buffer (`womb/collections/vec.vuma`) and is written to disk via `womb/fs/file.vuma` for offline use.

#### 4.4.7 `fontstack.vuma` (W-3, RFC-06 §font-fallback-chain)

Font fallback chain — 8-font stack: `[primary font, fallback CJK font, fallback emoji font, last-resort font, ...]`. `fontstack_lookup(stack, codepoint, variation_selector)` walks the stack, returns the first font whose `cmap` maps the codepoint to a non-zero glyph ID.

#### 4.4.8 Test plan

Per RFC-06 §test-plan, RFC-07 §test-plan, RFC-08/09. Highlights:
1. Parse: load NotoSans-Regular.ttf — verify all table offsets non-zero.
2. cmap: lookup 'A' — verify glyph ID 346 (NotoSans).
3. Variable font: Inter Variable, weight 400 vs 700 — verify outlines differ.
4. Color font: NotoColorEmoji, lookup 😀 — verify COLR layers present.
5. Vertical: NotoSansJP, vmtx advance for a CJK glyph — verify non-zero.
6. Hinting: render 'a' at 12px — verify crisp edges.
7. Hinting timeout: malformed font with infinite loop — verify trap after 10000 instructions.
8. Subsetting: subset NotoSansCJK to 1000 codepoints — verify subset < 500 KB.
9. Malformed font: truncated font — verify trap with exit 134 (OOB), not UB.
10. Latin shape: "Hello, World" at 16px — verify glyph IDs + advances match `ttf-parser`.
11. CJK shape: "你好世界" — verify 4 glyphs.
12. Arabic shape (v2): "السلام" — verify GSUB ligatures applied.
13. BiDi: "Hello \u0627\u0644\u0633\u0644\u0627\u0645" — verify RTL run reordered correctly.

---

### 4.5 `womb/ui/ime/` (W-5, RFC-10)

**RFC**: `vuma-swe-package/10-rfc-ime-bridge.md`.
**Build-vs-buy**: Build (VUMA) for composition state machine (1w); Wrap (libibus / imm32.dll / InputMethodKit) for OS bridges (2w × 3 platforms = 6w).
**Effort**: 1 week (composition) + 6 weeks (3 platform bridges) + 3 weeks (caret, format, textfield) = **~10 weeks**.
**VUMA-side deps**: V-11 (session types Choice/Offer for IME channel — deferred).

#### 4.5.1 `composition.vuma` — ImeState layout (V-08)

```vuma
layout ImeState = {
    composition_active: u8,
    composition_start: u32,
    composition_end: u32,
    saved_text_offset: u32,
    saved_text_length: u32,
    saved_selection_start: u32,
    saved_selection_end: u32,
    focused_field_offset: u32,
    _pad: [u8; 4],
}
```

State machine: `compositionstart` → 0..n × `textupdate` / `textformatupdate` → `compositionend`. The `compositionend` event carries a `flags` field: bit 1 = `is_composition_end`, bit 0 = `is_new_composition`. On cancel (`is_cancelled`), rollback to saved text.

#### 4.5.2 Session-typed IME channel (V-11)

```vuma
type ImeSession = Offer<{
    0: Recv<UiEvent, ImeSession>,    // textupdate (only valid during composition)
    1: Recv<UiEvent, ImeSession>,    // textformatupdate
    2: Recv<UiEvent, ImeSession>,    // compositionstart
    3: Recv<UiEvent, ImeSession>,    // compositionend
    4: End,                           // blur
}>;
```

IVE verifies: `textupdate`/`textformatupdate` only valid after `compositionstart`; `compositionend` must follow `compositionstart`; `End` (blur) closes the channel. **V-11 is deferred** (2 weeks parser/AST/IR + 2-4 weeks IVE checker + Lean proofs — see catalog). Without V-11, the IME channel is a plain `Channel<UiEvent>` with runtime checks (less safe but functional).

#### 4.5.3 `bridge.vuma` — host imports (f32 cursor rect)

```vuma
extern "C" {
    transform ime_create_context(window_id: i32) -> i32;
    transform ime_set_cursor_rect(window_id: i32, x: f32, y: f32, w: f32, h: f32);  // f32 subpixel
    transform ime_focus(window_id: i32);
    transform ime_blur(window_id: i32);
}
```

Per-platform bridges (RFC-10 §per-platform-host-bridges):
- **Linux**: IBus via D-Bus (`libibus`, LGPL, ~500 KB). ~500 LOC C.
- **Windows**: IMM32 (`imm32.dll`). WM_IME_COMPOSITION handler in window procedure. ~400 LOC C.
- **macOS**: IMK (`InputMethodKit.framework`) + `NSTextInputClient` protocol. ~400 LOC Objective-C.

Per RFC-10's note: file `24-rough-draft-vuma-only.md` P-10 suggests writing a D-Bus client in VUMA (using `womb/net/socket.vuma`) instead of libibus. This is a v2 optimization; v1 uses libibus (simpler, faster to ship).

#### 4.5.4 `caret.vuma` — caret position (f32)

Caret pixel position is computed by walking the shaped run up to the caret's cluster index, summing x_advances (or y_advances for vertical text). Depends on W-2 (shaper) + W-1 (layout). Returns f32 for subpixel caret positioning.

#### 4.5.5 `format.vuma` — composition formatting

```vuma
layout FormatRange = {
    start: u32,
    end: u32,
    style: u8,    // 0=none, 1=solid, 2=dotted, 3=thick
    color: u32,
}
```

The renderer (W-2) draws an underline under the composition range. Depends on W-5 (vector renderer).

#### 4.5.6 Test plan (per RFC-10 §test-plan)

1. Japanese: "konnichiha" in Mozc → composition → こんにちは.
2. Chinese Pinyin: "nihao" → candidate popup → 你好.
3. Korean: 안녕하세요 → Hangul composition.
4. Cancel: Escape mid-composition → rollback to saved text.
5. Blur: click away mid-composition → force-commit (or cancel per platform).
6. Subpixel cursor: caret at (100.5, 200.25) → IME popup at exact position.
7. Multi-field: focus A, start composition; focus B → A committed before B starts.

---

### 4.6 `womb/ui/a11y/` (W-6, RFC-11)

**RFC**: `vuma-swe-package/11-rfc-a11y-bridge.md`.
**Build-vs-buy**: Build (VUMA) for semantics tree + build + diff + reverse + preferences (4w); Wrap (libatspi / uiautomationcore.dll / AppKit) for OS bridges (2w × 3 platforms = 6w).
**Effort**: 4 weeks (VUMA) + 6 weeks (3 platform bridges) = **~10 weeks**.
**VUMA-side deps**: V-34 (DONE), V-03 (IVE soundness for nested SemanticsNode trees).

#### 4.6.1 `semantics.vuma` — SemanticsNode layout (V-08, f32 bounds)

```vuma
layout SemanticsNode = {
    node_id: u32,
    role: u8,             // 1=button, 2=text, 3=textfield, 4=list, 5=listitem, ...
    state: u32,           // bitflags: focused, disabled, selected, expanded, checked, ...
    name_offset: u32, name_length: u32,
    value_offset: u32, value_length: u32,
    bounds: Rect,         // f32 (subpixel)
    first_child_offset: u32,
    next_sibling_offset: u32,
    parent_offset: u32,
    action_callback: u64,
    _pad: [u8; 4],
}
```

#### 4.6.2 `build.vuma` — LayoutNode → SemanticsNode tree (A-01)

Walks the layout tree, assigns stable node IDs (path-based hash — see §4.6.3), copies role/state/bounds. Depends on W-1 layout.

#### 4.6.3 Stable node IDs (A-05)

```vuma
transform a11y_assign_id(parent_id: u32, node_kind: u8, node_index: u32) -> u32 {
    let h = parent_id ^ ((node_kind as u32) * 2654435761) ^ (node_index * 40503);
    h = (h ^ (h >> 16)) * 0x85ebca6b;
    h = (h ^ (h >> 13)) * 0xc2b2ae35;
    return h ^ (h >> 16);
}
```

Path-based hash — stable across frames (so the screen reader's "focus" doesn't jump when the tree is rebuilt).

#### 4.6.4 `diff.vuma` — tree diff (A-02)

Recursive diff: node added, node removed, state changed, value changed, name changed, bounds changed (f32 comparison). Emits A11y events into a queue, which `bridge.vuma` forwards to the OS a11y API.

#### 4.6.5 `bridge.vuma` — host imports (f32 bounds)

```vuma
extern "C" {
    transform a11y_register() -> i32;
    transform a11y_emit_focus(node_id: u32);
    transform a11y_emit_state_changed(node_id: u32, new_state: u32);
    transform a11y_emit_value_changed(node_id: u32, value_ptr: Address, value_len: u32);
    transform a11y_emit_name_changed(node_id: u32, name_ptr: Address, name_len: u32);
    transform a11y_emit_bounds_changed(node_id: u32, x: f32, y: f32, w: f32, h: f32);  // f32
    transform a11y_emit_announcement(text_ptr: Address, text_len: u32);
}
```

Per-platform bridges (RFC-11 §per-platform-bridges):
- **Linux**: AT-SPI (`libatspi` over D-Bus, LGPL, ~500 KB). ~500 LOC C.
- **Windows**: UIAutomation (`uiautomationcore.dll`, COM). ~600 LOC C.
- **macOS**: NSAccessibility (`NSAccessibilityPostNotification`, AppKit). ~400 LOC Objective-C.

Per RFC-11's note: file `24-rough-draft-vuma-only.md` P-11 suggests a D-Bus client in VUMA (using `womb/net/socket.vuma`) instead of libatspi. v2 optimization; v1 uses libatspi.

#### 4.6.6 `reverse.vuma` — a11y actions (A-06)

Screen readers invoke actions on nodes (e.g. "click" via Enter). Pushed as `EVENT_A11Y_ACTION`:

```vuma
transform ui_on_a11y_action(engine: State<UiEngine>, event: State<UiEvent>) {
    let node_id = __vuma_load_u32(event as u32 + 16);
    let action_kind = __vuma_load_u32(event as u32 + 20);
    let node = a11y_find_node_by_id(engine.semantics_root, node_id);
    if node == 0 { return; }
    let sem = state_at_offset::<SemanticsNode>(node);
    if action_kind == A11Y_ACTION_CLICK {
        if sem.action_callback != 0 { __call_indirect1(sem.action_callback, node_id); }
    } else if action_kind == A11Y_ACTION_FOCUS {
        ui_set_focus(engine, sem.node_id);
    } else if action_kind == A11Y_ACTION_SCROLL_TO {
        ui_scroll_to_node(engine, node);
    }
}
```

#### 4.6.7 `preferences.vuma` — high-contrast + reduced-motion (A-08, A-09)

```vuma
extern "C" {
    transform host_get_color_scheme() -> u8;  // 0=light, 1=dark
    transform host_get_contrast() -> u8;       // 0=normal, 1=high
    transform host_get_reduced_motion() -> u8;
}
```

Host polls OS preference:
- Linux: `gsettings get org.gnome.desktop.interface color-scheme` (D-Bus).
- Windows: registry `HKEY_CURRENT_USER\Software\Microsoft\Windows\CurrentVersion\Themes\Personalize\AppsUseLightTheme`.
- macOS: `NSUserDefaults` `AppleInterfaceStyle`.

On change, host pushes `EVENT_THEME_CHANGE` (or `EVENT_VISIBILITY_CHANGE`) into the state ring. The UI engine re-layouts and announces.

#### 4.6.8 Test plan (per RFC-11 §test-plan)

1. NVDA (Windows): tab through buttons — NVDA announces names.
2. VoiceOver (macOS): navigate via trackpad — VoiceOver reads focused element.
3. Orca (Linux): keyboard navigation — Orca announces.
4. State change: toggle checkbox — screen reader announces "checked".
5. Value change: type in textfield — screen reader announces new text.
6. Action: invoke "click" via screen reader — button action fires.
7. Subpixel bounds: a11y node at (100.5, 200.25) — screen reader highlights exact bounds.
8. Theme switch: switch OS dark mode — app receives `EVENT_THEME_CHANGE`, re-layouts.
9. High contrast: enable OS high contrast — app switches to high-contrast theme.
10. Reduced motion: enable OS reduced motion — animations snap to end.

---

### 4.7 `womb/ui/animation.vuma` (W-7, RFC-04 §built-in-animation-system)

**RFC**: `vuma-swe-package/04-rfc-layout-engine.md` §built-in-animation-system (L-12, V-24).
**Build-vs-buy**: Build (VUMA) — reference: iOS UIView.animate.
**Effort**: **2 weeks**.
**VUMA-side deps**: V-34 (DONE). Reuses `womb/lib/sys/time.vuma` (`time_monotonic_ns()`) + `womb/lib/sys/math.vuma` (`sin`/`cos`/`pow`/`sqrt`).

Easing curves: linear, ease-in, ease-out, ease-in-out, cubic-bezier (Newton-Raphson to find t for given x). Keyframe animations: array of (time, value) pairs, piecewise interpolation.

```vuma
const EASING_LINEAR: u8 = 0;
const EASING_EASE_IN: u8 = 1;
const EASING_EASE_OUT: u8 = 2;
const EASING_EASE_IN_OUT: u8 = 3;
const EASING_CUBIC_BEZIER: u8 = 4;

transform animate(from: f32, to: f32, duration_ms: u32, easing: u8) -> State<Animation> {
    let anim = state_new(Animation);
    anim.from_value = from;
    anim.to_value = to;
    anim.start_time_ms = host_get_time_ms();
    anim.duration_ms = duration_ms;
    anim.easing = easing;
    anim.current_value = from;
    anim.completed = 0;
    if host_get_reduced_motion() == 1 {
        anim.current_value = to;  // snap to end
        anim.completed = 1;
    }
    return anim;
}
```

For the vector renderer, scale/rotate/skew animations apply to the `Transform` field of `SceneNode` — no pixel artifacts (the path is unchanged; only the transform changes; GPU tessellates with the new transform → crisp at any scale).

---

### 4.8 `womb/ui/theme.vuma` (W-7, RFC-11 §multi-theme-support)

**RFC**: `vuma-swe-package/11-rfc-a11y-bridge.md` §multi-theme-support (A-08, A-09, V-29, V-32).
**Build-vs-buy**: Build (VUMA + C host) — per-platform OS APIs.
**Effort**: **2 weeks**.
**VUMA-side deps**: V-34 (DONE). Reuses `womb/lib/text/json.vuma` for theme-file parsing.

```vuma
layout ThemeManager = {
    themes: [u32; 3],     // offsets to Theme states (light, dark, high_contrast)
    active_index: u32,
}

transform theme_manager_init() -> State<ThemeManager> {
    let mgr = state_new(ThemeManager);
    mgr.themes[0] = build_light_theme();      // parses theme-light.json
    mgr.themes[1] = build_dark_theme();       // parses theme-dark.json
    mgr.themes[2] = build_high_contrast_theme();  // parses theme-high-contrast.json

    let color_scheme = host_get_color_scheme();
    let contrast = host_get_contrast();
    if contrast == 1 { mgr.active_index = 2; }
    else if color_scheme == 1 { mgr.active_index = 1; }
    else { mgr.active_index = 0; }
    return mgr;
}

transform ui_on_theme_change(engine: State<UiEngine>, event: State<UiEvent>) {
    let theme = __vuma_load_u32(event as u32 + 24);
    let mgr = engine.theme_manager;
    if theme == 2 { mgr.active_index = 2; }
    else if theme == 1 { mgr.active_index = 1; }
    else { mgr.active_index = 0; }
    layout_mark_dirty(engine.layout_root);   // re-layout all nodes with new theme
    a11y_emit_announcement("Theme changed", 13);
}
```

Theme files are JSON (parsed by `womb/lib/text/json.vuma`). The active theme is looked up by every layout node that has a `bg_color` / `text_color` / `border_color` field; the layout engine resolves the color name to an ARGB value at measure time.

---

## 5. Host runtime (C shim, from RFC-02)

**Source**: `vuma-swe-package/02-rfc-host-runtime.md` (note: superseded by file `24-rough-draft-vuma-only.md` for the ~500 LOC shim framing; the build-vs-buy matrix from file `16-build-vs-buy.md` still applies for the OS-provided libraries the shim wraps).

### 5.1 Architecture

The C host runtime is a thin (~500 LOC) shim that wraps OS-provided libraries and exposes a uniform `extern "C"` ABI to the VUMA binary. The shim:

- Loads the capability bundle, generates the HMAC-SHA-256 signing key (mlock'd).
- Creates the window via SDL2.
- Loads the VUMA binary via `dlopen` / `dlsym`.
- Runs the VUMA `ui_main_loop()` on a worker thread.
- Runs the host event loop on the main thread (required by macOS Cocoa).
- Drains SDL2 events into the two-ring event pipeline (state ring + stream ring).
- Dispatches OS IME / a11y / file-picker / network callbacks into the state ring as `UiEvent`s.
- Calls `cap_frame_begin()` per frame to invalidate the per-frame capability cache.

```c
// host-runtime/src/main.c (~200 LOC)
int main(int argc, char** argv) {
    FILE* f = fopen(argv[1], "rb");
    // ... read cap bundle ...
    vuma_host_init(cap_bundle_ptr, cap_bundle_len);
    cap_init();  // generate HMAC-SHA-256 key
    int32_t window = window_create("VUMA UI", 7, 1280, 720);
    void* binary = dlopen(argv[2], RTLD_NOW);
    int (*vuma_main)(void) = dlsym(binary, "main");
    pthread_t vuma_thread;
    pthread_create(&vuma_thread, NULL, (void*(*)(void*))vuma_main, NULL);

    while (!should_quit) {
        cap_frame_begin();
        window_poll_events(window);          // → state_ring
        window_poll_stream_events(window);   // → stream_ring
        process_pending_callbacks();          // IME / a11y / file / net
        usleep(1000);                        // yield CPU (1 ms)
    }
    pthread_join(vuma_thread, NULL);
    vuma_host_shutdown();
    return 0;
}
```

### 5.2 Host import surface (~170 `extern "C"` functions)

Per RFC-02 §host-import-surface. Grouped:

| Group | Imports | Backend | LOC estimate |
|---|---|---|---|
| Window + event loop (H-01) | `window_create`, `window_get_size`, `window_set_size`, `window_poll_events`, `window_poll_stream_events`, `window_close`, `window_get_dpr` | SDL2 | ~400 LOC |
| GPU Vulkan (H-02, H-11) | `gpu_context_create`, `gpu_create_swapchain`, `gpu_ring_buffer_create`, `gpu_create_texture`, `gpu_create_pipeline_spirv`, `gpu_begin_render_pass`, `gpu_bind_pipeline`, `gpu_draw`, `gpu_set_scissor`, `gpu_set_blend_mode`, `gpu_texture_upload`, ~150 more | Vulkan (Linux/Windows) | ~3 kLOC |
| GPU Metal (H-11) | Same surface, dispatched to Metal | Metal framework (macOS) | ~2 kLOC |
| OS IME (H-03, H-12) | `ime_create_context`, `ime_set_cursor_rect`, `ime_focus`, `ime_blur` | libibus (Linux), IMM32 (Windows), IMK (macOS) | ~1.3 kLOC |
| OS a11y (H-04, H-13) | `a11y_register`, `a11y_emit_focus`, `a11y_emit_state_changed`, `a11y_emit_value_changed`, `a11y_emit_name_changed`, `a11y_emit_bounds_changed`, `a11y_emit_announcement` | libatspi (Linux), UIA (Windows), NSAccessibility (macOS) | ~1.5 kLOC |
| Theme auto-switch (H-14, V-32) | `host_get_color_scheme`, `host_get_contrast` | gsettings (Linux), registry (Windows), NSUserDefaults (macOS) | ~300 LOC |
| Clipboard (H-05) | `clipboard_read_text`, `clipboard_write_text`, `clipboard_read_image`, `clipboard_write_image` | X11/Wayland (Linux), Win32 (Windows), NSPasteboard (macOS) | ~600 LOC |
| File picker (H-06) | `filepick_open`, `filepick_save`, `filepick_cancel` | zenity/kdialog/XDG portal (Linux), IFileDialog (Windows), NSOpenPanel (macOS) | ~600 LOC |
| Network (H-07) | `net_fetch`, `net_fetch_cancel`, `net_websocket_open`, `net_websocket_send`, `net_websocket_close` | libcurl + libwebsockets (or `womb/net/*.vuma` for native-only) | ~1 kLOC |
| Capability verifier (H-08) | `cap_init`, `cap_verify`, `cap_verify_cached`, `cap_revoke`, `cap_frame_begin` | OpenSSL HMAC | ~800 LOC |
| Two-ring + yield (H-09, H-10) | `ring_push_state`, `ring_push_stream`, `ring_pop_state`, `ring_pop_stream`, `host_yield`, `host_side_buffer_read` | C11 stdatomic + pthread | ~1 kLOC |
| Theme + DPR polling (H-14) | `host_get_color_scheme`, `host_get_contrast`, `host_get_reduced_motion` | per-platform | ~300 LOC |
| SPIR-V loader (H-15) | (called from VUMA via `vk_create_compute_pipeline_spirv`) | (none — just bytes in `.rodata`) | ~100 LOC |
| **Total C shim** | | | **~13 kLOC** (per RFC-02 §estimated-effort) |

Note: file `24-rough-draft-vuma-only.md` proposes a **~500 LOC shim** that pushes more of the host-bridge logic into VUMA (D-Bus client in VUMA, HTTP client via `womb/net/http.vuma`, etc.). The ~500 LOC framing is the v2 target; v1 ships the ~13 kLOC shim per RFC-02 because (a) SDL2/Vulkan/Metal wrappers are irreducibly in C, (b) libibus/libatspi are C libraries, and (c) the WOMB team has higher-priority work than porting D-Bus to VUMA.

### 5.3 Host-runtime dependencies (per `16-build-vs-buy.md` §dependencies-to-add)

| Library | License | Size | Purpose |
|---|---|---|---|
| SDL2 | zlib | ~1 MB | Window, event loop |
| Vulkan | MIT | (system) | GPU (Linux/Windows) |
| Metal | Apple | (system) | GPU (macOS) |
| libcurl | MIT | ~1 MB | HTTP |
| libwebsockets | MIT | ~500 KB | WebSocket |
| libibus | LGPL | ~500 KB | IME (Linux) |
| libatspi | LGPL | ~500 KB | a11y (Linux) |
| OpenSSL | Apache-2.0 | ~2 MB | HMAC-SHA-256 |
| glslangValidator | BSD-3 | (build-time only) | SPIR-V compilation |

**Total host binary**: ~7 MB (host runtime + libs) + VUMA binary (~3-5 MB) = ~10-12 MB.

### 5.4 What is NOT used (per `16-build-vs-buy.md` §what-we-are-NOT-using)

| Library | Why not |
|---|---|
| wgpu (Rust) | Pure VUMA kernel, no Rust crates |
| rustybuzz (Rust) | Same — port the algorithm to VUMA |
| ttf-parser (Rust) | Same — study for reference |
| cosmic-text (Rust) | Bundles own layout engine |
| swash (Rust) | Unnecessary with custom atlas (deleted in vector engine — no atlas at all) |
| unicode-bidi (Rust, stale) | Port the algorithm to VUMA |
| unicode-linebreak (Rust, stale) | Build from UAX #14 |
| gfx-rs | Deprecated |
| MoltenVK | Native Metal is better (no translation overhead) |
| FreeType | ~150 kLOC, too heavy — port `ttinterp.c` for hinting (simplified, v1) |
| HarfBuzz | C++, ~80 kLOC — port from rustybuzz |
| Pango | Wraps HarfBuzz + FreeType |
| Qt | ~50 MB, GPL/LGPL |
| GTK | ~30 MB, LGPL |
| shaderc | ~2 MB — pre-compiled SPIR-V instead (V-26) |
| MLIR | C++ toolchain, massive — rejected by ADR-0022 |
| Cranelift | Rust crate, would be 6th dep |
| LLVM | C++, no e-graphs, violates hand-write philosophy |

### 5.5 Cross-platform GPU (H-11) — Vulkan + native Metal

| Platform | GPU API | Library | Notes |
|---|---|---|---|
| Linux | Vulkan | Mesa / NVIDIA driver | Native |
| Windows | Vulkan | GPU vendor driver | Native |
| macOS | **Metal** (native) | Metal framework | **Not MoltenVK** — no translation overhead |

The WOMB-side code calls `gpu_*` / `vk_*` functions identically on all platforms. The host runtime dispatches to `gpu_vulkan.c` (Linux/Windows) or `gpu_metal.m` (macOS). Trade-off: two GPU code paths (~6 kLOC total) instead of one (Vulkan + MoltenVK, ~3 kLOC). Worth it: ~5-10% faster draw calls on macOS, no MoltenVK dependency (~2 MB savings), access to Metal-specific features.

---

## 6. WOMB-side bugs

### 6.1 V-WOMB-1 — broken `womb/net/*.vuma` imports (ADR-0020)

**Severity**: P1 (blocks WOMB UI engine's network layer).
**Status**: Open (verified by Wave J audit, `docs/research/J-1-womb-layer.md` §5.4). ADR-0020 accepted.
**Effort**: 1 day.

The `crypto/` subtree was reorganized into subdirectories (`crypto/mac_kdf/`, `crypto/aead/`, `crypto/hash/`, etc.) but the imports in `womb/net/` were never updated. The VUMA module resolver (`src/parser/src/resolver.rs:500-510`) is a simple `base_dir.join(path)` with no fallback — broken imports fail compilation.

**Affected files** (per ADR-0020 §affected-files):

| File | Broken import | Should be |
|---|---|---|
| `womb/net/ssh.vuma` | `import "crypto/hmac.vuma"` | `import "crypto/mac_kdf/hmac.vuma"` |
| `womb/net/quic.vuma` | `import "crypto/hmac.vuma"` | `import "crypto/mac_kdf/hmac.vuma"` |
| `womb/net/tls12.vuma` | `import "crypto/hmac.vuma"` | `import "crypto/mac_kdf/hmac.vuma"` |
| `womb/net/tls13.vuma` | `import "crypto/hmac.vuma"` | `import "crypto/mac_kdf/hmac.vuma"` |
| `womb/net/http2.vuma` | `import "crypto/hmac.vuma"` | `import "crypto/mac_kdf/hmac.vuma"` |
| `womb/net/http3_mqtt_coap.vuma` | `import "crypto/hmac.vuma"` | `import "crypto/mac_kdf/hmac.vuma"` |
| `womb/net/websocket.vuma` | `import "crypto/hmac.vuma"` | `import "crypto/mac_kdf/hmac.vuma"` |
| `womb/lib/sys/email.vuma` | `import "crypto/hmac.vuma"` | `import "crypto/mac_kdf/hmac.vuma"` |

(Exact paths to be verified during implementation — the audit found these 8 files; there may be more with different broken imports — J-1 §5.4 lists additional broken imports to `crypto/hqc.vuma`, `crypto/aes256.vuma`, `crypto/bignum2048.vuma`, `crypto/hkdf.vuma`, `crypto/aes_modes.vuma`, `crypto/drbg.vuma`, `crypto/sha1.vuma`.)

**Fix**:
1. Audit all imports in `womb/net/` and `womb/lib/` — `grep 'import "crypto/'` to find every file that references the old flat layout.
2. Map each broken import to the correct new path (verify against current `womb/crypto/` structure).
3. Update each broken import — mechanical sed pass.
4. Add a CI check that compiles every `.vuma` file in `womb/` to catch future import breakage (also V-NEW-8 from the catalog — the full test matrix should be in CI).
5. Add a regression test that imports `womb/net/websocket.vuma` and compiles it, to prevent future breakage.

**Why fix first**: Without V-WOMB-1 fixed, `womb/net/websocket.vuma` is DEAD code, which blocks `womb/ui/net/ws_bridge.vuma` (W-8) from reusing it on the native target. The fix is a one-day mechanical pass; ship it as Phase 1 of the execution plan (§8).

### 6.2 V-A2-4 — dead-code backend arms for `IRInstr::Channel*` / `StarkProof` / etc. (P3)

**Severity**: P3 (dead code, no functional impact).
**Status**: Open.
**Effort**: 3 weeks (mechanical deletion across 14+ backends).

The `=> {}` arms in 14+ backends for `IRInstr::ChannelSend`, `ChannelRecv`, `StarkProof`, `Transform`, `BulkCopy`, `BulkFill`, etc. are dead code: `ipc_lowering.rs` lowers these Call-form builtins to ordinary IR before the backend sees them. These builtins are what WOMB's `womb/ui/event/` (channel-based event dispatch), `womb/ui/ime/composition.vuma` (session-typed IME channel), and `womb/ui/capability.vuma` (W-9 capability tokens) build on.

**Fix**: Delete the `=> {}` arms (or replace with `unreachable!()` / `panic!("lowered by ipc_lowering.rs before backend sees it")`).

**Why P3**: Not blocking — the dead arms don't cause incorrect behavior, just clutter. But once W-0 lands, it's worth doing so the backend dead-code doesn't mask future regressions (e.g. if `ipc_lowering.rs` ever fails to lower a builtin, the backend would silently no-op rather than panic).

---

## 7. Dependency on VUMA fixes

The WOMB UI engine depends on the following VUMA-side (Layer 1) fixes. Statuses are from `docs/vuma-side-problem-catalog.md` at commit `6dc97e18`.

### 7.1 Master dependency table

| WOMB module | Required VUMA fix | Status | Effort | ADR | Blocking? |
|---|---|---|---|---|---|
| All `womb/ui/*` (PMT soundness for nested layouts) | V-03 (legacy `bridge_type_size` still used by `build_pmt_layout_specs`) | Open (verified) | 1 week | ADR-0004 | Yes — affects IVE soundness for nested `LayoutNode` / `SemanticsNode` trees |
| `womb/ui/event/normalize.vuma` (UiEvent f32 fields) | V-34 (`bridge_type_to_ir_type` doesn't map f32/f64) | **DONE** (treated as fixed — first step of bridge-fix epic) | 3 days | ADR-0001 | Yes (was) — unblocks f32 state fields |
| `womb/ui/layout/*` (f32 coordinates) | V-34 | **DONE** | 3 days | ADR-0001 | Yes (was) |
| `womb/ui/layout/*` (LayoutNode arrays) | V-46 (`resolve_state_array_access` `_ => (1, None)` for unknown element types) | Open (deferred) | 1 week | (deferred) | Workaround: `Vec<LayoutNode>` (heap-backed) instead of `[LayoutNode; N]` |
| `womb/ui/render/shaders_spirv.vuma` (SPIR-V embedding) | V-26 (parser lacks const byte arrays / `Expr::ArrayLit`) | Open (deferred) | 2 weeks | (deferred) | Workaround: load SPIR-V from disk at runtime via `womb/fs/file.vuma` |
| `womb/ui/render/*` (Path IR instructions — optional) | V-33 (PathFromGlyph, PathRect, PathAppendSegment, PathTransform, SceneFlatten) | Open (deferred) | 2 weeks | (deferred) | No — VUMA can use plain `state_write` byte-stitched calls instead |
| `womb/ui/text/shaper_v2.vuma` (GSUB/GPOS acceleration) | V-A2-3 (SIMD vectorizer fix — hardcodes `Xmm0/1/2`, `V0/1/2`; vectorizer non-functional) | Open (deferred) | 2 weeks | ADR-0025 | No — shaper v2 works without SIMD, ~2-3× slower on long runs |
| `womb/ui/text/{font_parse, cmap, glyf, fvar, gvar, colr, cpal}.vuma` (font byte embedding — optional) | V-26 (const byte arrays) | Open (deferred) | 2 weeks | (deferred) | Workaround: load font from disk at runtime via `womb/fs/file.vuma` |
| `womb/ui/text/bidi.vuma` (BiDi property table) | V-26 (const byte arrays for the property table) | Open (deferred) | 2 weeks | (deferred) | Workaround: load table from disk at runtime |
| `womb/ui/text/subset.vuma` (subset output) | V-26 (const byte arrays for the subset `[u8; N]`) | Open (deferred) | 2 weeks | (deferred) | Workaround: write subset to disk via `womb/fs/file.vuma` |
| `womb/ui/ime/composition.vuma` (session-typed IME channel) | V-11 (session types lack `Choice`/`Offer`) | Open (corrected) — IVE-side `session_type.rs:38-56` already has them as dead code; AST/IR + parser + Lean proof remain | 2 weeks (down from 2-4) | (deferred) | Workaround: plain `Channel<UiEvent>` with runtime checks (less safe) |
| `womb/ui/capability.vuma` (HMAC-SHA-256 capability tokens) | V-16 (capability signatures use FNV-1a × 4, not HMAC-SHA-256; `verify_capability` never called) | Open (corrected, expanded scope) | 7 weeks | ADR-0007 | Yes — capability model is unforgeable only against accidental collision, not adversarial |
| `womb/ui/cap_cache.vuma` (per-frame capability cache) | V-09 (capability-model IR plumbing) + C-06 (per-frame verification cache) | (deferred) | (within V-16's 7 weeks) | (within ADR-0007) | No — cache is a perf optimization |
| `womb/net/*.vuma` (V-WOMB-1 fix unblocks reuse on native) | V-WOMB-1 (broken `womb/net/*.vuma` imports) | Open (ADR-0020 accepted) | 1 day | ADR-0020 | Yes — `womb/ui/net/{http,ws}_bridge.vuma` reuse `womb/net/{http,websocket}.vuma` on native target |

### 7.2 The bridge-fix epic (prerequisite for almost all WOMB UI work)

Per `docs/vuma-side-problem-catalog.md` §recommended-execution-order, the bridge-fix epic (~10 weeks) is the prerequisite for every WOMB layout and renderer module that uses f32 coordinates or nested-struct state fields:

1. **ADR-0001 / V-34** (3 days) — add f32/f64 arms to `bridge_type_to_ir_type`. No deps.
2. **ADR-0002 / V-35 + V-44** (1 week + 2 days) — fix `type_size_from_name` + `type_alignment` to consult layouts table. No deps.
3. **ADR-0004 / V-03 + V-NEW-2** (1 week + 3 days) — migrate `build_pmt_layout_specs` + IVE `rederive_layout` to `bridge_type_size_with_layouts`. Deps: ADR-0001, ADR-0002.
4. **ADR-0003 / V-36 + V-A2-1** (1 week + 1 week) — thread IRType through `StateRead`/`StateWrite` + fix `Alloc { size: 0 }`. Deps: ADR-0001, ADR-0004.
5. **V-46** (1 week) — fix `resolve_state_array_access` `_ => (1, None)`. Deps: ADR-0002.
6. **V-NEW-1** (1 week) — fix `allocate(<non-literal>)` truncation. Deps: ADR-0002.
7. **ADR-0005 / V-40** (1 day) — delete legacy `bridge_type_size` + delete `cc`/`find-msvc-tools`/`shlex` build-deps. Deps: ADR-0004 landed.
8. **ADR-0009** — re-run full test suite on `main` HEAD. No deps; can run in parallel with steps 1–7.

After the bridge-fix epic lands:
9. **ADR-0008** (3 days) — fix `discharge_rate` denominator. No deps.
10. **ADR-0006** — defer f32 PMT Lean proof to v2; add `__float_overflow_trap` stub on all 19 backends. No deps.
11. **ADR-0007** (7 weeks) — HMAC-SHA-256 capability signatures + wire `verify_capability`. Should land after ADR-0005 to avoid merge conflicts in `Cargo.toml`.
12. **ADR-0010** (1 day) — adopt "5 external crates max" policy + document in `contributing.md`. Deps: ADR-0005 landed.

The bridge-fix epic (steps 1-7, ~10 weeks) is the prerequisite for WOMB Phases 3-7 (§8).

---

## 8. Execution plan

### Phase 0 — Bridge-fix epic (VUMA team, ~10 weeks, parallel with Phase 1)

Per `docs/vuma-side-problem-catalog.md` §recommended-execution-order. Steps 1-7 above. The WOMB team can start Phase 1 (V-WOMB-1 fix) immediately — it's independent of the bridge-fix epic. The WOMB team can also start Phase 2 (spsc.vuma generalization) immediately — it doesn't depend on f32 / nested-struct support.

### Phase 1 — V-WOMB-1 fix (WOMB team, 1 day)

- Audit all imports in `womb/net/` and `womb/lib/` (`grep 'import "crypto/'`).
- Map each broken import to the correct new path.
- Update each broken import — mechanical sed pass.
- Add CI check that compiles every `.vuma` file in `womb/`.
- Add regression test that imports `womb/net/websocket.vuma` and compiles it.

**Deliverable**: 8 dead files become live again. `womb/net/websocket.vuma` is unblocked for W-8.

### Phase 2 — `womb/sync/spsc.vuma` generalization (WOMB team, 1 week)

- Extract the SPSC ring logic from `womb/kernel/trap/irq_ring.vuma` (472 LOC) into `womb/sync/spsc.vuma` (~100 LOC).
- `womb/kernel/trap/irq_ring.vuma` becomes a thin wrapper that instantiates `SpscRing` with `slot_size = 8, slot_count = 32`.
- Add `womb/sync/spsc_test.vuma` (regression tests).
- Update `scripts/test_womb_compile.sh` to follow imports (compile-import each module's transitive closure) — this is the CI hardening that ADR-0020 §implementation-step-4 calls for.

**Deliverable**: `womb/sync/spsc.vuma` ready for W-0 `womb/ui/event/ring.vuma` to wrap.

### Phase 3 — `womb/ui/event/` + `womb/ui/layout/` (parallel, WOMB team, 4 + 12 weeks)

After Phase 0 (bridge-fix epic) lands, two parallel work-streams:

**3a. `womb/ui/event/`** (~4 weeks):
- `womb/ui/event/ring.vuma` (wraps `womb/sync/spsc.vuma` with `slot_size = 64`, two instantiations: state ring `slot_count = 32`, stream ring `slot_count = 256`).
- `womb/ui/event/normalize.vuma` (UiEvent layout, payload decode).
- `womb/ui/event/dispatch.vuma` (state ring first, stream ring second; hit-testing).
- `womb/ui/event/yield.vuma` (host_yield extern — adapts `womb/kernel/arch/wasm32/sched_hal.vuma`).
- `womb/ui/event/*_test.vuma` (regression tests per RFC-03 §test-plan).

**3b. `womb/ui/layout/`** (~12 weeks):
- `womb/ui/layout/node.vuma` (LayoutNode layout + measure/position entry points).
- `womb/ui/layout/flex.vuma` (Flexbox measure + position + distribute).
- `womb/ui/layout/stacking.vuma` (stacking contexts, z-index DAG via `womb/graph/algorithms.vuma`).
- `womb/ui/layout/position.vuma` (absolute/fixed, containing-block walk).
- `womb/ui/layout/dirty.vuma` (dirty propagation, topological sort).
- `womb/ui/layout/scroll.vuma` (composited scroll, momentum + bounce).
- `womb/ui/layout/vertical.vuma` (deferred to Phase 4 — depends on W-3 `vmtx.vuma`).
- `womb/ui/layout/linebreak_knuth_plass.vuma` (deferred to Phase 4 — depends on W-3 `linebreak.vuma`).
- `womb/ui/layout/*_test.vuma` (regression tests per RFC-04 §test-plan).

### Phase 4 — `womb/ui/text/` (parallel sub-streams, WOMB team, ~32 weeks)

After Phase 0 lands. Three parallel sub-streams:

**4a. Font parser** (~6 weeks): `font_parse.vuma` + `cmap.vuma` + `hmtx.vuma` + `vmtx.vuma` + `glyf.vuma` + `fvar.vuma` + `gvar.vuma` + `colr.vuma` + `cpal.vuma` + `fontstack.vuma`. Study `ttf-parser` for reference. Re-export `womb/lib/text/unicode.vuma` as `womb/ui/text/utf8.vuma` (or migrate the relevant functions).

**4b. Shaper v1 + hinting + subsetting** (~6 weeks): `shaper_v1.vuma` (2w) + `hint.vuma` (2w, simplified pixel-snap) + `subset.vuma` (2w). All Build (VUMA).

**4c. Shaper v2 + BiDi + linebreak + grapheme/wordbreak** (~20 weeks): `shaper_v2.vuma` (10w, port from rustybuzz) + `bidi.vuma` (5w, port from unicode-bidi) + `linebreak.vuma` (3w, UAX #14) + `grapheme.vuma` + `wordbreak.vuma` (2w combined, UAX #29).

Phase 4 also unblocks `womb/ui/layout/vertical.vuma` (needs W-3 `vmtx.vuma`) and `womb/ui/layout/linebreak_knuth_plass.vuma` (needs W-3 `linebreak.vuma`).

### Phase 5 — `womb/ui/render/` (WOMB team, ~18 weeks)

After Phase 0 (V-34) lands. Phase 5 has three sub-streams:

**5a. VUMA-side scene/path/clip/blend/scroll_layer/frame** (~3 weeks): All Build (VUMA). Includes `path.vuma`, `scene.vuma`, `scene_build.vuma`, `outline_to_path.vuma`, `gpu_encode.vuma`, `clip.vuma`, `blend.vuma`, `color_glyph.vuma`, `scroll_layer.vuma`, `frame.vuma`. Depends on Phase 3b (layout tree → scene tree) and Phase 4a (glyf → path).

**5b. Compute shader (~500 LOC GLSL) + SPIR-V build tooling** (~8 weeks): 6w (shader) + 2w (Python script wrapping glslangValidator output as VUMA const byte arrays). Per ADR-0022. **V-26 dependency** — without V-26, the generated `shaders_spirv.vuma` cannot embed the SPIR-V bytes; workaround is loading from disk at runtime.

**5c. GPU host wrap** (~7 weeks): 4w (Vulkan on Linux/Windows) + 3w (Metal on macOS). `womb/ui/render/gpu_dispatch.vuma` declares the `extern "C"` imports; the C host runtime provides `gpu_vulkan.c` and `gpu_metal.m`.

### Phase 6 — `womb/ui/ime/` + `womb/ui/a11y/` (parallel, WOMB team, ~10 + 10 weeks)

After Phase 3 (layout) and Phase 5 (renderer) land. Two parallel sub-streams:

**6a. `womb/ui/ime/`** (~10 weeks): `composition.vuma` (1w) + 3 platform bridges (6w total) + `caret.vuma` + `format.vuma` + `textfield.vuma` (3w). **V-11 dependency** — without V-11, the IME channel is a plain `Channel<UiEvent>` with runtime checks.

**6b. `womb/ui/a11y/`** (~10 weeks): `semantics.vuma` + `build.vuma` + `diff.vuma` + `reverse.vuma` + `preferences.vuma` (4w) + 3 platform bridges (6w total). Depends on Phase 3b (layout tree → semantics tree).

### Phase 7 — `womb/ui/animation.vuma` + `womb/ui/theme.vuma` + `womb/ui/{clipboard,filepick,net/capability}.vuma` (WOMB team, ~8 weeks)

After Phase 3 (layout) and Phase 5 (renderer) land:
- `womb/ui/animation.vuma` (2w, depends on `womb/lib/sys/{time,math}.vuma`).
- `womb/ui/theme.vuma` (2w, depends on `womb/lib/text/json.vuma`).
- `womb/ui/clipboard.vuma` (9 days, Wrap).
- `womb/ui/filepick.vuma` (9 days, Wrap).
- `womb/ui/net/{http,ws}_bridge.vuma` (2w combined, Wrap — depends on V-WOMB-1 fix from Phase 1 for native-target reuse of `womb/net/{http,websocket}.vuma`).
- `womb/ui/capability.vuma` + `cap_bundles.vuma` + `cap_delegate.vuma` + `cap_cache.vuma` (8w combined, Build VUMA, depends on V-16/ADR-0007 + V-09).

### Phase 8 — Integration + WOMB v1 release (WOMB team, ~4 weeks)

- Integrate all UI modules into `womb/ui/main.vuma` (the `ui_main_loop()` entry point).
- Run the full RFC test plans (each module's `*_test.vuma` + the gold-standard `tests/gold_standard/ui_*` entries).
- Run the kernel-smoke analogue: `scripts/ui_smoke.sh` that compiles `womb/ui/main.vuma` with `--verify`, runs the binary, greps for the "VUMA UI booted" banner.
- Update `scripts/test_womb_compile.sh` to follow imports transitively (catches future broken imports).
- Cut the WOMB v1 release tag.

### 8.1 Critical path

The critical path is Phase 0 (10w) → Phase 3b layout (12w) → Phase 5a render VUMA-side (3w) → Phase 5b shader (8w) → Phase 8 integration (4w) = **~37 weeks (~9 months) sequential**. With 5-person parallelism (Phase 3a/3b in parallel; Phase 4a/4b/4c in parallel; Phase 5a/5b/5c in parallel; Phase 6a/6b in parallel; Phase 7 sub-modules in parallel), the critical path drops to **~24 weeks (~6 months)** after Phase 0 lands.

Adding Phase 0 (10w), the total critical-path time to WOMB v1 is **~34 weeks (~8.5 months)** at 5-person parallelism.

---

## 9. Open questions

### 9.1 Should WOMB have its own test harness (`womb/ui/tests/`) or use the existing `tests/` directory?

**Context**: J-1 §6.4 recommends keeping the current split (per-file compile harness for syntax/IVE; per-module KAT tests under `tests/gold_standard/`; smoke tests under `scripts/`).

**Options**:
- **(A) Keep the current split** — every new `womb/ui/*.vuma` ships a matching `womb/ui/*_test.vuma` (or a `tests/gold_standard/ui_*` entry). The current per-file compile harness catches syntax/IVE errors; KAT tests under `tests/gold_standard/` catch algorithmic errors; smoke tests catch integration errors.
- **(B) Create `womb/ui/tests/`** — each WOMB module ships its own test file alongside the implementation.
- **(C) Fold WOMB tests into the existing `tests/` directory** — under `tests/gold_standard/ui/`.

**This draft's recommendation**: **(A) Keep the current split**, matching J-1 §6.4. The existing harness structure works for the kernel; it will work for the UI engine. Every new `womb/ui/*.vuma` file MUST ship a matching `womb/ui/*_test.vuma` (or a `tests/gold_standard/ui_*` entry) — this is a hard requirement, not a guideline. The existing `scripts/test_womb_compile.sh` harness must be extended to follow imports transitively (compile-import each module's transitive closure) — this is the CI hardening that ADR-0020 §implementation-step-4 calls for and is part of Phase 2.

**Confidence**: HIGH.

### 9.2 Should the C host runtime be one binary or per-platform?

**Context**: RFC-02 §architecture shows a single host binary per platform. File `24-rough-draft-vuma-only.md` proposes a ~500 LOC shim that pushes more logic into VUMA.

**Options**:
- **(A) One host binary per platform** (Linux/Windows/macOS) — current RFC-02 plan. Each binary is ~13 kLOC C (Linux) / ~12 kLOC (Windows) / ~11 kLOC Objective-C (macOS). SDL2 + Vulkan/Metal + libibus/libatspi/libcurl/libwebsockets/OpenSSL = ~7 MB host binary + ~3-5 MB VUMA binary = ~10-12 MB total.
- **(B) One universal host binary** with platform dispatch via `#ifdef` — rejected: macOS needs Objective-C for IMK/NSAccessibility/Cocoa; Windows needs Win32/COM; Linux needs D-Bus. A single source tree with platform-specific files is cleaner.
- **(C) ~500 LOC shim per file `24-rough-draft-vuma-only.md`** — pushes D-Bus / HTTP / WebSocket / clipboard logic into VUMA (using `womb/net/socket.vuma`, `womb/net/http.vuma`, etc.). Reduces the C shim to ~500 LOC wrapping only SDL2 + Vulkan/Metal. But (i) SDL2/Vulkan/Metal wrappers are irreducibly in C, (ii) libibus/libatspi are C libraries that VUMA would have to FFI to anyway, (iii) the WOMB team has higher-priority work than porting D-Bus to VUMA. v2 optimization.

**This draft's recommendation**: **(A) One host binary per platform** for v1, matching RFC-02. Revisit (C) for v2 once WOMB v1 ships and the team has bandwidth to port D-Bus / HTTP / WebSocket to VUMA.

**Confidence**: MEDIUM (file `24-rough-draft-vuma-only.md` P-10/P-11 explicitly recommends (C); the trade-off is v1 ship velocity vs. long-term VUMA purity).

### 9.3 How does WOMB handle HiDPI / DPR scaling?

**Context**: Per RFC-21 §hidpi-free and RFC-03 `EVENT_DPR_CHANGE`, the vector renderer renders at any DPR without re-rasterization. But the layout engine's `LayoutNode` coordinates are in CSS pixels (f32), and the GPU's framebuffer is in physical pixels (CSS × DPR).

**This draft's answer**: All layout coordinates are in CSS pixels (f32). The viewport uniform (`vk_bind_viewport_uniform(device, w, h, dpr)`) tells the compute shader the target resolution: the compute shader rasterizes at `w × dpr` × `h × dpr`, downsamples to `w × h` for display (or renders directly at `w × dpr` × `h × dpr` if the framebuffer is high-DPI). On `EVENT_DPR_CHANGE`, the host pushes a new event into the state ring; the UI engine calls `vk_bind_viewport_uniform` with the new DPR on the next frame. **No per-DPR atlas, no re-rasterization on DPR change. Just a uniform update.**

For fractional DPR (1.5×, 2.5× — common on Windows and some Linux Wayland compositors), the same scene renders crisp — the compute shader's analytic coverage computes each pixel's coverage exactly from the path geometry, regardless of fractional scaling.

**Confidence**: HIGH (matches RFC-21 §hidpi-free).

### 9.4 Should `womb/ui/render/` support software rendering fallback for headless testing?

**Context**: CI test runners often lack a GPU. WOMB UI tests need to verify rendering output without a real GPU.

**Options**:
- **(A) No software fallback** — CI uses a virtual GPU (SwiftShader on Linux, lavapipe, software Vulkan). The compute shader runs on the virtual GPU; output is compared against reference images.
- **(B) Software fallback in VUMA** — `womb/ui/render/swrender.vuma` implements the path tessellation in pure VUMA (CPU). ~5 kLOC, ~100× slower than GPU but correct. Used only in headless CI.
- **(C) Software fallback in C host** — the host runtime's `gpu_vulkan.c` detects "no GPU" and falls back to a CPU implementation.

**This draft's recommendation**: **(A) No software fallback; use SwiftShader / lavapipe** for headless CI. Software rasterization in VUMA would be ~5 kLOC of duplicate logic that has to track the compute shader's evolution — high maintenance cost. SwiftShader / lavapipe are well-tested, OS-provided, and free. The CI test plan calls for `vk_physical_device_emulation = software` environment variable to force the software path.

**Confidence**: MEDIUM (depends on SwiftShader / lavapipe availability on the CI runners; if they're not available, (B) becomes necessary).

### 9.5 Should `womb/sync/spsc.vuma` be the ONLY sync primitive WOMB uses, or should `womb/kernel/sync/{spinlock,mutex,rwlock,semaphore}.vuma` also be used?

**Context**: The browser target is single-threaded Wasm — no need for kernel sync primitives. The native target is multi-threaded (VUMA binary on a worker thread, host event loop on main thread) — but the two threads communicate only via the SPSC rings, not via shared state.

**This draft's answer**: For v1, `womb/sync/spsc.vuma` is the only sync primitive WOMB UI uses. The two rings (state + stream) are the entire cross-thread communication channel between the VUMA worker thread and the host main thread. No locks, no mutexes, no condition variables in VUMA-side WOMB code. The host runtime uses pthread mutexes + condition variables internally (for the side buffer, for blocking `ring_wait_event`), but those are C-side, not VUMA-side.

If W-9 capability verification needs cross-frame state (the per-frame capability cache), that's a single-threaded data structure (the VUMA binary is single-threaded for v1). The cache is invalidated at `cap_frame_begin()` and rebuilt lazily — no locks needed.

**Confidence**: HIGH for v1 (single-threaded VUMA binary). MEDIUM for v2 (if VUMA grows a multi-threaded execution model, e.g. for parallel layout / parallel shaping, kernel sync primitives come back into play).

### 9.6 Should the `womb/ui/capability.vuma` model use the existing `womb/crypto/mac_kdf/hmac.vuma`, or re-implement in Rust?

**Context**: J-1 §6.2 (ADR-0007 currently specifies use of `womb/crypto/mac_kdf/hmac.vuma` for the VUMA capability model — the cross-layer bootstrap dependency). V-16 / ADR-0007 migrates the Rust-side capability layer to HMAC-SHA-256. The question is whether the Rust-side capability layer calls into VUMA-compiled `hmac_sha256` (option A) or re-implements HMAC-SHA-256 in Rust (option B).

**Options**:
- **(A) `womb/crypto/mac_kdf/hmac.vuma` is canonical; Rust calls VUMA-compiled code.** Requires the VUMA compiler to invoke a VUMA-compiled transform from Rust codegen — the "self-compilation bootstrap."
- **(B) Re-implement HMAC-SHA-256 in Rust inside `capability.rs` (or sibling `hmac.rs`).** Adds ~150 LOC of Rust. Removes the self-compilation dependency.

**This draft's recommendation**: **(B) Re-implement in Rust** for the compile-time capability layer (per J-1 §6.2's recommendation). The capability layer is in the VUMA compiler (Rust), not in compiled VUMA binaries. The womb/crypto HMAC is for VUMA programs (like `womb/ui/capability.vuma`) to use at runtime; the Rust-side capability layer needs HMAC at compile time to sign capability tokens. The two layers are different consumers at different times. A 150-LOC Rust HMAC-SHA-256 is trivial, verifiable against KAT vectors, and doesn't require self-compilation.

**Note**: This contradicts ADR-0007 (which currently specifies A). The WOMB team should propose an ADR-0026 (or similar) that supersedes ADR-0007 on this specific point. The `womb/crypto/mac_kdf/hmac.vuma` is still canonical for runtime use by `womb/ui/capability.vuma` (W-9); only the compile-time Rust-side capability layer re-implements.

**Confidence**: MEDIUM (J-1 §6.2 says MEDIUM — "turns on self-compilation bootstrap decision").

### 9.7 Should `womb/kernel/` be kept, deleted, or split out?

**Context**: J-1 §6.5 (HIGH confidence: keep as-is). The `womb/kernel/` tree is 43 kLOC of VWK kernel that is NOT needed for the browser-target UI engine (the browser provides the kernel). But it represents the native-target story (post-v1) and the IrqRing pattern (which ADR-0019 Decision 2 generalizes into `womb/sync/spsc.vuma`).

**This draft's answer**: **Keep `womb/kernel/` as-is** (matching J-1 §6.5). The kernel is a substantial existing investment and represents the native-target story. The IrqRing pattern is the only piece the UI engine needs, and the generalization (ADR-0019 Decision 2) extracts it cleanly into `womb/sync/spsc.vuma`. Deleting the kernel would waste 43 kLOC of working code; splitting it out would break the `import "../crypto/..."` convention. The "do nothing" decision is correct.

**Confidence**: HIGH.

### 9.8 Should the duplicated SHA-256 implementations be reconciled?

**Context**: J-1 §6.7 (MEDIUM confidence: keep both for v1). `womb/crypto/hash/sha256_sha224.vuma` (1 525 LOC, stdlib) and `womb/kernel/crypto/sha.vuma` (611 LOC, kernel) both implement SHA-256. The kernel version is PMT-pure; the stdlib version uses `allocate()`/`free()`.

**This draft's answer**: **Keep both for v1** (matching J-1 §6.7). Reconciling them is a refactor that doesn't unblock anything for the UI engine. Defer to post-v1.

**Confidence**: MEDIUM.

---

## 10. References

### 10.1 ADRs (locked)

- **ADR-0013**: Three-layer architecture (VUMA / WOMB / VEEE). `docs/adr/ADR-0013.md`.
- **ADR-0019**: WOMB UI modules live in `womb/ui/`; IrqRing generalizes to `womb/sync/`. `docs/adr/ADR-0019.md`.
- **ADR-0020**: Fix broken `womb/net/*.vuma` imports (V-WOMB-1). `docs/adr/ADR-0020.md`.
- **ADR-0022**: Hand-written SPIR-V backend (supersedes ADR-0018's MLIR approach). `docs/adr/ADR-0022.md`.

### 10.2 ADRs (referenced)

- **ADR-0001**: V-34 fix (f32/f64 arms in `bridge_type_to_ir_type`). `docs/adr/ADR-0001.md`.
- **ADR-0002**: V-35 + V-44 fix (`type_size_from_name` + `type_alignment`). `docs/adr/ADR-0002.md`.
- **ADR-0003**: V-36 + V-A2-1 fix (`StateRead`/`StateWrite` IRType threading). `docs/adr/ADR-0003.md`.
- **ADR-0004**: V-03 + V-NEW-2 fix (`build_pmt_layout_specs` + IVE `rederive_layout`). `docs/adr/ADR-0004.md`.
- **ADR-0005**: V-40 fix (delete legacy `bridge_type_size`). `docs/adr/ADR-0005.md`.
- **ADR-0006**: V-14 (f32 PMT Lean proof deferred to v2; `__float_overflow_trap` stub). `docs/adr/ADR-0006.md`.
- **ADR-0007**: V-16 (HMAC-SHA-256 capability signatures). `docs/adr/ADR-0007.md`.
- **ADR-0009**: V-39 (re-run full test suite on `main` HEAD). `docs/adr/ADR-0009.md`.
- **ADR-0010**: 5-crate dependency policy. `docs/adr/ADR-0010.md`.
- **ADR-0021**: Delete dead `Effect` enum (resolves V-A3-7). `docs/adr/ADR-0021.md`.
- **ADR-0025**: V-13 (SIMD coverage extension, driven by text-shaper benchmarks). `docs/adr/ADR-0025.md`.

### 10.3 Audit / research notes

- **J-1**: WOMB layer audit. `docs/research/J-1-womb-layer.md`. (The authoritative inventory: 195 `.vuma` files, ~117 kLOC, 6 reusable artifacts, V-WOMB-1 broken imports, 9 ADRs assessed.)
- **K-1**: VEEE rename + three-layer design. `docs/research/K-1-veee-rename-design.md`.

### 10.4 VUMA-side references

- `docs/vuma-side-problem-catalog.md` — the catalog of VUMA-side bugs (V-34, V-35, V-36, V-03, V-46, V-26, V-11, V-16, V-A2-3, V-A2-4, V-WOMB-1, etc.).
- `docs/language-reference.md` §6-8 — builtins (memory/arena, PMT state, IPC/concurrency, capability/sandbox, verification), PMT model (Programs as Memory Transformations), FFI (`extern "C"`, `#[borrow]`, `#[secret]`).
- `docs/pmt-formal-spec.md` — the Lean formal spec for the PMT memory model (arena, capacity invariant, field-bounds safety, liveness, guard page, codegen trap contract, TCB).

### 10.5 SWE package RFCs (the WOMB design blueprint)

- `vuma-swe-package/16-build-vs-buy.md` — per-component decision matrix (Build / Port / Wrap) with effort estimates. The authoritative source for effort numbers in §3.2 and §4.
- `vuma-swe-package/02-rfc-host-runtime.md` — the C host runtime (~13 kLOC, SDL2/Vulkan/Metal/libibus/libatspi/libcurl/libwebsockets/OpenSSL). Note: superseded by file `24-rough-draft-vuma-only.md` for the ~500 LOC shim framing; the build-vs-buy matrix from file 16 still applies.
- `vuma-swe-package/03-rfc-event-pipeline.md` — W-0 event pipeline (two-ring design, UiEvent layout, host_yield, hit-testing).
- `vuma-swe-package/04-rfc-layout-engine.md` — W-1 layout engine (Flexbox, f32, stacking, abs, vertical, Knuth-Plass, composited scroll, animation, dirty tracking).
- `vuma-swe-package/06-rfc-font-parser.md` — W-3 font parser (OpenType tables, cmap formats 0/4/6/12/13/14, glyf/loca, fvar/gvar, COLR/CPAL, vmtx/vhea, hinting, subsetting, TTC, font fallback chain).
- `vuma-swe-package/07-rfc-text-shaper-v1.md` — W-3 shaper v1 (cmap + hmtx + variable deltas, f32 advances, UTF-8↔UTF-32).
- `vuma-swe-package/08-rfc-text-shaper-v2-v3.md` — W-3 shaper v2/v3 (GSUB + GPOS, port from rustybuzz).
- `vuma-swe-package/09-rfc-bidi.md` — W-3 BiDi (UAX #9, port from unicode-bidi; UAX #14 line breaking; UAX #29 grapheme/word).
- `vuma-swe-package/10-rfc-ime-bridge.md` — W-5 IME bridge (ImeState, host imports, IBus/IMM32/IMK, session-typed channel).
- `vuma-swe-package/11-rfc-a11y-bridge.md` — W-6 a11y bridge (SemanticsNode, diff, AT-SPI/UIA/NSA11y, multi-theme, reduced motion).
- `vuma-swe-package/12-rfc-clipboard-file-net.md` — W-7 clipboard/file picker/network bridges.
- `vuma-swe-package/13-rfc-capability-model-ui.md` — W-9 UI capability tokens (HMAC-SHA-256 signing, capability bundles, delegation/revocation, per-frame cache).
- `vuma-swe-package/21-rfc-vector-engine.md` — W-2 vector renderer (supersedes `05-rfc-renderer.md`; scene tree = paths, GPU compute shader tessellation, no glyph atlas, no SDF, no per-DPR atlas).
- `vuma-swe-package/24-rough-draft-vuma-only.md` — the ~500 LOC C shim framing (v2 target; supersedes file 02's ~13 kLOC framing for the long term).
- `vuma-swe-package/26-new-plans-three-layers.md` — the original three-layer plan (VUMA / WOMB / VEEE), spiritual ancestor of ADR-0013.

### 10.6 Source files referenced

- `womb/kernel/trap/irq_ring.vuma` (472 LOC) — the SPSC ring substrate for `womb/sync/spsc.vuma` (ADR-0019 Decision 2).
- `womb/kernel/arch/wasm32/sched_hal.vuma` (585 LOC) — the `host_yield()` pattern for `womb/ui/event/yield.vuma`.
- `womb/crypto/mac_kdf/hmac.vuma` (193 LOC) — RFC 2104 HMAC-SHA-256 for `womb/ui/capability.vuma` (W-9).
- `womb/lib/text/unicode.vuma` (709 LOC) — RFC 3629 UTF-8 for `womb/ui/text/utf8.vuma`.
- `womb/lib/text/json.vuma` (1 254 LOC) — RFC 8259 JSON for `womb/ui/theme.vuma` and IPC.
- `womb/lib/sys/{time,math}.vuma` (756 LOC combined) — animation + layout math primitives.
- `womb/collections/{vec,hashmap,btree_map}.vuma` (676 LOC) — foundational data structures.
- `womb/graph/{digraph,algorithms}.vuma` (363 LOC) — DAG + topological sort for layout trees + dirty propagation.
- `womb/lib/mem_helpers.vuma` (60 LOC) — single source of truth for `store_u*`/`load_u*` byte-stitched little-endian helpers.
- `womb/net/{http,websocket}.vuma` (944 + 506 LOC) — RFC-grade HTTP/WebSocket for native target (after V-WOMB-1 fix).
- `src/parser/src/resolver.rs:500-510` — the VUMA module resolver (`base_dir.join(path)`, no fallback).
- `src/pipeline.rs:6506-6516` — the buggy `bridge_type_to_ir_type` arm (V-34, ADR-0001).
- `src/parser/src/to_scg.rs:4057-4065` — the buggy `type_size_from_name` (V-35, ADR-0002).

### 10.7 External references

- CSS Flexbox spec: <https://www.w3.org/TR/css-flexbox-1/>
- CSS Stacking Contexts: <https://developer.mozilla.org/en-US/docs/Web/CSS/CSS_Positioning/Understanding_z_index/The_stacking_context>
- Knuth & Plass 1981, "Breaking Paragraphs into Lines"
- CSS `writing-mode`: <https://developer.mozilla.org/en-US/docs/Web/CSS/writing-mode>
- OpenType spec: <https://learn.microsoft.com/en-us/typography/opentype/spec/>
- OpenType Variable Fonts: <https://learn.microsoft.com/en-us/typography/opentype/spec/otvaroverview>
- OpenType COLR: <https://learn.microsoft.com/en-us/typography/opentype/spec/colr>
- TrueType hinting: <https://learn.microsoft.com/en-us/typography/opentype/spec/tt_instructions>
- FreeType `ttinterp.c`: <https://gitlab.freedesktop.org/freetype/freetype/-/blob/master/src/truetype/ttinterp.c>
- `ttf-parser` (font-parser reference): <https://github.com/RazrFalcon/ttf-parser>
- `rustybuzz` (shaper v2 reference): <https://github.com/RazrFalcon/rustybuzz>
- `unicode-bidi` (BiDi reference): <https://github.com/servo/unicode-bidi>
- **Vello** (vector renderer reference): <https://github.com/linebender/vello>
- Loop-Blinn 2005, "Resolution Independent Curve Rendering Using Programmable Graphics Hardware"
- Vulkan spec: <https://registry.khronos.org/vulkan/specs/1.3/html/index.html>
- Metal spec: <https://developer.apple.com/documentation/metal>
- Vulkan compute shaders: <https://registry.khronos.org/vulkan/specs/1.3/html/chap32.html>
- Metal compute: <https://developer.apple.com/documentation/metal/compute_passes>
- glslangValidator: <https://github.com/KhronosGroup/glslang>
- SDL2: <https://www.libsdl.org/>
- OpenSSL HMAC: <https://www.openssl.org/docs/manmaster/man3/HMAC.html>
- IBus: <https://github.com/ibus/ibus>
- IMM32: <https://learn.microsoft.com/en-us/windows/win32/intl/input-method-manager>
- IMK: <https://developer.apple.com/documentation/inputmethodkit>
- AT-SPI: <https://gitlab.gnome.org/GNOME/at-spi2-core>
- UIAutomation: <https://learn.microsoft.com/en-us/windows/win32/winauto/ui-automation-spec>
- NSAccessibility: <https://developer.apple.com/documentation/appkit/nsaccessibility>
- libcurl: <https://curl.se/libcurl/>
- libwebsockets: <https://libwebsockets.org/>
- XDG Desktop Portal: <https://flatpak.github.io/xdg-desktop-portal/>
- C11 `<stdatomic.h>`: <https://en.cppreference.com/w/c/atomic>
- Linux `futex(2)`: <https://man7.org/linux/man-pages/man2/futex.2.html>
- iOS Scroll View Programming Guide: <https://developer.apple.com/library/archive/documentation/WindowsViews/Conceptual/UIScrollView_Guide/>
