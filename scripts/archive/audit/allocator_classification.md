# Allocator Classification Audit — Caveat §2.1

- **Task ID:** 2-a-audit
- **Wave:** 2
- **Caveat addressed:** §2.1 — Stack-slot ISel on 15 of 19 backends
- **Audit type:** READ-ONLY (no source files edited)
- **HEAD before audit:** `196bd372 [wave-1-dod-pass] build baseline green`
- **Audit date:** Wave 2 (post Wave-1 DoD-pass)
- **Scope:** all 19 VUMA backends under `src/codegen/src/`

## 1. Methodology

For each of the 19 backends (per `docs/backends.md` §1 Overview Table),
this audit reads the backend's `allocate_registers` impl and classifies
the allocator kind using the protocol's priority order:

1. **Real linear-scan (`LinearScanAllocator`)** — direct call to
   `LinearScanAllocator::new(...)` in the production path.
2. **Real target-agnostic linear-scan (`TargetAgnosticRegAlloc`)** —
   direct call to `TargetAgnosticRegAlloc::new(...)` (typically via the
   per-backend `try_real_regalloc` helper, which looks the ISA up in
   `TargetDescRegistry`).
3. **Stack-slot ISel** — every vreg is assigned a stack slot, operands
   are loaded/stored through memory for each instruction; no
   `LinearScanAllocator` and no `TargetAgnosticRegAlloc` in the
   production path. (The backend may have an opt-in `_real` variant
   gated by `use_real_regalloc: false` default — such a variant is
   considered non-production.)
4. **Wasm-structured** — special-case for `wasm32` (no registers in the
   ISA; vregs are mapped to Wasm locals and the IR is lowered directly
   to Wasm bytecode via `lower_function`).

`TargetDesc` status is also recorded per backend: "Full (registered)"
means a `*_target_desc()` constructor exists in `target_desc.rs` AND
the ISA is inserted into `TargetDescRegistry::new()`
(`target_desc.rs:1386-1396`); "Missing" means no `*_target_desc()`
constructor exists (only a `LatencyTable::{isa}()` constructor exists,
which populates just the `latency_table` field, not the register file
or ABI).

A wrapper backend (e.g. `aarch64_be`) delegates `allocate_registers`
to its inner backend (`self.inner.allocate_registers(func)`); its
allocator kind therefore inherits the parent's kind.

## 2. Classification Table

