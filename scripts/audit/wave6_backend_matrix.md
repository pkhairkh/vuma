# Wave 6 — Per-Backend Matrix Consistency Audit (caveat §6)

- **Task ID:** 6-c-audit
- **Agent:** 6-c-audit (sub-agent, wave 6)
- **Wave:** 6 (depends on waves 0 / 1 / 2 / 3 / 4 / 5 / 6-a / 6-b)
- **Caveat addressed:** §6 — per-backend matrix consistency between
  `docs/backends.md`, `docs/fp_backends.md`, and the actual backend
  modules under `src/codegen/src/`.
- **Files in scope (READ-ONLY audit):**
  - `/home/z/my-project/vuma/docs/backends.md`
  - `/home/z/my-project/vuma/docs/fp_backends.md`
  - `/home/z/my-project/vuma/src/codegen/src/` (backend module listing)
- **Files out of scope:** any source file under `vuma/src/` (no source edits).
- **DoD:** this summary markdown exists; either zero drift, OR all drift
  items listed for orchestrator follow-up.

## Method

1. Listed `src/codegen/src/` and treated each backend module as one
   backend. A backend module is either:
   - a single file `<isa>.rs` at top level (e.g. `arm64.rs`, `hppa.rs`),
     or
   - a directory `<isa>/` whose `mod.rs` declares the backend
     (e.g. `arm32/`, `x86_64/`).
   Non-backend top-level files (`backend.rs`, `regalloc.rs`, `emit.rs`,
   `ir.rs`, `marshal.rs`, `scheduler.rs`, `target_desc.rs`, `egraph.rs`,
   `dwarf.rs`, `opt.rs`, `closures.rs`, `effects.rs`, `capability.rs`,
   `alias_analysis.rs`, `escape_analysis.rs`, `vectorize.rs`,
   `control_flow.rs`, `loop_unroll.rs`, `monomorphize.rs`,
   `memory_safety.rs`, `regalloc_emit.rs`, `ipc.rs`, `ipc_lowering.rs`,
   `scg_to_ir.rs`, `bv_verify.rs`, `syscall_abi.rs`, `wrapper_smoke.rs`,
   `proof_artifacts.rs`, `riscv_common.rs`, `lib.rs`, and the
   `runtime/` subdirectory) are shared infrastructure, not backends, and
   are excluded.
2. Extracted the 19-row backend table from `docs/backends.md`
   (§1, lines 38–58) — columns `Name` and `File`.
3. Extracted the 19-row FP table from `docs/fp_backends.md`
   (§Summary, lines 15–35) — columns `Backend` and `File`.
4. Computed three-way set diff:
   - src ⊕ docs/backends.md
   - src ⊕ docs/fp_backends.md
5. Cross-checked `File` column entries resolve to the listed source path
   (informational; the only file-name asymmetry — `arm64.rs` ↔ `aarch64`
   — is documented in both docs and is the canonical reference-backend
   naming, not drift).

## Backend lists

### Source backends (19) — `src/codegen/src/`

| # | Module | Path | Backend name |
|--:|--------|------|--------------|
|  1 | dir    | `arm32/`             | `arm32`       |
|  2 | file   | `aarch64_be.rs`      | `aarch64_be`  |
|  3 | file   | `alpha.rs`           | `alpha`       |
|  4 | file   | `arm64.rs`           | `aarch64`     |
|  5 | file   | `armeb.rs`           | `armeb`       |
|  6 | file   | `hppa.rs`            | `hppa`        |
|  7 | dir    | `loongarch64/`       | `loongarch64` |
|  8 | file   | `m68k.rs`            | `m68k`        |
|  9 | dir    | `mips64/`            | `mips64`      |
| 10 | file   | `mips64be.rs`        | `mips64be`    |
| 11 | dir    | `ppc64/`             | `ppc64`       |
| 12 | file   | `ppc64le.rs`         | `ppc64le`     |
| 13 | file   | `riscv32.rs`         | `riscv32`     |
| 14 | file   | `riscv64.rs`         | `riscv64`     |
| 15 | file   | `s390x.rs`           | `s390x`       |
| 16 | file   | `sparc64.rs`         | `sparc64`     |
| 17 | dir    | `wasm32/`            | `wasm32`      |
| 18 | dir    | `x86_32/`            | `x86_32`      |
| 19 | dir    | `x86_64/`            | `x86_64`      |

