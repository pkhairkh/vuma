# VUMA Releases

This document summarizes each VUMA release with key changes and known limitations.

---

## Current State — 2026-03-07 (post-v0.2.0-alpha.1 hardening, Waves 5-28)

**Honest snapshot of what works and what doesn't, after the W1-W28 hardening
waves that followed the v0.2.0-alpha.1 scientific-integrity release.**

This is not a new version tag — `Cargo.toml` remains at `0.2.0-alpha.1`. It
is a status snapshot documenting the real state of the framework so that
users, contributors, and downstream LLM agents can plan against accurate
claims rather than the v0.2.0-alpha.1 release notes (which described only
the scientific-integrity pass and carried forward older "deferred to Phase 3"
limitations that have since been resolved).

### What works (verified by tests)

- **Verifier works on real programs.** `examples/hello_memory.vuma` and
  `examples/doubly_linked_list.vuma` both pass **all 5 VUMA invariants**
  (Liveness, Exclusivity, Interpretation, Origin, Cleanup) end-to-end
  through the full parse → SCG → IVE pipeline. Locked in by
  `showcase_hello_memory` and `showcase_doubly_linked_list` in
  `src/tests/src/showcase_verification.rs`.
  - The W9-10 Exclusivity false positive on `doubly_linked_list.vuma`'s
    `link(prev, next)` sequential-write pattern
    (`(*last).next = node; (*sentinel).prev = node;` flagged as a
    write-write conflict) was fixed in W17-18 by chaining Access nodes
    into the ControlFlow sequence in `src/parser/src/to_scg.rs`.
  - The earlier Liveness/Cleanup false positives on top-level `region`
    declarations were fixed in W1-W4 + G4 (3-tier `free(var)` tracking;
    skip top-level region allocations).
- **Return-value propagation works** on x86_64 (native) and AArch64 (via
  QEMU). `fn main() -> i32 { return 42; }` exits 42 on both architectures
  (Test 17 in `cross_backend.rs`); `fn main() -> i32 { return 79; }`
  (SHA256d's "proof verified" convention, 0x4F = first byte of
  SHA256d("abc")) exits 79 on AArch64 via QEMU (W24). The W5-6 fix
  removed the AST → SCG return-value drop by eliminating the duplicate
  FunctionReturn node and restoring the `lit_N → FunctionReturn`
  DataFlow edge.
- **Verification is blocking and enforced.** `compile()` returns
  `Err(Vec<VumaError>)` and emits no binary when verification fails.
  Pinned by `test_e2e_leak_fails_compilation` in
  `src/tests/src/showcase_verification.rs`.
- **3700+ tests pass** across the workspace (`cargo test --workspace`).
  Per-crate breakdowns: `vuma` core 140+, `vuma-tests` 486+,
  `vuma-parser` 36+, plus `vuma-scg`, `vuma-ive`, `vuma-bd`,
  `vuma-codegen`, `vuma-proof`, `vuma-std`, `vuma-cor`,
  `vuma-projection`, `vuma-package`.
- **8 backend architectures** — x86_64 and AArch64 both have real
  end-to-end execution tests through the full parse → SCG → IR →
  regalloc → encode pipeline. RISC-V 64, ARM32, MIPS64, PPC64,
  LoongArch64 pass ELF header + IR/SCG validation. Wasm32 generates
  valid modules. Cross-architecture execution via QEMU is gated on
  `qemu-<arch>-static` being installed.

### What doesn't work yet (documented honestly)