| # | Backend | Allocator Kind | TargetDesc Status | Source File:Line | Notes |
|--:|---------|----------------|-------------------|-------------------|-------|
|  1 | `aarch64`     | Real — `TargetAgnosticRegAlloc` via `try_real_regalloc` (annotation-only on top of stack-slot bytes) | Full (registered: `target_desc.rs:1420`, registry: `:1388`) | `src/codegen/src/backend.rs:3162` (allocate_registers), `:3249` (try_real_regalloc call), `:3114` (`TargetAgnosticRegAlloc::new`) | **DRIFT-4 (minor)**: caveat §2.1 says aarch64 uses `LinearScanAllocator`, but the production `allocate_registers` calls `TargetAgnosticRegAlloc::new` via `try_real_regalloc` (`backend.rs:3114`). `LinearScanAllocator::new` is invoked only inside `#[cfg(test)]` modules (`regalloc.rs:4738, 4777, 4806, 4841, 4876, 4958, 5013, 5053, 5093, 5140, 5176, 5204, 5293, 5328, 5507, 5573`; `emit.rs:9188, 9208, 9287, 9389`). Encoded bytes always come from `emitter.emit_function(func, None)` (stack-slot ISel path, `backend.rs:3180`); the real allocator only annotates each instruction's `reads`/`writes` metadata via `regalloc_emit::annotate_with_regalloc`. |
|  2 | `aarch64_be`  | Real — inherits aarch64 (delegates `allocate_registers` to `self.inner`) | Inherits aarch64 (no separate `*_target_desc()`) | `src/codegen/src/aarch64_be.rs:150-152` (delegates) | **DRIFT-1**: caveat §2.1 lists `aarch64_be` in the stack-slot column, but it actually inherits aarch64's real-linear-scan path. `docs/backends.md` §1 row 2 correctly records "inherits AArch64" with `LinearScan (real)`. |
|  3 | `x86_64`      | Real — `TargetAgnosticRegAlloc` via `try_real_regalloc` (annotation-only) | Full (registered: `target_desc.rs:1932`, registry: `:1392`) | `src/codegen/src/x86_64/mod.rs:4141` (allocate_registers), `:4149` (try_real_regalloc call), `:4081` (`TargetAgnosticRegAlloc::new`), `:4143` (`stack_slot_isel::allocate_registers` baseline) | Matches caveat §2.1. Encoded bytes come from `stack_slot_isel::allocate_registers` (line 4143); real allocator annotates metadata only. |
|  4 | `x86_32`      | Stack-slot (pure `stack_slot_isel::allocate_registers`; `use_real_regalloc=false` default — `_real` path is greedy stub, not linear-scan) | Missing (only `LatencyTable::x86_32()` at `target_desc.rs:667`; no `x86_32_target_desc()` constructor; not in registry) | `src/codegen/src/x86_32/mod.rs:3410` (allocate_registers), `:3411` (`stack_slot_isel::allocate_registers`), `:3344` (`use_real_regalloc` field), `:3352` (`use_real_regalloc: false` default), `:4795` (test sets `true`) | Matches caveat §2.1. `use_real_regalloc` only set `true` in `#[cfg(test)]`. |
|  5 | `riscv64`     | Real — `TargetAgnosticRegAlloc` via `try_real_regalloc` (annotation-only) | Full (registered: `target_desc.rs:1562`, registry: `:1389`) | `src/codegen/src/riscv64.rs:6607` (allocate_registers), `:10306` (try_real_regalloc call), `:6557` (`TargetAgnosticRegAlloc::new`) | Matches caveat §2.1. Encoded bytes from stack-slot baseline; real allocator annotates metadata only. |
|  6 | `riscv32`     | Stack-slot (pure; no `TargetAgnosticRegAlloc`, no `LinearScanAllocator`, no `use_real_regalloc` flag, no `try_real_regalloc`) | Missing (only `LatencyTable::riscv32()` at `target_desc.rs:661`; not in registry) | `src/codegen/src/riscv32.rs:5805` (allocate_registers) | Matches caveat §2.1. Pure stack-slot — no opt-in real path at all. |
|  7 | `loongarch64` | Stack-slot (pure `stack_slot_isel::allocate_registers`); `emit_function_regalloc`/`run_regalloc("loongarch64")` exist at `mod.rs:2590,2605` but are NOT called from production `allocate_registers` | Full (registered: `target_desc.rs:1788`, registry: `:1391`) — but UNUSED by production `allocate_registers` | `src/codegen/src/loongarch64/mod.rs:2621` (allocate_registers → `stack_slot_isel::allocate_registers`), `:6946` (`pub mod reg_alloc_isel;` commented out → dead code) | Matches caveat §2.1. The TargetDesc exists in the registry but is only consulted by the unused `emit_function_regalloc` path. `reg_alloc_isel.rs` (1.6 K LOC) is dead code per `docs/backends.md` §2. |
|  8 | `arm32`       | Stack-slot (pure; `emit_function_regalloc`/`run_regalloc("arm32")` exist at `mod.rs:3465,3480` but are NOT called from production `allocate_registers`) | Full (registered: `target_desc.rs:2046`, registry: `:1393`) — but UNUSED by production `allocate_registers` | `src/codegen/src/arm32/mod.rs:4485` (allocate_registers, pure stack-slot per comment at `:4486-4500`) | Matches caveat §2.1. TargetDesc exists but is unused by production path. |
|  9 | `armeb`       | Stack-slot — inherits arm32 (delegates `allocate_registers` to `self.inner`) | Inherits arm32 (only `LatencyTable::armeb()` at `target_desc.rs:682`; no separate `armeb_target_desc()`) | `src/codegen/src/armeb.rs:185-187` (delegates) | Matches caveat §2.1. arm32's `allocate_registers` is pure stack-slot. |
| 10 | `mips64`      | Stack-slot (`use_real_regalloc=false` default; `_real` path is greedy stub that re-runs `_ss` and annotates — NOT linear-scan) | Full (registered: `target_desc.rs:2170`, registry: `:1394`) — but UNUSED by production `allocate_registers` | `src/codegen/src/mips64/mod.rs:3720` (allocate_registers), `:3722-3724` (`use_real_regalloc` branch → `_ss` default), `:1877` (`use_real_regalloc: false` default), `:3648` (`mips64_allocate_registers_real` greedy stub), `:6166` (test sets `true`) | Matches caveat §2.1. TargetDesc exists but production path doesn't consult it. `use_real_regalloc` only set `true` in `#[cfg(test)]`. |
| 11 | `mips64be`    | Stack-slot — inherits mips64 (delegates `allocate_registers` to `self.inner`) | Inherits mips64 (only `LatencyTable::mips64be()` at `target_desc.rs:677`; no separate `mips64be_target_desc()`) | `src/codegen/src/mips64be.rs:200-202` (delegates) | Matches caveat §2.1. mips64's default `allocate_registers` is stack-slot. |
| 12 | `ppc64`       | Real — `TargetAgnosticRegAlloc` via `try_real_regalloc` (annotation-only) | Full (registered: `target_desc.rs:2320`, registry: `:1395`) | `src/codegen/src/ppc64/mod.rs:3084` (allocate_registers), `:4864` (try_real_regalloc call), `:3026` (`TargetAgnosticRegAlloc::new`), `:4857-4863` (comment: "real allocator is now always attempted... `use_real_regalloc` field retained for API compatibility but no longer gates this path") | Matches caveat §2.1. Encoded bytes from stack-slot baseline; real allocator annotates metadata only. |
| 13 | `ppc64le`     | Real — inherits ppc64 (delegates `allocate_registers` to `self.inner`) | Inherits ppc64 (only `LatencyTable::ppc64le()` at `target_desc.rs:672`; no separate `ppc64le_target_desc()`) | `src/codegen/src/ppc64le.rs:400-406` (delegates; comment "Register allocation is endianness-independent") | **DRIFT-2**: caveat §2.1 lists `ppc64le` in the stack-slot column, but it actually inherits ppc64's real-linear-scan path. `docs/backends.md` §1 row 13 correctly records "inherits PPC64" with `TargetAgnostic (real)`. |
| 14 | `wasm32`      | Wasm-structured (no registers; lowers IR to Wasm bytecode via `lower_function`; vregs → Wasm locals) | Full (registered: `target_desc.rs:1706`, registry: `:1390`) — but UNUSED by production `allocate_registers` (Wasm has no registers) | `src/codegen/src/wasm32/mod.rs:4631` (allocate_registers → `lower_function`), `:4632` (comment: "Wasm has no registers — map virtual regs to locals") | **DRIFT-3**: caveat §2.1 lists `wasm32` in the stack-slot column, but it actually uses Wasm-structured control flow (its own paradigm — see `docs/backends.md` §1 row 14 "Wasm-structured" and §5 "wasm32 fork emulation"). Not stack-slot ISel in the same sense as the other 14 (which use scratch physical registers + per-vreg stack slots). The TargetDesc is registered but irrelevant (Wasm has no registers to allocate). |
| 15 | `sparc64`     | Stack-slot (`use_real_regalloc=false` default; `_real` path is greedy stub) | Missing (only `LatencyTable::sparc64()` at `target_desc.rs:694`; not in registry) | `src/codegen/src/sparc64.rs:4885` (allocate_registers), `:4887-4889` (`use_real_regalloc` branch → `_ss` default), `:4790` (`use_real_regalloc: false` default), `:2225` (`sparc64_allocate_registers_real` greedy stub), `:6746` (test sets `true`) | Matches caveat §2.1. `use_real_regalloc` only set `true` in `#[cfg(test)]`. |
| 16 | `s390x`       | Stack-slot (`use_real_regalloc=false` default; `_real` path is greedy stub) | Missing (only `LatencyTable::s390x()` at `target_desc.rs:757`; not in registry) | `src/codegen/src/s390x.rs:3048` (allocate_registers), `:3050-3052` (`use_real_regalloc` branch → `_ss` default), `:2953` (`use_real_regalloc: false` default), `:1327` (`s390x_allocate_registers_real` greedy stub), `:4394` (test sets `true`) | Matches caveat §2.1. `use_real_regalloc` only set `true` in `#[cfg(test)]`. |
| 17 | `m68k`        | Stack-slot (`use_real_regalloc=false` default; `_real` path is greedy stub) | Missing (only `LatencyTable::m68k()` at `target_desc.rs:820`; not in registry) | `src/codegen/src/m68k.rs:4577` (allocate_registers), `:4579-4581` (`use_real_regalloc` branch → `_ss` default), `:4482` (`use_real_regalloc: false` default), `:1141` (`m68k_allocate_registers_real` greedy stub), `:6465` (test sets `true`) | Matches caveat §2.1. `use_real_regalloc` only set `true` in `#[cfg(test)]`. |
| 18 | `alpha`       | Stack-slot (`use_real_regalloc=false` default; `_real` path is greedy stub) | Missing (only `LatencyTable::alpha()` at `target_desc.rs:882`; not in registry) | `src/codegen/src/alpha.rs:3112` (allocate_registers), `:3113-3116` (`use_real_regalloc` branch → `_ss` default), `:3018` (`use_real_regalloc: false` default), `:1268` (`alpha_allocate_registers_real` greedy stub), `:5053` (test sets `true`) | Matches caveat §2.1. `use_real_regalloc` only set `true` in `#[cfg(test)]`. |
| 19 | `hppa`        | Stack-slot (`use_real_regalloc=false` default; `_real` path is greedy stub) | Missing (only `LatencyTable::hppa()` at `target_desc.rs:944`; not in registry) | `src/codegen/src/hppa.rs:5608` (allocate_registers), `:5609-5612` (`use_real_regalloc` branch → `_ss` default), `:1338` (`use_real_regalloc: false` default), `:5536` (`hppa_allocate_registers_real` greedy stub), `:6733` (test sets `true`) | Matches caveat §2.1. `use_real_regalloc` only set `true` in `#[cfg(test)]`. |

