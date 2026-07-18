# src/ — VUMA Compiler Source

The `src/` tree is the VUMA compiler itself: a Cargo workspace of **10
internal library crates** (~329K LOC of Rust) plus the root binary crate.
It parses `.vuma` source, builds the Semantic Computation Graph (SCG), runs
the IVE state verifiers, lowers to IR, runs the O2 optimizer pipeline
(e-graph equality saturation → instruction scheduler → LICM → escape/SROA
→ vectorize → loop unroll), and emits machine code for **19 backends**.

This README is the entry point for the compiler source. For the build
reference see [`docs/building.md`](../docs/building.md). For the
contribution workflow (code style, adding a backend, adding a test) see
[`docs/contributing.md`](../docs/contributing.md). For the compiler
architecture (PMT pipeline, state type system, the 3 state verifiers,
Behavioral Descriptors, e-graph layout optimization, dependent state
types, FFI 4-mode marshal matrix) see [`docs/architecture.md`](../docs/architecture.md).

---

## The 10 workspace crates

| Crate (`src/...`) | Package | Purpose | LOC | Key modules |
|-------------------|---------|---------|-----|-------------|
| `src/parser/` | `vuma-parser` | Frontend: lexer, recursive-descent parser, typed AST, AST→SCG lowering, error recovery, module resolver | ~21K | `lexer.rs`, `parser.rs`, `ast.rs`, `to_scg.rs`, `resolver.rs`, `error.rs` |
| `src/scg/` | `vuma-scg` | Semantic Computation Graph — the formal graph IR; nodes: StateInit, StateRead, StateWrite, StateTransform, BinOp, etc. | ~22K | `graph.rs`, `node.rs`, `transform.rs`, `callgraph.rs`, `dominance.rs`, `loop_detection.rs`, `region.rs`, `liveness.rs`, `query.rs`, `serialize.rs` |
| `src/bd/` | `vuma-bd` | Behavioral Descriptors — RepD (representation), CapD (capability), RelD (relation) lattices + inference | ~16K | `descriptor.rs`, `repd.rs`, `capd.rs`, `reld.rs`, `inference.rs`, `context.rs`, `manifold.rs`, `unify.rs`, `serialize.rs` |
| `src/ive/` | `vuma-ive` | Inference & Verification Engine — the 3 PMT state verifiers (StateRead, StateWrite, StateTransform) + FFI marshal verifier | ~19K | `state_read.rs`, `state_write.rs`, `state_transform.rs`, `ffi.rs`, `borrow_region.rs`, `escape.rs`, `liveness.rs`, `exclusivity.rs`, `origin.rs`, `interpretation.rs`, `interprocedural.rs`, `arena_bounds.rs`, `verification.rs` |
| `src/codegen/` | `vuma-codegen` | IR lowering, register allocator, instruction scheduler, O2 optimizer pipeline, 19-architecture backends, runtime | ~156K | `ir.rs`, `scg_to_ir.rs`, `monomorphize.rs`, `closures.rs`, `egraph.rs`, `opt.rs`, `scheduler.rs`, `regalloc.rs`, `regalloc_emit.rs`, `backend.rs`, `marshal.rs`, `escape_analysis.rs`, `vectorize.rs`, `loop_unroll.rs`, `bv_verify.rs`, `alias_analysis.rs`, `syscall_abi.rs`, `target_desc.rs`, `emit.rs`, `dwarf.rs`, `effects.rs`, `memory_safety.rs`, `proof_artifacts.rs`, `control_flow.rs`, `runtime/`, 19 `*/mod.rs` backend modules |
| `src/proof/` | `vuma-proof` | Formal proof system — proof checker, tactics, counterexamples, liveness/exclusivity/origin/interpretation proofs | ~11K | `proof.rs`, `checker.rs`, `tactics.rs`, `counterexample.rs`, `judgment.rs`, `rules.rs`, `models.rs`, `liveness_proofs.rs`, `exclusivity_proofs.rs`, `origin_proofs.rs`, `interpretation_proofs.rs`, `composition.rs`, `serialization.rs` |
| `src/cor/` | `vuma-cor` | Continuous Optimization Runtime — JIT, profiling, speculation, deployment | ~11K | `runtime.rs`, `optimization.rs`, `speculative.rs`, `profile.rs`, `bridge.rs`, `config.rs`, `deployment.rs`, `ownership.rs`, `types.rs` |
| `src/vuma/` | `vuma-core` | Memory model, MSG (Memory State Graph) construction, invariant checking, security, REPL, address analysis | ~15K | `msg.rs`, `msg_builder.rs`, `msg_incremental.rs`, `scg_to_msg.rs`, `access.rs`, `access_analysis.rs`, `address.rs`, `derivation.rs`, `program_point.rs`, `region.rs`, `security.rs`, `sync.rs`, `repl.rs`, `pipeline.rs` (the top-level `compile*` entry points) |
| `src/package/` | `vuma-package` | Package manager — manifest parser (TOML-lite), dependency resolver, registry | ~2K | `manifest.rs`, `resolver.rs`, `registry.rs`, `toml_lite.rs` |
| `src/tests/` | `vuma-tests` | Integration test framework — full-pipeline tests, cross-backend tests, ABI conformance, ELF validation, execution validation, property tests, regression suite | ~31K | `framework.rs`, `full_pipeline.rs`, `cross_backend.rs`, `codegen.rs`, `abi_conformance.rs`, `elf_validation.rs`, `execution_validation.rs`, `ffi_types.rs`, `property_tests.rs`, `regression.rs`, `sha256d.rs`, `sha256d_backends.rs`, `parser_roundtrip.rs`, `bd_inference.rs`, `dwarf_ffi_integration.rs`, `wasm_validation.rs`, `concurrent.rs`, `dlist.rs`, `graph.rs`, `e2e_cor.rs`, `final_integration.rs`, `diagnostics_integration.rs`, `wave47_bootstrap.rs`, `wave48_bootstrap.rs`, `wave48_self_host.rs`, `wave50.rs`, `trivial.rs` |
| (root) | `vuma` | CLI binary (`src/main.rs`) + `compile_dump` / `dump_ir` / `dump_codegen_scg` / `scg_dump` / `parse_test` drivers in `src/bin/` + `pipeline.rs`, `lib.rs`, `api.rs`, `diagnostics.rs`, `ffi.rs`, `json_value.rs`, `llm_api.rs`, `logging.rs`, `lsp/`, `telemetry.rs`, `time.rs` | — | |