- **`sha256d.vuma` does not compile end-to-end.** The SCG → MSG converter
  (`vuma_core::scg_to_msg::scg_to_msg`) refuses to convert SCGs that
  contain back-edges from loops — `sha256d.vuma` has `while i < 64` over
  the 64 SHA-256 rounds and `while j < len` over message blocks, both of
  which the parser-built SCG represents with control-flow cycles. This
  blocks MSG construction, BD inference, IVE verification feeding off the
  MSG, and COR init. The benchmark suite falls back to `fibonacci.vuma`
  when `sha256d.vuma` does not lower. Two diagnostic tests
  (`test_sha256d_real_program_compiles`,
  `test_sha256d_real_program_executes`) surface the failure rather than
  masking it. **SHA256d real-program compilation is the next major
  milestone** (M3.11 in `docs/ROADMAP.md`). The fix is either (a) unroll
  bounded loops in the parser → SCG bridge, or (b) teach the MSG builder
  to walk cyclic SCGs by treating loop back-edges as revisits.
- **Proof system is a sketch, not mechanized.** The `proof` crate
  produces paper proofs (proof sketches) checked by a syntactic checker.
  It is **not** mechanized in Coq, Isabelle, or Lean. A `Proven` verdict
  from the IVE means the invariant verifiers found no violations, not
  that a proof assistant has certified the program. Full mechanization
  is future work (Phase 5+).
- **Concurrent verification is limited to single-threaded programs.**
  LDXR/STXR, locks, channels, and happens-before analysis are Phase 3
  targets (M3.1, M3.2) and not yet implemented.
- **Self-hosting is not started.** VUMA cannot compile itself; the
  compiler is written in Rust.
- **No user-defined structs or enums.** Programs operate on primitives,
  pointers, and regions.
- **Standard library is host-side only.** Math, fmt, string, and crypto
  functions execute on the host (Rust); they are not yet compiled to
  target machine code.
- **BD inference completeness (M2.3) is deferred.** Some complex BD
  inference scenarios remain unfinished — this is the only Phase 2
  milestone still deferred (M2.4 doubly-linked list verification is now
  ✅ Complete as of W17-18).
- **COR is not integrated end-to-end.** The Continuous Optimization
  Runtime has ProfileCollector, SpeculativeExecutor, OptimizationEngine,
  and DeploymentManager components, but the end-to-end optimization loop
  is not wired into the default compilation pipeline.
- **LoongArch64 execution is not validated.** No `qemu-loongarch64-static`
  path is wired up; the LoongArch64 backend passes ELF header + IR/SCG
  validation only.
- **Pi 5 bare-metal backend is not shipped.** The `pi5` crate and
  `src/pi5/link.ld` linker script are absent from the tree.

### Test count

`cargo test --workspace` reports **3700+ tests passing** across all 12
workspace crates. The largest single contributors are `vuma-tests`
(integration tests, 486+ tests covering cross-backend, ABI, ELF/Wasm
validation, DWARF/FFI, full-pipeline, SHA256d, property-based, and
benchmark categories) and `vuma` core (140+ tests covering pipeline,
API, and unit-level SCG/MSG/IVE/BD integration).

### Files touched in W29-30 (this snapshot)

- `README.md` — rewrote "Known Limitations" into two tables
  ("Verification — Current Honest State" and "Other Limitations"); added
  "Verifier Now Works on Real Programs (post-alpha.1 hardening, Waves
  5-28)" subsection under "What's New in v0.2.0-alpha.1"; removed the
  stale W11-12 "Known front-end limitation" claim about return-value
  dropping (fixed by W5-6); updated the 8-backend section to reflect
  that x86_64 and AArch64 both have full-pipeline end-to-end execution
  tests; added a Worklog entry.
- `docs/ROADMAP.md` — updated Phase 2 status to "10/11 milestones
  achieved" with M2.4 (doubly-linked list verified) now ✅ Complete;
  updated Phase 2 success criteria to mark the dlist checkbox `[x]`;
  added a "Real-Program Verifier Progress (W1-W28 hardening)" subsection
  to Phase 3; added three new Phase 3 milestones (M3.9 real-program
  verifier, M3.10 return-value propagation, M3.11 SHA256d real-program
  compilation); added new Phase 3 success criteria; updated the
  dependency graph and risk-mitigation table; updated the Success
  Criteria Summary table.