## 3. Summary by Allocator Kind

| Allocator Kind | Count | Backends |
|----------------|------:|----------|
| Real linear-scan (direct, `TargetAgnosticRegAlloc` via `try_real_regalloc`) | 4 | `aarch64`, `x86_64`, `riscv64`, `ppc64` |
| Real linear-scan (inherited via wrapper delegation) | 2 | `aarch64_be` (→ aarch64), `ppc64le` (→ ppc64) |
| Stack-slot ISel (pure or `use_real_regalloc=false` default) | 12 | `arm32`, `armeb`, `mips64`, `mips64be`, `riscv32`, `x86_32`, `loongarch64`, `sparc64`, `s390x`, `m68k`, `alpha`, `hppa` |
| Wasm-structured (no registers) | 1 | `wasm32` |
| **Total** | **19** | |

## 4. Comparison Against Caveat §2.1

Caveat §2.1 (`docs/caveats.md:44-52`) claims:
> 4 real linear-scan: `aarch64`, `x86_64`, `riscv64`, `ppc64`
> 15 stack-slot: `arm32`, `armeb`, `aarch64_be`, `mips64`, `mips64be`, `ppc64le`, `riscv32`, `x86_32`, `sparc64`, `s390x`, `m68k`, `alpha`, `hppa`, `loongarch64`, `wasm32`