**Total: 19 backends** = **7 directory-style** (`arm32`, `loongarch64`,
`mips64`, `ppc64`, `wasm32`, `x86_32`, `x86_64`) **+ 12 single-file**
(`aarch64_be`, `alpha`, `arm64`, `armeb`, `hppa`, `m68k`, `mips64be`,
`ppc64le`, `riscv32`, `riscv64`, `s390x`, `sparc64`).

### docs/backends.md table (19) — §1, rows 1–19

`aarch64`, `aarch64_be`, `x86_64`, `x86_32`, `riscv64`, `riscv32`,
`loongarch64`, `arm32`, `armeb`, `mips64`, `mips64be`, `ppc64`,
`ppc64le`, `wasm32`, `sparc64`, `s390x`, `m68k`, `alpha`, `hppa`.

### docs/fp_backends.md table (19) — §Summary, rows 1–19

`x86_64`, `x86_32`, `aarch64`, `aarch64_be`, `arm32`, `armeb`,
`riscv64`, `riscv32`, `mips64`, `mips64be`, `ppc64`, `ppc64le`,
`loongarch64`, `s390x`, `sparc64`, `alpha`, `hppa`, `m68k`, `wasm32`.

## Three-way diff

| Comparison | In src only (missing from docs) | In docs only (stale docs) |
|---|---|---|
| `src/` ⊕ `docs/backends.md`  | — | — |
| `src/` ⊕ `docs/fp_backends.md` | — | — |
| `docs/backends.md` ⊕ `docs/fp_backends.md` | — | — |

All three lists contain exactly the same 19 backend names. No drift.

## Cross-checks (informational)

- The `File` column in `docs/backends.md` matches the source layout for
  all 19 rows: 7 directory-style entries are written as
  `<isa>/{mod,stack_slot_isel,disasm}.rs` or `<isa>/{mod,disasm}.rs`
  (matching the directory contents — `loongarch64/` and `x86_32/` and
  `x86_64/` have `stack_slot_isel.rs`; `arm32/`, `mips64/`, `ppc64/`,
  `wasm32/` do not), and 12 single-file entries are written as
  `<isa>.rs`.
- The `arm64.rs` ↔ `aarch64` name split is documented in both files
  (`docs/backends.md` row 1 lists `aarch64` → `arm64.rs`;
  `docs/fp_backends.md` row 3 lists `aarch64` → `arm64.rs` / `emit.rs`).
  This is the canonical reference-backend naming, **not drift**.
- Wave 2 (task `2-d-doc`) already synchronized the `Regalloc` column in
  `docs/backends.md` §1 against the actual allocator wiring. Wave 6-c
  confirms that column remained stable and that the broader matrix
  (backend names + file paths) is also consistent.
- `docs/fp_backends.md` cites `emit.rs` as a secondary file for the
  `aarch64` row; `src/codegen/src/emit.rs` exists (shared emit
  infrastructure) — accurate.

## DoD Assessment

| DoD criterion | Status | Evidence |
|---|---|---|
| Summary markdown exists at `vuma/scripts/audit/wave6_backend_matrix.md` | **PASS** | this file |
| Zero drift, OR all drift items listed for orchestrator follow-up | **PASS** | zero drift — 19/19 backends match across `src/`, `docs/backends.md`, and `docs/fp_backends.md`; both directions of both diffs are empty |

## Constraint check

- READ-ONLY audit: no source files edited. `git status` after this task
  will show only the new audit markdown (+ the worklog append).
- No `git push` invoked (local commit only).
- No further sub-agents spawned.
- Time budget: well under 5 minutes (single read-only diff pass).

## Note for orchestrator

Caveat §6 per-backend matrix audit complete. **Zero drift.** The 19
backends in `src/codegen/src/` exactly match the 19 rows of
`docs/backends.md` §1 and the 19 rows of `docs/fp_backends.md` §Summary.
Wave 2's allocator-column sync held; no further doc edits are needed.
No follow-up items for the orchestrator.