All path dependencies are declared in the root [`Cargo.toml`](../Cargo.toml)
under `[workspace.dependencies]`. There are no external crates — only `std`
and the internal `vuma-*` path crates.

---

## The compilation pipeline

```
   .vuma source
        │
        ▼
   parser (lexer, recursive descent, typed AST)
   src/parser/{lexer,parser,ast,to_scg,resolver}.rs
        │
        ▼
   SCG — Semantic Computation Graph
   src/scg/{graph,node,transform,callgraph,dominance,...}.rs
   nodes: StateInit, StateRead, StateWrite, StateTransform, BinOp, …
        │
        ▼
   IVE — Invariant Verification Engine (VerificationLevel::Pmt)
   src/ive/{state_read,state_write,state_transform,ffi,borrow_region}.rs
   3 state verifiers + FFI marshal verifier
        │
        ▼
   IR lowering (monomorphize, closures, bv_verify)
   src/codegen/{scg_to_ir,monomorphize,closures,bv_verify}.rs
        │
        ▼
   O2 optimizer pipeline
   src/codegen/{egraph,opt,scheduler,escape_analysis,vectorize,loop_unroll}.rs
   e-graph equality saturation → scheduler → LICM →
   cross-function const prop → escape/SROA → vectorize → loop_unroll
        │
        ▼
   register allocation
   src/codegen/{regalloc,regalloc_emit}.rs
        │
        ▼
   19 backends (isel + register allocation + ELF emission)
   src/codegen/{x86_64,aarch64,riscv64,...,wasm32}/mod.rs
        │
        ▼
   .bin (ELF for the target arch, or wasm32 module)
```

The pipeline is monolithic in source order but each stage is independently
testable. The scheduler models memory dependencies via cast-aware
type-based alias analysis (TBAA) with IVE-proven non-aliasing overrides.
The e-graph feeds both binop algebraic rules (35 rules) and state-op
rewrites (`state_transform_elision` — a transform whose src layout equals
its dst layout is rewritten to its input).