### Verdict: **DRIFT on 3 backends + 1 minor naming drift**

The canonical "4 directly-wired real-linear-scan backends" (`aarch64`,
`x86_64`, `riscv64`, `ppc64`) MATCH the caveat. The drift is in how the
other 15 are bucketed:

| Drift | Backend | Caveat §2.1 says | Audit finds | Cross-ref |
|-------|---------|------------------|-------------|-----------|
| DRIFT-1 | `aarch64_be` | stack-slot | Real — inherits aarch64's `TargetAgnosticRegAlloc` via `try_real_regalloc` (delegation at `aarch64_be.rs:150-152`) | `docs/backends.md` §1 row 2 correctly records "inherits AArch64" + `LinearScan (real)` |
| DRIFT-2 | `ppc64le` | stack-slot | Real — inherits ppc64's `TargetAgnosticRegAlloc` via `try_real_regalloc` (delegation at `ppc64le.rs:400-406`) | `docs/backends.md` §1 row 13 correctly records "inherits PPC64" + `TargetAgnostic (real)` |
| DRIFT-3 | `wasm32` | stack-slot | Wasm-structured (no registers; vregs → Wasm locals; IR lowered directly to Wasm bytecode via `lower_function` at `wasm32/mod.rs:4631-4660`) | `docs/backends.md` §1 row 14 records "Wasm-structured" (separate category, not stack-slot); §2 lists wasm32 alongside the 4 real backends as "does not use stack-slot ISel" |
| DRIFT-4 (minor) | `aarch64` | uses `LinearScanAllocator` (`regalloc.rs`) | Production `allocate_registers` (`backend.rs:3162`) actually calls `TargetAgnosticRegAlloc::new` via `try_real_regalloc` (`backend.rs:3114`). `LinearScanAllocator::new` is invoked only in `#[cfg(test)]` modules (`regalloc.rs:4738+`, `emit.rs:9188+`). The `emit_function_regalloc` path in `emit.rs:1056` (which does consume an `AllocationResult` from `LinearScanAllocator`) is reachable only when `emitter.emit_function(func, Some(alloc))` is called; `AArch64Backend::allocate_registers` calls `emitter.emit_function(func, None)` (`backend.rs:3180`), so the regalloc path is NOT taken. | Both allocators are "real linear-scan" by the protocol's classification, so this is a naming drift only — the 4-real/15-stack-slot count is unaffected. |