- `RELEASES.md` — added this "Current State — 2026-03-07" section at
  the top.

### Next actions

1. **Fix SCG cycle handling** (M3.11) — unblock SHA256d real-program
   compilation. This is the single highest-leverage task in the project
   right now.
2. **Mechanize the proof system** in Coq, Isabelle, or Lean (long-term,
   Phase 5+ track).
3. **Implement concurrent verification** (M3.1, M3.2) — LDXR/STXR,
   happens-before analysis, deadlock detection.
4. **Add user-defined structs and enums** — currently a hard front-end
   limitation.
5. **Wire COR end-to-end** into the default compilation pipeline.

---

## v0.2.0-alpha.1 — 2026-03-07

**Scientific Integrity, Provenance, and Versioning Correction**

This pre-release is a scientific-integrity pass over the project: it adds
explicit AI-authorship disclosure, corrects non-monotonic version history, and
bumps the version to reflect the substantial work done since the original
`0.1.0` foundation release.

### Why the version bump?

The project had accumulated ~266k LOC across 8 backend architectures, 32
engineering waves, an FFI/syscall layer, a package manager, LLM integration,
and ~266 tests — yet the `Cargo.toml` version was still pinned at
`0.1.0-alpha.1`. That number was artificially low and gave a misleading
impression of project maturity. The bump to `0.2.0-alpha.1` reflects the
minor-version worth of work done since `0.1.0`, while keeping the `-alpha.1`
pre-release tag to honestly signal that the API is not yet stabilized.

### Highlights — Scientific Integrity Improvements