The top-level entry points live in `src/vuma/src/pipeline.rs`:
`compile`, `compile_with_path`, `compile_modules`,
`run_scg_transforms`, `run_ir_pipeline`, `bridge_ast_to_codegen_scg`,
`build_pmt_layout_specs`, plus `CompileConfig`, `CompileTarget`,
`OptLevel`, `VerificationLevel`, `CompileResult`.

---

## The 19 codegen backends

All 19 backends emit real machine code (or wasm) — no interpreter stubs.
The backend dispatch lives in
[`src/codegen/src/backend.rs::BackendKind`](codegen/src/backend.rs).

| Backend | Architecture | Source | Executable? | Notes |
|---------|--------------|--------|-------------|-------|
| `X86_64` | AMD64 (Intel/AMD) | `x86_64/mod.rs` + `x86_64/disasm.rs` + `x86_64/stack_slot_isel.rs` | native | Default host backend on x86_64 Linux; has hosted-mode syscall stubs |
| `AArch64` | ARMv8 64-bit, little-endian | `arm64.rs` | QEMU (`qemu-aarch64`) | Servers, Apple Silicon, Raspberry Pi 4/5 |
| `AArch64Be` | ARMv8 64-bit, big-endian | `aarch64_be.rs` | compile-only | Networking appliances, some embedded |
| `RiscV64` | RISC-V 64-bit, little-endian | `riscv64.rs` + `riscv_common.rs` | QEMU (`qemu-riscv64`) | VisionFive, SiFive boards |
| `RiscV32` | RISC-V 32-bit, little-endian | `riscv32.rs` + `riscv_common.rs` | compile-only | Embedded RV32 cores |
| `Arm32` | ARMv7 32-bit, little-endian | `arm32/mod.rs` + `arm32/disasm.rs` | QEMU (`qemu-arm`) | Legacy mobile, Pi 1/2 |
| `ArmEb` | ARMv7 32-bit, big-endian | `armeb.rs` | compile-only | Specialty embedded |
| `Mips64` | MIPS 64-bit, little-endian | `mips64/mod.rs` + `mips64/disasm.rs` | QEMU (`qemu-mips64`) | Loongson-class LE MIPS |
| `Mips64Be` | MIPS 64-bit, big-endian | `mips64be.rs` | compile-only | SGI/Loongson BE MIPS |
| `PowerPC64` | PowerPC 64-bit, big-endian | `ppc64/mod.rs` + `ppc64/disasm.rs` | compile-only | AIX, IBM POWER (BE mode) |
| `PowerPC64LE` | PowerPC 64-bit, little-endian | `ppc64le.rs` | QEMU (`qemu-ppc64le`) | ppc64le Linux (IBM POWER8/9 LE) |
| `LoongArch64` | LoongArch 64-bit | `loongarch64/mod.rs` + `loongarch64/disasm.rs` + `loongarch64/reg_alloc_isel.rs` + `loongarch64/stack_slot_isel.rs` | QEMU (`qemu-loongarch64`) | Loongson 3A/3B |
| `S390X` | IBM Z mainframe, big-endian | `s390x.rs` | QEMU (`qemu-s390x`) | z/Architecture |
| `Sparc64` | SPARC V9 64-bit, big-endian | `sparc64.rs` | compile-only | UltraSPARC, Fujitsu SPARC64 |
| `Alpha` | DEC Alpha 64-bit, little-endian | `alpha.rs` | compile-only | Legacy 64-bit RISC |
| `Hppa` | HP PA-RISC 32-bit, big-endian | `hppa.rs` | compile-only | Legacy HP workstations |
| `M68k` | Motorola 680x0 32-bit, big-endian | `m68k.rs` | compile-only | Amiga, Atari ST, classic Mac |
| `X86_32` | i386 32-bit | `x86_32/mod.rs` + `x86_32/disasm.rs` + `x86_32/stack_slot_isel.rs` | compile-only | Legacy PC compatibles |
| `Wasm32` | WebAssembly 32-bit | `wasm32/mod.rs` + `wasm32/disasm.rs` | wasmtime | Browser/standalone wasm runtime |