### Corrected Split (per audit)

- **6 backends** with real linear-scan wired (4 direct + 2 inherited):
  `aarch64`, `aarch64_be`, `x86_64`, `riscv64`, `ppc64`, `ppc64le`
- **12 backends** with stack-slot ISel:
  `arm32`, `armeb`, `mips64`, `mips64be`, `riscv32`, `x86_32`,
  `loongarch64`, `sparc64`, `s390x`, `m68k`, `alpha`, `hppa`
- **1 backend** with Wasm-structured lowering:
  `wasm32`
- **Total: 19** ✓

This corrected split (6/12/1) matches `docs/backends.md` §1 and §2 exactly
(`docs/backends.md:33-66`). Caveat §2.1 should be updated to reflect
this; the simplest fix is to move `aarch64_be` and `ppc64le` out of the
stack-slot column into the real-linear-scan column (alongside their
parent backends), and to either move `wasm32` to its own row (matching
`docs/backends.md` §1 row 14) or to clarify that "stack-slot" in the
caveat is used loosely to mean "no real register allocation" (which is
accurate for wasm32, but conflates two distinct lowering strategies).

## 5. Additional Observations

### 5.1 All "real" backends are annotation-only

Even on the 4 directly-wired real-linear-scan backends (`aarch64`,
`x86_64`, `riscv64`, `ppc64`) and the 2 wrappers (`aarch64_be`,
`ppc64le`), the encoded instruction bytes ALWAYS come from the
stack-slot ISel path:

- `aarch64`: `emitter.emit_function(func, None)` (`backend.rs:3180`)
  → `emit_function_stack_slot` (per the dispatch at `emit.rs:959-992`).
- `x86_64`: `stack_slot_isel::allocate_registers(func)?` (`x86_64/mod.rs:4143`).
- `riscv64`: stack-slot baseline (see `riscv64.rs:6607-10300`).
- `ppc64`: stack-slot baseline (see `ppc64/mod.rs:3084-4863`).