- **Authorship Disclosure**: prominent new section in `README.md`
  (immediately after the title and badges) explicitly acknowledging that the
  project was developed primarily through AI-assisted sessions using
  [GLM-5.1](https://z.ai) and other AI coding agents, with human oversight
  throughout. The `authors` field in `Cargo.toml` (`["Super Z (GLM-5.1)"]`)
  is now consistent with the documentation.
- **CONTRIBUTING.md**: added a note requesting that PRs authored or
  substantially assisted by AI tools disclose this in the PR description.
- **CHANGELOG.md**: added an authorship blockquote at the top, a new
  `0.2.0-alpha.1` entry, and a *Versioning Note* explaining the historical
  `0.1.0` → `0.1.0-alpha.X` → `0.2.0-alpha.1` progression. Same-day entries
  (`0.1.0` and `0.1.0-alpha.0`, both 2026-03-05) are now explicitly annotated
  as *earlier* (initial public release) and *later* (Phase 2 pre-release) to
  remove the previous ambiguity.
- **architecture.md**: updated the `Authors` field from "VUMA Project Team" to
  an explicit AI-authorship statement, and bumped the document version to
  `0.2.0-alpha.1` to match `Cargo.toml`.
- **Cargo.toml**: bumped `version` in both `[package]` and
  `[workspace.package]` from `0.1.0-alpha.1` to `0.2.0-alpha.1`.

### What did *not* change in this release

No source code (`src/`) was modified in this pass — only documentation,
metadata, and versioning files. The 8 backends, verification pipeline, parser,
FFI, package manager, and standard library are unchanged from
`0.1.0-alpha.1`. The next pre-release (`0.2.0-alpha.2`) will resume
functional work.

### Known Limitations

Carried forward unchanged from `v0.1.0-alpha.1`:
- BD inference completeness (M2.3) deferred to Phase 3
- Doubly-linked list verification (M2.4) deferred to Phase 3
- ARM64 atomics and concurrent verification are Phase 3 targets
- COR end-to-end integration not yet complete
- LoongArch64 full SHA256d too slow for QEMU (should work natively)

---

## v0.1.0-alpha.1 — 2026-03-06

**Alpha Pre-Release: Critical Bug Fixes, Atomics, FP Conversions, Test Hardening**

Pre-release alpha incorporating Waves 1–5 of critical bug fixes, atomics/ABI support, FP conversion casts, infrastructure/stdlib expansion, and test hardening across all 8 backends.

### Highlights

- **ARM64 ROR/ROL fix**: Rotation instructions now correctly emit `EXTR`/`RORV`
- **6-arch atomics**: LoongArch64 (LL.D/SC.D), PPC64 (LDARX/STDCX), RISC-V 64, Wasm32 (24 new atomic ops), ARM32 (LDREX/STREX), MIPS64 (LLD/SCD)
- **FP conversion casts**: Type-aware `IRInstr::Cast` with `from_ty`/`to_ty` across all 8 backends
- **LoongArch64 terminators**: Fixed Switch/Invoke/TailCall/Resume lowering
- **ARM32 AAPCS**: Proper >4 argument passing via the stack
- **Standard library expansion**: `math.rs` (92 items), `fmt.rs` (13 functions)
- **DWARF debug info**: Full pipeline integration with per-backend CIE presets
- **C FFI wiring**: ExternRegistry, SyscallTable, RelocationKind
- **~266 new/expanded tests** across SHA256d, regression, diagnostics, DWARF/FFI, ABI, property-based, math, and fmt categories
- **CI/CD**: GitHub Actions workflow, .gitignore, repo cleanup

### Known Limitations

- BD inference completeness (M2.3) deferred to Phase 3
- Doubly-linked list verification (M2.4) deferred to Phase 3
- ARM64 atomics and concurrent verification are Phase 3 targets
- COR end-to-end integration not yet complete
- LoongArch64 full SHA256d too slow for QEMU (should work natively)

---

## v0.1.0-alpha.0 — 2026-03-05 (later — Phase 2 pre-release)

**Phase 2: Multi-Architecture Codegen, LLM Integration, Wasm Sandbox**

> **Note**: This is a pre-release (alpha) version. The API is not yet stabilized and may change before v0.1.0.

This release represents the substantial completion of Phase 2 of the VUMA framework. The project now supports 8 backend architectures, provides comprehensive LLM integration, and includes a Wasm32 sandbox for safe LLM-generated code execution.

### Highlights

- **8 Backend Architectures**: x86_64, AArch64, RISC-V 64, ARM32, MIPS64, PPC64, LoongArch64, Wasm32 — all passing SHA256d or individual operation tests
- **LLM Integration**: VumaForLLM API, VumaCompiler API, LSP server, enhanced REPL (`:wasm`, `:backends`, `:check`, `:diagnostics`, `:exports`), parser hardened for LLM-generated code
- **Wasm32 Sandbox**: LLM agents compile to safe, sandboxed WebAssembly modules
- **Memory Safety**: 10 violation types (E041–E050), compile-time checks, runtime bounds checking (`--safe`)
- **Constant-Time Crypto**: Branchless `ct_select`, `ct_eq`, `ct_ne`, `ct_lt`, `ct_gte` across all 8 backends
- **Module System**: Multi-file compilation with `import "path"::{names};` and circular import detection
- **Package Manager**: Foundation with `vuma pkg init/build/add` and dependency resolution
- **FFI & Syscalls**: 19 Linux syscalls across 8 architectures, architecture-specific relocations
- **Register Allocator**: Loop-aware spill weights, GreedyRegCache, dead-vreg reuse
- **Diagnostics**: 65 diagnostic codes (E001–E050, W001–W010, I001–I005), error chaining, structured suggestions, 4 output formats (JSON, ANSI, plain text, LSP)
- **CI**: GitHub Actions with cross-compile matrix for all 8 ISA targets

### New Features (since initial alpha)

#### Multi-Architecture Codegen
- All 6 native backends pass SHA256d: x86_64, AArch64, RISC-V 64, ARM32, MIPS64, PPC64
- LoongArch64 deep audit: fixed 7 encoding/ABI bugs, added maskeqz/masknez instructions
- PPC64 deep audit: fixed LR save offset, CMP l-field, RLDCL/RLDCR opcode, mb5/me5
- Wasm32 generates valid modules with 12-section validation
- Cross-backend consistency tests (9 tests × 8 backends)
- ELF validation for all 7 native backends (ELF32/64, endianness, machine types)
- ABI conformance testing (27 tests covering calling conventions)
- ELF emission: 3 LOAD segments (W^X), per-arch section alignment

#### LLM Integration
- `VumaForLLM`: `compile()`, `check()`, `analyze()`, `to_wasm()`, `explain_error()`, `suggest_fixes()`, `targets()`
- `VumaCompiler`: `compile()`, `parse()`, `analyze()`, `validate()`, `verify()`
- LSP server: diagnostics, hover, go-to-definition, completion, semantic tokens
- REPL: `:wasm`, `:backends`, `:check`, `:diagnostics`, `:exports`, tab completion, ANSI color
- Parser hardening: LLM type aliases (int→i32), macro detection (println!), C-style for loop, &T→*T

#### Verification & Safety
- `VumaCompiler.verify()` with per-invariant pass/fail and counterexamples
- Property-based testing (15 proptests across 6 categories)
- Memory safety analyzer: 10 violation types, compile-time and runtime checks
- 65 diagnostic codes with error chaining and structured suggestions
- Interprocedural analysis, escape analysis, verification cache

#### Standard Library
- `crypto.rs`: SHA-256 constants, host-side helpers, constant-time operations
- `string.rs`: strlen, strcmp, memcpy, memset
- `math.rs`: abs, min, max, clamp
- `alloc.rs`: heap_alloc, heap_free, heap_realloc
- `io.rs`: read_bytes, write_bytes, read_u32_le, write_u32_le

#### Infrastructure
- GitHub Actions CI with 8-target cross-compile matrix
- DWARF debug info with per-backend configuration
- `--debug-info`, `--safe`, `--bench`, `--sections` CLI flags
- Package manager: `vuma pkg init/build/add`
- Module system: `import "path"::{names};`

### Known Limitations

- BD inference completeness (M2.3) deferred to Phase 3
- Doubly-linked list verification (M2.4) deferred to Phase 3
- ARM64 atomics and concurrent verification are Phase 3 targets
- COR end-to-end integration not yet complete
- LoongArch64 full SHA256d too slow for QEMU (should work natively)

---

## v0.1.0 — 2026-03-05 (earlier — initial public release)

**Phase 1: Foundation**

Initial release of the VUMA framework. This release establishes the complete architectural foundation: all 12 workspace crates, 15 formal specifications, 10 example programs, a comprehensive benchmark suite, and full documentation.

### Highlights

- **12 Workspace Crates**: scg, bd, vuma, ive, cor, projection, parser, codegen, proof, std, tests, package
- **5 VUMA Invariants**: Liveness, Exclusivity, Interpretation, Origin, Cleanup — all verifiable end-to-end
- **8 Backend Architectures**: x86_64, AArch64, RISC-V 64, ARM32, MIPS64, PPC64, LoongArch64, Wasm32
- **15 Formal Specifications**: SCG, RepD, CapD, RelD, invariants, MSG, codegen, benchmarks, proofs, decidability
- **Full Verification Pipeline**: SCG → MSG → IVE verification with counterexample generation
- **Proof System**: Proof sketch with checker, tactics, counterexample generation (not mechanized in a proof assistant)
- **Standard Library**: Ptr, RegionPtr, Slice, Vec, HashMap, VumaString, Mutex, RwLock, Channel
- **10 Example Programs**: hello_memory, doubly_linked_list, arena_allocator, gpio_blink, lock_free_queue, etc.

### Known Limitations

- Concurrent verification limited to single-threaded programs
- ARM64 codegen does not support atomic instructions
- COR not yet integrated end-to-end
- Parser has known type mismatches in AST→SCG lowering
- LoongArch64 and Wasm32 backends need further hardening