**7 backends are executable** via QEMU user-mode (or natively on x86_64, or
under `wasmtime` for wasm32): `x86_64`, `aarch64`, `riscv64`, `arm32`,
`ppc64le`, `loongarch64`, `s390x`. The remaining **12 are compile-only** —
they emit valid ELF machine code and pass IVE verification, but a QEMU
user-mode binary for that architecture is not in the standard sweep. The
[`scripts/kernel_parity.sh`](../scripts/kernel_parity.sh) sweep
compile-verifies all 19; it executes the 7 in the executable set.

The `Backend` trait (defined in [`backend.rs`](codegen/src/backend.rs))
requires: `target_info()`, `allocate_registers()`, `encode_function()`,
`encode_program()`, `return_stub()`, `trampoline(addr)`,
`disassemble(bytes, addr)`, `name()`. Each backend also has a `LatencyTable`
entry (see [`scheduler.rs`](codegen/src/scheduler.rs)).

---

## Key entry points

### `src/main.rs` — the CLI

The root binary crate's `src/main.rs` (~3,200 LOC) is the user-facing CLI:

```
vuma build <file>           — Parse + compile to AArch64 ELF (default), save to output file
vuma run <file>             — Build + execute (via QEMU aarch64 or native)
vuma check <file>           — Parse + SCG + BD inference + IVE verification only
vuma emit <isa> <file>      — Compile to specific ISA
vuma disasm <file>          — Read binary and disassemble
vuma verify <file>          — Run IVE state verification
vuma repl                   — Interactive REPL (parse expr, print AST)
vuma lsp                    — Start Language Server (LSP) for IDE/LLM integration
```

It delegates to `vuma::pipeline::*` for the actual compilation. The CLI
lives in `src/main.rs`; the LSP server lives in `src/lsp/`; the LLM API
bridge lives in `src/llm_api.rs`; telemetry in `src/telemetry.rs`.

### `src/bin/compile_dump.rs` — the test driver

The standard test driver used by every test runner script. Compiles a
single `.vuma` file to a given backend, emits the binary, and runs it
under QEMU / wasmtime. CLI:

```
compile_dump <input.vuma> <output.bin> <backend> [--verify] [--pmt-only]
compile_dump diag <backend> <input.vuma> [qemu-binary]
```

The `--verify` flag runs the three IVE state verifiers and prints
`IVE: Pass passed=N failed=0 total=N` on success. All kernel commits
require `--verify` to pass on `womb/kernel/kernel.vuma`. The `diag`
subcommand compiles + runs the file and prints the exit code (used by
[`scripts/pi5_test_suite.sh`](../scripts/pi5_test_suite.sh)).

### `src/bin/dump_ir.rs` — IR dumper

Dumps the lowered IR / SCG for a `.vuma` file. Used to bisect backend
codegen bugs (compare IR between two backends).

### `src/bin/dump_codegen_scg.rs` — codegen-side SCG dumper

Dumps the codegen-side SCG after pipeline transforms (post-monormorphize,
post-closures, post-egraph). Useful for debugging optimizer rewrites.

### `src/bin/scg_dump.rs` — parser-side SCG dumper

Dumps the parser-side SCG before lowering. Useful for debugging AST→SCG
lowering.

### `src/bin/parse_test.rs` — parse-only smoke driver

Parse-only smoke driver (no codegen). Useful for parser regression tests.

### `src/vuma/src/pipeline.rs` — the top-level compile entry points

The `pipeline` module is the top-level entry point for programmatic
compilation. It exposes:

- `compile(source, &CompileConfig) -> CompileResult`
- `compile_with_path(source, path, &CompileConfig) -> CompileResult`
- `compile_modules(modules, &CompileConfig) -> CompileResult`
- `run_scg_transforms(scg) -> scg` — the SCG-side transforms
- `run_ir_pipeline(ir) -> ir` — the IR-side O2 pipeline
- `bridge_ast_to_codegen_scg(ast) -> scg` — AST → codegen-side SCG
- `build_pmt_layout_specs(ast) -> LayoutRegistry` — PMT layout registry
- `CompileConfig`, `CompileTarget`, `OptLevel`, `VerificationLevel`,
  `CompileResult`

---

## The `runtime/` module