The real allocator (`TargetAgnosticRegAlloc`) is then run via
`try_real_regalloc(func)` and its `RegAllocResult` is fed to
`regalloc_emit::annotate_with_regalloc(&mut allocated, &alloc)`, which
updates each `AllocatedInstruction`'s `reads`/`writes` metadata
fields. **The `encoded` bytes are NOT modified** (per
`regalloc_emit.rs:82-92`: "The `encoded` bytes are NOT modified — they
remain the stack-slot ISel's output").

This means the runtime performance of all 6 "real" backends is still
bounded by the spill path (load/store per operand), exactly as the
caveat's "Implication" paragraph describes for the stack-slot backends.
The `reads`/`writes` metadata is consumed by downstream tools
(disassemblers, debuggers, future codegen waves) but does NOT change
emitted code. The performance claim "~2–5× slower than the
linear-scan backends" in caveat §2.1's Implication is therefore
**misleading** — there is no performance gap between the "real" and
"stack-slot" backends today, because no backend actually emits
register-based code. (A future wave could close this gap by adding a
real `emit_function_regalloc` path that produces different bytes; the
plumbing exists at `emit.rs:1056` but is unused in production for
non-test builds.)

### 5.2 Six backends have test-only `_real` greedy stubs

Six backends (`alpha`, `sparc64`, `hppa`, `s390x`, `m68k`, `mips64`)
plus `x86_32` have a `use_real_regalloc: bool` field on the backend
struct (default `false`), and the `allocate_registers` impl branches
on it:

```text
if self.use_real_regalloc {
    <isa>_allocate_registers_real(func)   // greedy stub
} else {
    <isa>_allocate_registers_ss(func)     // stack-slot (production)
}
```

The `_real` variants do NOT call `LinearScanAllocator` or
`TargetAgnosticRegAlloc`. They run the existing stack-slot allocator
(`*_allocate_registers_ss`) and then post-process the result with a
simple "first N vregs → first N caller-saved GPRs" greedy assignment
to populate the `reads`/`writes` metadata. This is a stub intended for
future linear-scan wiring, not a real linear-scan allocator.

`use_real_regalloc` is set to `true` ONLY in `#[cfg(test)]` modules
(see audit table rows 4, 10, 15-19 for the exact test line numbers).
In production builds all 7 backends run the pure stack-slot path.

### 5.3 Unused `emit_function_regalloc` plumbing

Five backends define `pub fn emit_function_regalloc(...)` that calls
`regalloc_emit::run_regalloc(func, "<isa>")` (which looks the ISA up
in `TargetDescRegistry` and runs `TargetAgnosticRegAlloc::new`):

| Backend | `emit_function_regalloc` | `run_regalloc` call | Called from `allocate_registers`? |
|---------|--------------------------|---------------------|-----------------------------------|
| `aarch64` (via `AArch64Backend`) | `backend.rs:2212` | `:2284` (`"aarch64"`) | NO — `allocate_registers` uses `try_real_regalloc` instead (`backend.rs:3249`) |
| `arm32` | `arm32/mod.rs:3465` | `:3480` (`"arm32"`) | NO — `allocate_registers` is pure stack-slot (`mod.rs:4485`) |
| `loongarch64` | `loongarch64/mod.rs:2590` | `:2605` (`"loongarch64"`) | NO — `allocate_registers` calls `stack_slot_isel::allocate_registers` (`mod.rs:2621`) |
| `x86_64` | `x86_64/mod.rs:3962` | `:3985` (`"x86_64"`) | NO — `allocate_registers` uses `try_real_regalloc` instead (`mod.rs:4149`) |
| `riscv64` | `riscv64.rs:4162` | `:4181` (`"riscv64"`) | NO — `allocate_registers` uses `try_real_regalloc` instead (`riscv64.rs:10306`) |

These methods exist as alternative entry points (used by tests at
`backend.rs:4884-4992`) but the production `allocate_registers` path
on each backend uses `try_real_regalloc` (aarch64/x86_64/riscv64/ppc64)
or pure stack-slot (arm32/loongarch64). The `arm32` and `loongarch64`
TargetDescs are therefore registered but never consulted by the
production `allocate_registers`.

### 5.4 TargetDesc registration coverage

`TargetDescRegistry::new()` (`target_desc.rs:1386-1396`) registers
exactly 8 ISAs: `aarch64`, `riscv64`, `wasm32`, `loongarch64`,
`x86_64`, `arm32`, `mips64`, `ppc64`. Each has a corresponding
`<isa>_target_desc()` constructor in `target_desc.rs` (lines 1420,
1562, 1706, 1788, 1932, 2046, 2170, 2320) that fully populates the
`registers`, `calling_convention`, `instruction_categories`, and
`latency_table` fields.

The remaining 11 ISAs (`aarch64_be`, `armeb`, `mips64be`, `ppc64le`,
`riscv32`, `x86_32`, `sparc64`, `s390x`, `m68k`, `alpha`, `hppa`)
have ONLY `LatencyTable::<isa>()` constructors (which populate just
the `latency_table` field — no register file, no ABI). They are NOT in
the registry.

The 4 big-endian / little-endian wrappers (`aarch64_be`, `armeb`,
`mips64be`, `ppc64le`) inherit their parent's full TargetDesc via
delegation at the `allocate_registers` level (they don't need their
own TargetDesc because register allocation is endianness-independent).
The 7 remaining ISAs (`riscv32`, `x86_32`, `sparc64`, `s390x`,
`m68k`, `alpha`, `hppa`) have no TargetDesc at all — they would need
one before `TargetAgnosticRegAlloc` could be wired up.

## 6. Audit Artifacts

- This file: `scripts/audit/allocator_classification.md`
- Source files inspected (READ-ONLY, no edits):
  - `src/codegen/src/regalloc.rs` (6 319 lines) — `LinearScanAllocator` (test-only) and `TargetAgnosticRegAlloc` (production)
  - `src/codegen/src/target_desc.rs` (3 018 lines) — `TargetDesc`, `TargetDescRegistry`, 8 full `<isa>_target_desc()` constructors, 19 `LatencyTable::<isa>()` constructors
  - `src/codegen/src/regalloc_emit.rs` (267 lines) — `run_regalloc`, `annotate_with_regalloc`
  - `src/codegen/src/backend.rs` (5 008 lines) — `AArch64Backend::allocate_registers` + `try_real_regalloc` (aarch64 path)
  - `src/codegen/src/emit.rs` (9 411 lines) — AArch64 `Emitter::emit_function` (production: stack-slot) + `emit_function_regalloc` (test-only entry)
  - `src/codegen/src/x86_64/mod.rs` (5 693 lines), `x86_64/stack_slot_isel.rs`, `x86_64/disasm.rs`
  - `src/codegen/src/riscv64.rs` (13 714 lines)
  - `src/codegen/src/ppc64/mod.rs` (7 223 lines), `ppc64/disasm.rs`
  - `src/codegen/src/ppc64le.rs` (594 lines)
  - `src/codegen/src/aarch64_be.rs` (233 lines)
  - `src/codegen/src/arm32/mod.rs` (12 211 lines), `arm32/disasm.rs`
  - `src/codegen/src/armeb.rs` (296 lines)
  - `src/codegen/src/riscv32.rs` (11 918 lines)
  - `src/codegen/src/x86_32/mod.rs` (4 812 lines), `x86_32/stack_slot_isel.rs`, `x86_32/disasm.rs`
  - `src/codegen/src/mips64/mod.rs` (12 211 lines), `mips64/disasm.rs`
  - `src/codegen/src/mips64be.rs` (364 lines)
  - `src/codegen/src/loongarch64/mod.rs` (6 948 lines), `loongarch64/stack_slot_isel.rs` (dead-code `reg_alloc_isel.rs`), `loongarch64/disasm.rs`
  - `src/codegen/src/wasm32/mod.rs` (8 827 lines), `wasm32/disasm.rs`
  - `src/codegen/src/sparc64.rs` (6 030 lines)
  - `src/codegen/src/s390x.rs` (4 239 lines)
  - `src/codegen/src/m68k.rs` (5 057 lines)
  - `src/codegen/src/alpha.rs` (5 317 lines)
  - `src/codegen/src/hppa.rs` (6 310 lines)
  - `docs/caveats.md` (§2.1, lines 44-67)
  - `docs/backends.md` (§1 Overview Table, §2 Stack-Slot ISel Pattern)

## 7. DoD Check

- [x] Markdown table exists at `scripts/audit/allocator_classification.md`
      with all 19 backends classified (§2 above).
- [x] Comparison against caveat §2.1 is documented (§4 above):
      4 directly-wired real backends MATCH (aarch64, x86_64, riscv64,
      ppc64); DRIFT on 3 wrapper/edge backends (aarch64_be, ppc64le,
      wasm32) + 1 minor naming drift on aarch64's allocator type.
- [x] No source files edited (READ-ONLY audit — verified by
      `git show --name-only HEAD` after commit will show only this
      markdown file added under `scripts/audit/`).
