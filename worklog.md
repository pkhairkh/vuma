# VUMA Project — Detailed Examination Worklog

## Project
VUMA (Verified-Unsafe Memory Access) — an AI-native programming language framework
written in Rust. Repo: `https://github.com/pkhairkh/vuma.git`. The actual project
lives in the `vuma/` subfolder of the clone (a Rust workspace).

## Scale
- 12 workspace crates: scg, ive, vuma(core), bd, cor, projection, parser, codegen,
  std, proof, tests, package
- 194 Rust source files, ~265,000 lines of Rust
- 47 example `.vuma` programs
- 8 codegen backends: x86_64, AArch64, RISC-V 64, ARM32, MIPS64, PPC64, LoongArch64, Wasm32
- Pinned to nightly-2026-03-01

---
Task ID: 1
Agent: main
Task: Check out the VUMA project in detail — build it, find real issues, fix what's fixable

Work Log:
- Cloned the repo; confirmed the real Rust workspace is in `vuma/` (not the
  `download/vuma-project/` mirror, which is an older snapshot).
- Read the full README, key crate docs (scg, ive, bd, vuma-core), pipeline.rs,
  backend.rs, and several example programs (hello_memory, doubly_linked_list,
  arena_allocator, lock_free_queue, fibonacci).
- Installed the Rust nightly toolchain (rustc 1.96.0-nightly).
- Ran `cargo check --workspace` → **FAILED** with 3 compile errors in
  `vuma-projection` (lib): non-exhaustive pattern matches missing the newer
  `NodeType::{StructDef, EnumDef, Match, ConstantTime}` and
  `NodePayload::{StructDef, EnumDef, Match, ConstantTime}` variants, plus a
  stale `operation` field reference on `ComputationNode` (renamed to `kind:
  ComputationKind::Other(..)` some time ago).
- Fixed the projection adapter (`src/projection/src/scg_adapter.rs`): added the
  4 missing NodeType arms, 4 missing NodePayload arms, and corrected the
  ComputationNode field. → `cargo check --workspace` now passes cleanly.
- Ran `cargo test --workspace --no-run` → **FAILED** with 19+ compile errors in
  *test* code across 5 crates (ive, projection, cor, vuma-core, bd): all the same
  stale `operation:` field on `ComputationNode` constructions.
- Fixed every stale `operation:` → `kind: ComputationKind::Other(..)` in test
  modules: bd_solver.rs, interprocedural.rs, verification.rs, textual.rs,
  conversational.rs, visual.rs, cor/bridge.rs, msg_builder.rs, scg_to_msg.rs,
  bd/inference.rs. Added the missing `ComputationKind` import in each. Carefully
  *reverted* 4 accidental edits in `bd/repd.rs` where `operation:` belonged to
  `BdError::InvalidOperation` (a different struct), not `ComputationNode`.
- All tests now compile. Ran `cargo test --workspace`.
- Found 1 real logic bug in `src/telemetry.rs`: `stage_end()` overwrote
  `stage_metrics` with a fresh `StageMetrics { error_count: 0, warning_count: 0 }`,
  destroying per-stage error/warning counts accumulated *before* the stage ended;
  additionally `increment_stage_error`/`increment_stage_warning` silently dropped
  counts when the stage entry didn't yet exist. Fixed both with
  `entry().or_insert(..)` semantics so counts survive. The
  `test_telemetry_stage_errors` test now passes.

Stage Summary:
- **Workspace now compiles cleanly** (was broken: 3 lib errors + ~19 test errors).
- **Test results after fixes: 1049 passed, 35 failed.**
- The 35 failures are ALL pre-existing and ALL in `vuma-codegen` — I did not touch
  codegen. Breakdown:
  - 27 in `loongarch64` — instruction selection emits placeholder `"instr"`
    tokens instead of real opcodes (e.g. `test_isel_add_emits_add` expects
    `add.d` but gets a prologue + `"instr"` + return). Worse than the README's
    "passes individual operation tests" claim.
  - 6 in `scg_to_ir` — the SCG→IR lowering silently accepts unknown variables
    instead of returning `Err(UnknownVariable)`; also phi/load-store issues.
  - 1 in `arm64` — `decode_ldr_str_roundtrip`: STR instruction fails to decode.
  - 1 in `x86_64` — `test_isel_select`.
- Root causes of the original breakage: the SCG `NodeType`/`NodePayload` enums and
  `ComputationNode` were extended (struct/enum/match/constant-time support) but
  downstream consumers (projection adapter + tests) were not fully updated. This
  is a classic "added a variant, forgot a match arm" integration regression.

Unresolved / Next-phase priorities:
1. **scg_to_ir unknown-variable bug** (correctness): the IR builder returns Ok
   for programs referencing undeclared variables. Should return
   `CodegenError::UnknownVariable`. Start at `src/codegen/src/scg_to_ir.rs:3984`.
2. **LoongArch64 isel** (27 failures): emits placeholder `"instr"` instead of
   real instructions. Either the isel table is stubbed or there's a dispatch
   fall-through. Start at `src/codegen/src/loongarch64/mod.rs:5441`.
3. **ARM64 STR decode** (1 failure): `decode_ldr_str_roundtrip` at
   `src/codegen/src/arm64.rs:5442` — STR encoding/decoding asymmetry.
4. **x86_64 isel_select** (1 failure).
5. ~80 compiler warnings (unused imports, dead code in ppc64/riscv64/x86_64
   backends) — cosmetic but worth a cleanup pass.