The `src/codegen/src/runtime/` directory contains the runtime support
modules that back the PMT state model and FFI marshal matrix. These are
**not** Rust runtime code that gets linked into the emitted binary — they
are the Rust-side implementations that the codegen emits calls to.

| File | LOC | Purpose |
|------|-----|---------|
| `runtime/arena.rs` | 181 | The arena allocator: `___pmt_buffer` symbol, `arena_alloc` bounds check, `__arena_overflow` trap on overflow. The `arena_alloc` codegen sequence loads the capacity (stored at `[arena_ptr+16]`), compares the new offset against it, and traps via `__arena_overflow` on overflow. |
| `runtime/ffi_scratch.rs` | 183 | The FFI marshal scratchpad: a thread-local stack-shaped buffer (`___ffi_scratch_alloc`), **never aliased by `___pmt_buffer`** — the state verifiers never see it. Used by `Marshal` mode in the FFI 4-mode matrix for NUL-terminated strings and C-owned memory round-trips. |
| `runtime/callback.rs` | 196 | FFI callback support: trampolines for invoking VUMA functions from C. Used by `MayRetain` mode in the FFI 4-mode matrix. |
| `runtime/vuma_context.rs` | 241 | The `vuma_context_t` host API: a C-compatible context structure for embedding VUMA in a host application. Exposed via [`vuma_vm.h`](../vuma_vm.h). |
| `runtime/mod.rs` | 12 | Module re-exports. |

The runtime module is the L3 layer of the VWK kernel's 4-layer cake (see
[`docs/kernel-architecture.md` §1](../docs/kernel-architecture.md)). The
`__arena_overflow` symbol is defined on all 19 backends as a trap
instruction (`ud2` on x86_64, `brk #0` on aarch64, `unimp` on riscv64,
etc.) — on hosted x86_64 it surfaces as a non-zero exit code; on bare metal
it halts the CPU.

---

## How to add a new backend

The full guide is in [`docs/contributing.md` §4](../docs/contributing.md#4-adding-a-new-backend).
The short version:

1. **Implement the `Backend` trait** in a new module
   `src/codegen/src/<arch>.rs` (or `src/codegen/src/<arch>/{mod,disasm}.rs`
   for backends with separate disassembly). The trait requires
   `target_info`, `allocate_registers`, `encode_function`, `encode_program`,
   `return_stub`, `trampoline`, `disassemble`, `name`. Add a `LatencyTable`
   entry in `scheduler.rs`.
2. **Add a variant to `BackendKind`** in `backend.rs` and extend the
   `isa_name`, `from_str`, and `qemu_binary` match arms. Also extend the
   `backend_from_name` helper in [`src/bin/compile_dump.rs`](bin/compile_dump.rs).
3. **Add the QEMU mapping** to `scripts/kernel_parity.sh`'s `QEMU_MAP`
   array and `scripts/pi5_test_suite.sh`'s `binfmt_misc` `entries` array.
4. **Add tests** — at least one PMT test under `tests/gold_standard/<arch>/`
   with an `// Expected exit code: N` header. Run the new backend on the
   full gold-standard suite to confirm agreement with the other 18
   backends.

The `Backend` trait is documented in
[`codegen/src/backend.rs`](codegen/src/backend.rs). The `LatencyTable`
(which the scheduler uses to model instruction costs) is documented in
[`codegen/src/scheduler.rs`](codegen/src/scheduler.rs).

---

## See also

- [`docs/architecture.md`](../docs/architecture.md) — full compiler
  architecture (PMT pipeline, state type system, 3 state verifiers,
  Behavioral Descriptors, e-graph layout optimization, dependent state
  types, FFI 4-mode marshal matrix).
- [`docs/building.md`](../docs/building.md) — build reference, profiles,
  constrained-memory workaround, QEMU installation.
- [`docs/contributing.md`](../docs/contributing.md) — code style, adding a
  backend, adding a test, VUMA code patterns.
- [`docs/kernel-architecture.md`](../docs/kernel-architecture.md) — how the
  VWK kernel uses the runtime/ module (the 4-layer cake).
- [`tests/README.md`](../tests/README.md) — the test suite layout.
- [`womb/kernel/README.md`](../womb/kernel/README.md) — the PMT-pure kernel
  source tree.
