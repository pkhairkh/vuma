# Follow-up Wave 2 — `emit_function_regalloc` Wire-up Design

- **Task ID:** F2-a-audit
- **Wave:** 2 (first sub-agent of the follow-up Wave 2 "Performance Gap Closure")
- **Audit type:** READ-ONLY design audit (no source files edited)
- **HEAD before audit:** `702b890e [F1-e-verify] add followup wave-1 DoD harness`
- **Prior run context:** Caveats-remediation Wave 2 audit (commit `83846368`,
  `scripts/audit/allocator_classification.md`) corrected the caveat §2.1
  4-real/15-stack-slot/1-wasm split to **6 real / 12 stack-slot / 1 Wasm**,
  and flagged that the real allocator runs **metadata-only** today — the
  `emit_function_regalloc` plumbing at `emit.rs:1056` is unused in production.
- **Scope (READ-ONLY):**
  `src/codegen/src/emit.rs`,
  `src/codegen/src/regalloc_emit.rs`,
  `src/codegen/src/regalloc.rs`,
  `src/codegen/src/backend.rs` (aarch64 path),
  `src/codegen/src/stack_slot_isel.rs` (and per-ISA variants),
  plus per-backend `emit_function_regalloc` definitions for the 6 "real" backends.

## 1. Current State

### 1.1 Two distinct methods share the name `emit_function_regalloc`

A subtle but critical finding: there are **two unrelated methods** named
`emit_function_regalloc` in the codegen crate.  They differ in receiver,
argument type, and — most importantly — in whether they change the encoded
machine-code bytes.

#### 1.1.A `Emitter::emit_function_regalloc` — AArch64, **byte-changing**

- **Location:** `src/codegen/src/emit.rs:1056`
- **Signature:**

  ```rust
  fn emit_function_regalloc(
      &mut self,
      func: &IRFunction,
      alloc: &AllocationResult,   // ← from LinearScanAllocator (regalloc.rs:480)
  ) -> Result<Vec<u32>>
  ```

- **Body summary** (read at `emit.rs:1056-1266`, 210 lines): consumes an
  `AllocationResult` and emits **register-based** AArch64 machine code:
  1. If `alloc` is empty on all four info fields (`used_callee_saved_gprs`,
     `spill_code`, `eliminated_copies`, `vreg_to_preg`), falls back to
     `emit_function_greedy` (byte-identical to the non-regalloc path) —
     `emit.rs:1066-1072`.
  2. Otherwise: resets emitter state (`emit.rs:1074-1081`), pre-assigns
     AAPCS64 argument registers X0–X7 (`emit.rs:1083-1102`), feeds
     `alloc.vreg_to_preg` into `RegAllocator::preassign` so `resolve_reg`
     returns the regalloc-assigned physical register rather than picking
     one from the caller-saved pool (`emit.rs:1104-1119`).
  3. Computes frame size = max(greedy_spill_area, alloc.total_spill_slots*8)
     + callee_saved_size (`emit.rs:1121-1146`).
  4. Emits callee-saved saves (`STP Xi, Xi+1, [SP, #k*16]`) before the
     standard FP/LR prologue (`emit.rs:1148-1175`).
  5. Spill area reservation (`emit.rs:1177-1199`).
  6. Per-instruction spill/reload via `alloc.spill_code.get(&P)` (where
     `P = 2*N`), inserting `LDR`/`STR` at `[X29, #slot.offset]`
     (`emit.rs:1017-1024` documents the contract).
  7. Skips `Cast` instructions whose `src` vreg is in
     `alloc.eliminated_copies` — coalescing made the move a no-op
     (`emit.rs:1025-1029`).
  8. Epilogue emits callee-saved restores before `RET`
     (`emit.rs:1030-1036`).

- **Callers (production):** ONE — `Emitter::emit_function(func, Some(alloc))`
  at `emit.rs:959-993`, dispatched at `emit.rs:965-967`:

  ```rust
  pub fn emit_function(
      &mut self,
      func: &IRFunction,
      alloc: Option<&AllocationResult>,
  ) -> Result<Vec<u32>> {
      if let Some(result) = alloc {
          return self.emit_function_regalloc(func, result);   // ← emit.rs:966
      }
      self.emit_function_stack_slot(func)                    // ← emit.rs:992
  }
  ```

- **Callers (production `allocate_registers`):** **ZERO.** The aarch64
  production path passes `None`:

  ```rust
  // backend.rs:3177-3184 (AArch64Backend::allocate_registers)
  let mut emitter = crate::emit::Emitter::new();
  let code =
      emitter
          .emit_function(func, None)                        // ← backend.rs:3180
          .map_err(|e| BackendError::RegisterAllocFailed { ... })?;
  ```

  So `emit_function_regalloc` (emit.rs:1056) is **unreachable in production**.
  The dispatch at `emit.rs:966` only fires when a caller passes `Some(alloc)`,
  and no production `allocate_registers` does so.

- **Callers in tests:** the only `Some(alloc)` production-style caller is
  `emit.rs:6787` (`emit_binary_with_regalloc_results` test harness), plus
  unit tests at `emit.rs:9228-9320` that explicitly verify the bytes
  differ from the greedy/stack-slot baseline.  The prior audit (§5.1)
  recorded this as: *"the `emit_function_regalloc` path in `emit.rs:1056`
  is reachable only when `emitter.emit_function(func, Some(alloc))` is
  called; `AArch64Backend::allocate_registers` calls
  `emitter.emit_function(func, None)` (`backend.rs:3180`), so the regalloc
  path is NOT taken."*

#### 1.1.B `<Backend>::emit_function_regalloc` — per-backend, **metadata-only**

Five backend-specific methods share the name but are unrelated to the
byte-changing emitter above:

| Backend | Location | Signature | What it actually does |
|---------|----------|-----------|----------------------|
| `AArch64Backend` | `backend.rs:2212-2277` | `(&self, func: &IRFunction, alloc: &RegAllocResult) -> Result<AllocatedFunction>` | Calls `Emitter::emit_function(func, None)` (stack-slot bytes, `backend.rs:2225`), then `regalloc_emit::annotate_with_regalloc` (`backend.rs:2274`). **Bytes unchanged.** |
| `X86_64Backend` | `x86_64/mod.rs:3962-3975` | (same shape) | Calls `stack_slot_isel::allocate_registers(func)?` (`:3969`), then `annotate_with_regalloc` (`:3972`). **Bytes unchanged.** |
| `RiscV64Backend` | `riscv64.rs:4162-4174` | (same shape) | Calls `self.allocate_registers(func)?` (which is itself the stack-slot path, `riscv64.rs:6607`), then `annotate_with_regalloc` (`:4171`). **Bytes unchanged.** |
| `Arm32Backend` | `arm32/mod.rs:3465-3473` | (same shape) | Calls `self.allocate_registers(func)?` (stack-slot, `mod.rs:4485`), then `annotate_with_regalloc` (`:3471`). **Bytes unchanged.** |
| `LoongArch64Backend` | `loongarch64/mod.rs:2590-2598` | (same shape) | Calls `self.allocate_registers(func)?` (which is `stack_slot_isel::allocate_registers(func)`, `mod.rs:2621-2623`), then `annotate_with_regalloc` (`:2596`). **Bytes unchanged.** |

(Ppc64 has no `emit_function_regalloc` method at all — its real-allocator
wiring is purely the `try_real_regalloc` + `annotate_with_regalloc` pattern
inside `allocate_registers` itself, `ppc64/mod.rs:3084-4863`.)

All five of these methods are misleadingly named: they are **metadata-only**
helpers, not byte-changing emitters.  None is called from production
`allocate_registers`; each is reachable only from the corresponding
`emit_function_with_regalloc` convenience method (e.g.
`backend.rs:2280-2286`), which is itself only called from
`#[cfg(test)]` modules (`backend.rs:4884-4992`).

### 1.2 `regalloc_emit::annotate_with_regalloc` — the metadata-only annotator

- **Location:** `src/codegen/src/regalloc_emit.rs:94-106`
- **Signature:** `fn annotate_with_regalloc(func: &mut AllocatedFunction, alloc: &RegAllocResult)`
- **What it does** (per `regalloc_emit.rs:73-93` and the implementation at
  `:94-147`): for each `AllocatedInstruction` in `func.blocks[*].instructions`,
  adds every entry of `alloc.used_callee_saved` to both `reads` and `writes`
  (deduplicated via `HashSet`).  Stores
  `alloc.total_spill_slots as usize` into `func.spill_slots`.
- **What it does NOT do** (explicitly documented at `regalloc_emit.rs:89-92`):
  *"The `encoded` bytes are NOT modified — they remain the stack-slot ISel's
  output."*  The annotation is **additive metadata** for downstream consumers
  (debuggers, disassemblers, future codegen waves).
- **Limitation** (acknowledged at `regalloc_emit.rs:126-134`): the
  implementation does not actually map per-instruction vregs to pregs,
  because *"the opcode string may contain vreg IDs … we can't easily parse
  these"*.  It only adds the callee-saved set to every instruction.  So even
  the metadata is coarse — `reads`/`writes` over-approximate rather than
  reflect the real per-instruction assignment.

### 1.3 The two allocator result types are NOT interchangeable

| Type | Source | Producer | Register enum |
|------|--------|----------|---------------|
| `AllocationResult` | `regalloc.rs:480-513` | `LinearScanAllocator::allocate_function` (`regalloc.rs:1318`, returns `Result<AllocationResult>`) | ARM64-specific `Register` / `SimdFpRegister` (from `arm64.rs`) |
| `RegAllocResult` | `regalloc.rs:3044-3059` | `TargetAgnosticRegAlloc::allocate_function` (`regalloc.rs:2673`, returns `Result<RegAllocResult, BackendError>`) | Target-agnostic `backend::PhysicalReg` (with `RegClass::Gpr`/`SimdFp`) |

`LinearScanAllocator` is **AArch64-only**: its `new()` (`regalloc.rs:1229`)
hardcodes the X0–X28 GPR pool and V0–V31 SIMD pool.  It is invoked only in
`#[cfg(test)]` modules (`regalloc.rs:4738+`, `emit.rs:9188+`) — the prior
audit's **DRIFT-4** finding.

`TargetAgnosticRegAlloc` is the production allocator used by all 6 "real"
backends' `try_real_regalloc` helpers.  It produces `RegAllocResult`, which
**cannot be fed directly to `Emitter::emit_function_regalloc`** (which
requires `&AllocationResult`).

### 1.4 Production `allocate_registers` flow on each of the 6 "real" backends

| Backend | Production `allocate_registers` | Real allocator call | Annotation |
|---------|---------------------------------|---------------------|------------|
| `aarch64` | `backend.rs:3162` → `emitter.emit_function(func, None)` (`:3180`) | `try_real_regalloc(func)` (`backend.rs:3249`, `TargetAgnosticRegAlloc::new` at `:3114`) | `annotate_with_regalloc(&mut allocated, &alloc)` (`backend.rs:3250`) |
| `x86_64` | `x86_64/mod.rs:4141` → `stack_slot_isel::allocate_registers(func)?` (`:4143`) | `try_real_regalloc(func)` (`:4149`, `TargetAgnosticRegAlloc::new` at `:4081`) | `annotate_with_regalloc` (`:4150`) |
| `riscv64` | `riscv64.rs:6607` (inline stack-slot ISel) | `try_real_regalloc(func)` (`riscv64.rs:10306`, `TargetAgnosticRegAlloc::new` at `:6557`) | `annotate_with_regalloc` (`:10307`) |
| `ppc64` | `ppc64/mod.rs:3084` (inline stack-slot ISel) | `try_real_regalloc(func)` (`:4864`, `TargetAgnosticRegAlloc::new` at `:3026`) | `annotate_with_regalloc` (`:4865`) |
| `aarch64_be` | `aarch64_be.rs:150-152` delegates to `self.inner.allocate_registers(func)` | inherits aarch64 | inherits aarch64 |
| `ppc64le` | `ppc64le.rs:400-406` delegates to `self.inner.allocate_registers(func)` | inherits ppc64 | inherits ppc64 |

In **every** case, the encoded bytes come from the stack-slot ISel path; the
real allocator's output is consumed only by `annotate_with_regalloc`, which
does not change `encoded`.  This is the metadata-only state documented in
caveat §2.1's "Metadata-only caveat (critical)" paragraph (added by Wave 2
task 2-d-doc).

## 2. What `emit_function_regalloc` Needs to Do

Closing the performance gap requires the **byte-changing** semantics of
`Emitter::emit_function_regalloc` (§1.1.A) to be reached in production.
Concretely, on each of the 6 real backends the production
`allocate_registers` must:

1. **Run a real linear-scan allocator** that produces register assignments
   AND spill code AND callee-saved-set information — not just per-vreg
   physical-register hints.
2. **Feed the allocator's result into a byte-changing emitter** that:
   - emits a prologue that saves the callee-saved registers the allocator
     marked as used;
   - resolves each vreg operand to its assigned physical register at
     encode time (instead of loading from a stack slot into a scratch
     register);
   - inserts `LDR`/`STR` (or ISA-equivalent) spill/reload instructions
     at the positions the allocator's `spill_code` map indicates;
   - skips `Cast`/move instructions the allocator coalesced away;
   - emits an epilogue that restores the callee-saved registers.
3. **Fall back to the stack-slot baseline** if the real allocator fails or
   the target description is missing (preserving today's correctness
   guarantee).

For **aarch64 only**, all of this machinery already exists: the
`Emitter::emit_function_regalloc` at `emit.rs:1056` does exactly steps
2.a–2.e, and the `LinearScanAllocator` at `regalloc.rs:1219` produces the
`AllocationResult` it needs.  The only missing piece is wiring them
together in `backend.rs:3180` (passing `Some(&alloc)` instead of `None`).

For **x86_64, riscv64, ppc64**, no byte-changing register-based emitter
exists today — each backend's emission is the stack-slot ISel
(`stack_slot_isel::allocate_registers` or an inline equivalent).  Closing
the gap on these ISAs requires **writing a new register-based emitter per
ISA** that consumes the allocator result (either `RegAllocResult` or a
new per-ISA `AllocationResult`-equivalent).

For **aarch64_be, ppc64le**, no extra work is needed beyond their parent
backends — they delegate.

## 3. The Gap (code paths to change, with line numbers)

### 3.1 aarch64 — smallest gap (plumbing only)

| File:line | Current code | Required change |
|-----------|--------------|-----------------|
| `backend.rs:3177-3184` | `let mut emitter = Emitter::new(); emitter.emit_function(func, None)?` | Run `LinearScanAllocator::new().allocate_function(func)` first; on `Ok(alloc)` call `emitter.emit_function(func, Some(&alloc))?`; on `Err` fall back to `emit_function(func, None)`. |
| `backend.rs:3249-3251` | `if let Some(alloc) = try_real_regalloc(func) { annotate_with_regalloc(&mut allocated, &alloc); }` | The annotation step is still useful (the `RegAllocResult` carries `used_callee_saved` as `PhysicalReg`, which the metadata path consumes).  Keep as-is.  The `AllocationResult` (separate type) feeds the byte path; the `RegAllocResult` feeds the metadata path.  Both can run in parallel. |
| `regalloc.rs:1219-1318` | `LinearScanAllocator::allocate_function` exists, returns `Result<AllocationResult>`, but is `#[cfg(test)]`-only in practice (per DRIFT-4) | No code change needed — the method is already `pub`; it just needs a production caller.  (Verify it's not gated by `#[cfg(test)]` at the impl block level — spot-check confirms the impl at `:1219` is NOT gated, only some of its callers are.) |

### 3.2 x86_64 — needs new register-based emitter

| File:line | Current code | Required change |
|-----------|--------------|-----------------|
| `x86_64/mod.rs:4141-4154` | `allocate_registers` calls `stack_slot_isel::allocate_registers(func)?` (`:4143`) then annotates | Add a new `x86_64::reg_isel::allocate_registers(func, &alloc)` that consumes a `RegAllocResult` (or a new x86_64-specific `AllocationResult`) and emits register-based x86_64 machine code (mov-into-reg / op-in-reg / spill-reload).  Branch on `try_real_regalloc(func)` success. |
| `x86_64/stack_slot_isel.rs:1-4513` | 4 513-line stack-slot emitter (every vreg → stack slot, load/store per operand) | No change — kept as the fallback path.  The new `reg_isel.rs` would live alongside it. |
| `x86_64/mod.rs:3962-3975` | `<X86_64Backend>::emit_function_regalloc` is metadata-only | Either repurpose this method to call the new `reg_isel::allocate_registers` (changing its semantics from "annotate" to "emit register-based bytes"), OR leave it as-is and add a new method with a clearer name.  Recommended: rename the existing one to `emit_function_with_regalloc_metadata` and reserve `emit_function_regalloc` for the byte-changing path, to match the `Emitter` (emit.rs:1056) naming. |

### 3.3 riscv64 — needs new register-based emitter

| File:line | Current code | Required change |
|-----------|--------------|-----------------|
| `riscv64.rs:6607-10305` | `allocate_registers` is a 3 700-line inline stack-slot ISel | Extract a `riscv64_reg_isel::allocate_registers(func, &alloc)` that emits register-based RVI/M-extension code from a `RegAllocResult`.  Branch on `try_real_regalloc(func)` (`:10306`). |
| `riscv64.rs:4162-4174` | metadata-only `emit_function_regalloc` | Same rename / re-purpose recommendation as x86_64. |

### 3.4 ppc64 — needs new register-based emitter

| File:line | Current code | Required change |
|-----------|--------------|-----------------|
| `ppc64/mod.rs:3084-4863` | `allocate_registers` is a 1 780-line inline stack-slot ISel | Extract `ppc64_reg_isel::allocate_registers(func, &alloc)`.  Branch on `try_real_regalloc(func)` (`:4864`). |
| (no `emit_function_regalloc` method exists for ppc64 today) | n/a | Add one (matching the new byte-changing semantics) for API symmetry with the other 5 real backends. |

### 3.5 aarch64_be, ppc64le — no direct work

| File:line | Current code | Required change |
|-----------|--------------|-----------------|
| `aarch64_be.rs:150-152` | `self.inner.allocate_registers(func)` | None — inherits aarch64's fix. |
| `ppc64le.rs:400-406` | `self.inner.allocate_registers(func)` | None — inherits ppc64's fix. |

### 3.6 Aggregate call-site grep (sanity check)

`rg "emit_function\(func, " src/codegen/src/` returns 5 hits:
- `emit.rs:6787` — test harness (`Some(alloc)`).
- `emit.rs:7249` — test harness (`None`).
- `backend.rs:2225` — `AArch64Backend::emit_function_regalloc` (metadata-only, `None`).
- `backend.rs:3180` — **production** `AArch64Backend::allocate_registers` (`None`).  ← the one to change.
- `emit.rs:5611` — comment only.

So the production surface for aarch64 is **one line** (`backend.rs:3180`).
For x86_64/riscv64/ppc64 the production surface is the entire
`allocate_registers` body (multi-thousand-line stack-slot ISels that need a
register-based sibling).

## 4. Per-Backend Readiness (for each of 6 real backends)

| # | Backend | TargetDesc complete? | Byte-changing emitter exists? | Allocator produces compatible result? | Readiness |
|--:|---------|----------------------|-------------------------------|---------------------------------------|-----------|
| 1 | `aarch64` | ✅ Full, registered (`target_desc.rs:1420`, registry `:1388`) | ✅ `Emitter::emit_function_regalloc` (`emit.rs:1056`) | ⚠️ `LinearScanAllocator::allocate_function` (`regalloc.rs:1318`) produces `AllocationResult` (ARM64-specific).  `TargetAgnosticRegAlloc::allocate_function` (`regalloc.rs:2673`) produces `RegAllocResult` (target-agnostic).  The byte-changing emitter requires the **former**; the production `try_real_regalloc` produces the **latter**.  Two options: (a) wire `LinearScanAllocator` into production (it's already AArch64-hardcoded, so no portability concern), or (b) write a `RegAllocResult → AllocationResult` adapter.  Option (a) is simpler. | **HIGH** — plumbing complete; one-line wire-up + a fallback path. |
| 2 | `aarch64_be` | Inherits aarch64 | Inherits aarch64 | Inherits aarch64 | **HIGH** — automatic with aarch64. |
| 3 | `x86_64` | ✅ Full, registered (`target_desc.rs:1932`, registry `:1392`) | ❌ Only `stack_slot_isel::allocate_registers` (`x86_64/stack_slot_isel.rs:1-4513`); no register-based emitter | ✅ `try_real_regalloc` (`x86_64/mod.rs:4081`) produces `RegAllocResult` via `TargetAgnosticRegAlloc` | **LOW** — needs a new register-based x86_64 emitter (likely 2–3 KLOC). |
| 4 | `riscv64` | ✅ Full, registered (`target_desc.rs:1562`, registry `:1389`) | ❌ Only inline stack-slot ISel (`riscv64.rs:6607-10305`) | ✅ `try_real_regalloc` (`riscv64.rs:6542`) produces `RegAllocResult` | **LOW** — needs a new register-based riscv64 emitter. |
| 5 | `ppc64` | ✅ Full, registered (`target_desc.rs:2320`, registry `:1395`) | ❌ Only inline stack-slot ISel (`ppc64/mod.rs:3084-4863`) | ✅ `try_real_regalloc` (`ppc64/mod.rs:4864`-area) produces `RegAllocResult` | **LOW** — needs a new register-based ppc64 emitter. |
| 6 | `ppc64le` | Inherits ppc64 | Inherits ppc64 | Inherits ppc64 | **LOW** — automatic with ppc64 (but blocked on ppc64). |

**Summary:** only `aarch64` (+ `aarch64_be`) has all the pieces in place
today.  The other 4 backends need significant new emitter code.

## 5. Risk Assessment

### 5.1 aarch64 wire-up risk: **MEDIUM**

The `Emitter::emit_function_regalloc` path at `emit.rs:1056` is exercised
only by unit tests (`emit.rs:9228-9320`, `emit.rs:6787`).  Production IR
has more complex shapes than the test fixtures — nested loops, `Alloc`
pointers surviving across control-flow edges, syscalls, indirect calls.
The existing comment at `emit.rs:971-983` records exactly this kind of
hazard on the greedy emitter (`mem_copy_buffer.vuma` SIGSEGV: a vreg
defined on a loop-exit path was read on the next loop's entry, and the
greedy allocator returned a stale register).  The regalloc emitter trusts
`AllocationResult.vreg_to_preg` and `spill_code` to be correct; if
`LinearScanAllocator` has a similar stale-interval bug on production IR,
switching the production path to `Some(alloc)` will re-introduce the
SIGSEGV cluster.

**Mitigation:** gate the wire-up behind `VUMA_REAL_REGALLOC_AARCH64=1`
env var (default off).  Run the full curated 39-test subset
(`scripts/audit/wave2_stackslot_results.md`) under QEMU with the flag on
before flipping the default.

### 5.2 `LinearScanAllocator` vs `TargetAgnosticRegAlloc` divergence risk: **MEDIUM**

These are two separate linear-scan implementations:
- `LinearScanAllocator` (`regalloc.rs:1219`, AArch64-hardcoded) produces
  `AllocationResult` with ARM64 `Register` enums, `BTreeMap<u32, Vec<SpillCode>>`
  spill code, and `eliminated_copies` coalescing info.
- `TargetAgnosticRegAlloc` (`regalloc.rs:2575`, target-agnostic) produces
  `RegAllocResult` with `PhysicalReg`, `BTreeMap<u32, Vec<GenericSpillCode>>`,
  no `eliminated_copies`.

Wiring `LinearScanAllocator` into aarch64 production means the metadata
path (which uses `RegAllocResult` via `try_real_regalloc`) and the byte
path (which uses `AllocationResult`) would be **two different allocators**
running on the same function.  Their decisions could disagree, leading to
the metadata claiming a vreg is in register R5 while the bytes have it in
R6.  This is benign for execution (the bytes are what runs) but breaks
debuggers / disassemblers that trust `reads`/`writes`.

**Mitigation:** either (a) make the byte path also use
`TargetAgnosticRegAlloc` (requires writing a `RegAllocResult →
AllocationResult` adapter, ~200 LOC), or (b) make the metadata path also
use `LinearScanAllocator` on aarch64 (drop `try_real_regalloc` for
aarch64).  Option (b) is simpler but loses the target-agnostic coverage
that `TargetAgnosticRegAlloc` was added for.

### 5.3 Callee-saved register save/restore correctness risk: **HIGH**

`Emitter::emit_function_regalloc` emits `STP Xi, Xi+1, [SP, #k*16]` pairs
in the prologue (`emit.rs:1148-1154`) and matching `LDP` restores in the
epilogue.  If the allocator's `used_callee_saved_gprs` set is wrong (e.g.
missing a register that a spilled-reload path uses as a scratch), the
epilogue will restore garbage into a callee-saved register and the caller
will see corruption.  This is a **silent correctness bug** that QEMU
smoke tests on small fixtures may not catch.

**Mitigation:** add a verifier pass that walks each
`AllocatedInstruction`'s `reads`/`writes`/`encoded` and asserts that
every physical register used is either (i) caller-saved, (ii) in
`used_callee_saved_gprs`, or (iii) `X29`/`X30`/`SP` (handled by the
standard prologue).  Run the verifier in `#[cfg(test)]` and as an
optional `VUMA_VERIFY_CALLEE_SAVED=1` production check.

### 5.4 Stack-frame layout drift risk: **MEDIUM**

The byte-changing emitter computes `frame_size = spill_area_aligned +
callee_saved_size` (`emit.rs:1146`).  Today `aarch64_compute_frame_size`
(`backend.rs:2254`-area, called from `allocate_registers` at `:3187`)
computes a **different** frame size that doesn't include the callee-saved
area.  If `allocate_registers` is changed to call the byte-changing
emitter, the `frame_size` stored in `AllocatedFunction` (`backend.rs:3236`)
must match what the emitter actually used, or stack slot offsets in
debug info / unwind info will be wrong.

**Mitigation:** have the byte-changing emitter return its computed
`frame_size` alongside the `Vec<u32>`, and use that in the
`AllocatedFunction` construction (replacing `aarch64_compute_frame_size`).

### 5.5 Test-only `_real` greedy stubs on 7 stack-slot backends: **LOW (out of scope)**

The 7 stack-slot backends with `use_real_regalloc` flags
(`x86_32`, `mips64`, `sparc64`, `s390x`, `m68k`, `alpha`, `hppa`) have
greedy stubs that the prior audit (§5.2) called out as **not real
linear-scan**.  This audit does **not** recommend touching them — they
lack TargetDescs entirely, so wiring them up requires adding 7 new
TargetDescs first, which is a separate effort.

## 6. Phased Rollout Plan

### Phase 1 — aarch64 wire-up (smallest gap, highest payoff)

1. Add `VUMA_REAL_REGALLOC_AARCH64` env var (default `0`) checked in
   `AArch64Backend::allocate_registers` (`backend.rs:3162`).
2. When set: run `LinearScanAllocator::new().allocate_function(func)`; on
   `Ok(alloc)` call `emitter.emit_function(func, Some(&alloc))?`; on `Err`
   fall back to `emit_function(func, None)?` (today's path).
3. When unset: today's `emit_function(func, None)?` path (unchanged).
4. Run the curated 39-test subset under QEMU with the flag on.  Expect
   failures in loop-heavy fixtures (`mem_copy_buffer.vuma` and friends);
   triage and fix `LinearScanAllocator` bugs.
5. Once green for 100% of the curated subset, flip the default to `1`.
6. Update `docs/caveats.md` §2.1 "Metadata-only caveat (critical)" to
   reflect that aarch64 now emits register-based bytes; keep the caveat
   for the other 5 real backends.

**Estimated effort:** 1–2 weeks (1 wire-up PR + 1+ bug-fix PRs).

### Phase 2 — x86_64 register-based emitter

1. Create `src/codegen/src/x86_64/reg_isel.rs` modeled on
   `stack_slot_isel.rs` but consuming `&RegAllocResult`.
2. Implement per-IR-instruction arms: `BinOp` → `mov rX, [stack]; op rX, rY; mov [stack], rX` becomes `op rX, rY` (direct register form).
3. Implement spill/reload insertion from `alloc.spill_code`.
4. Implement callee-saved prologue/epilogue (`push rbx; push r12; ...` /
   `pop ...; pop rbx`) from `alloc.used_callee_saved`.
5. Gate behind `VUMA_REAL_REGALLOC_X86_64=1`.
6. Run curated subset under `qemu-x86_64-static`.

**Estimated effort:** 3–4 weeks.

### Phase 3 — riscv64 register-based emitter

Similar to Phase 2 but for RV64I/M/F.  The inline stack-slot ISel at
`riscv64.rs:6607-10305` is 3 700 lines, so the register-based sibling
will likely be 2–3 KLOC.

**Estimated effort:** 3–4 weeks.

### Phase 4 — ppc64 register-based emitter

Similar to Phase 2 but for PowerISA 3.0B.  Inline stack-slot ISel at
`ppc64/mod.rs:3084-4863` is 1 780 lines.

**Estimated effort:** 2–3 weeks.

### Phase 5 — aarch64_be, ppc64le

Automatic with Phases 1 and 4.  Verify under `qemu-aarch64_be-static` and
`qemu-ppc64le-static`.

**Estimated effort:** 1 day each (smoke testing only).

### Phase 6 (out of scope for this design) — 12 stack-slot backends

For the 12 stack-slot backends (`arm32`, `armeb`, `mips64`, `mips64be`,
`riscv32`, `x86_32`, `loongarch64`, `sparc64`, `s390x`, `m68k`, `alpha`,
`hppa`): add 7 missing TargetDescs (`riscv32`, `x86_32`, `sparc64`,
`s390x`, `m68k`, `alpha`, `hppa`), then write register-based emitters.
This is a multi-quarter effort and should be its own design document.

## 7. Concrete Code Changes (specific functions to modify, with before/after sketches)

### 7.1 `AArch64Backend::allocate_registers` — aarch64 wire-up

**File:** `src/codegen/src/backend.rs:3162-3254`

**Before** (excerpt, `backend.rs:3175-3184`):

```rust
// Use the existing Emitter to emit the function, which internally
// performs register allocation and instruction encoding.
let mut emitter = crate::emit::Emitter::new();
let code =
    emitter
        .emit_function(func, None)                              // ← :3180
        .map_err(|e| BackendError::RegisterAllocFailed {
            isa: "aarch64",
            reason: e.to_string(),
        })?;
```

**After** (sketch):

```rust
let mut emitter = crate::emit::Emitter::new();

// Phase 1 of F2-a: opt-in real register allocation on aarch64.
// The byte-changing Emitter::emit_function_regalloc path at
// emit.rs:1056 requires an AllocationResult from LinearScanAllocator
// (regalloc.rs:1318).  When the env var is unset (default), fall back
// to the stack-slot path (today's behaviour).
let real_regalloc = std::env::var("VUMA_REAL_REGALLOC_AARCH64")
    .map(|v| v == "1")
    .unwrap_or(false);

let code = if real_regalloc {
    let allocator = crate::regalloc::LinearScanAllocator::new();
    match allocator.allocate_function(func) {
        Ok(alloc) => emitter.emit_function(func, Some(&alloc))?,   // ← byte-changing
        Err(e) => {
            vuma_log!(warn, "aarch64 LinearScanAllocator failed for '{}': {}, falling back to stack-slot", func.name, e);
            emitter.emit_function(func, None)?                     // ← fallback
        }
    }
} else {
    emitter.emit_function(func, None)?                             // ← today's path
};
```

(`?` propagates `emit::EmitError`; the surrounding `.map_err(|e| BackendError::RegisterAllocFailed { isa: "aarch64", reason: e.to_string() })?` wrapper stays unchanged.)

**No change** to the `try_real_regalloc(func)` + `annotate_with_regalloc` step
at `backend.rs:3249-3251` — the metadata path is independent and remains
correct (see §5.2 for the divergence-risk caveat and the recommended
option-(b) mitigation).

### 7.2 `Emitter::emit_function` — frame-size return

**File:** `src/codegen/src/emit.rs:959-993`

**Problem:** today `emit_function` returns `Result<Vec<u32>>` and
`AArch64Backend::allocate_registers` computes `frame_size` separately via
`aarch64_compute_frame_size(func)` (`backend.rs:3187`).  When the
byte-changing path runs, the actual frame size depends on
`alloc.used_callee_saved_gprs.len()` and `alloc.total_spill_slots`
(computed at `emit.rs:1146`), so `aarch64_compute_frame_size` will be
wrong.

**Recommended change** (sketch — would be a separate PR):

```rust
pub struct EmitResult {
    pub code: Vec<u32>,
    pub frame_size: u32,
    pub callee_saved: Vec<Register>,
}

pub fn emit_function(
    &mut self,
    func: &IRFunction,
    alloc: Option<&AllocationResult>,
) -> Result<EmitResult> {
    if let Some(result) = alloc {
        let code = self.emit_function_regalloc(func, result)?;
        return Ok(EmitResult {
            code,
            frame_size: self.frame_size,             // ← set at emit.rs:1146
            callee_saved: self.callee_saved_used.clone(),
        });
    }
    let code = self.emit_function_stack_slot(func)?;
    Ok(EmitResult { code, frame_size: self.frame_size, callee_saved: vec![] })
}
```

Then `AArch64Backend::allocate_registers` consumes `EmitResult.frame_size`
instead of `aarch64_compute_frame_size(func)`.  This is a **breaking API
change** — all callers of `emit_function` (including the test harness at
`emit.rs:6787`, `:7249`) need updating.  Out of scope for the F2-a wire-up
PR; should be its own PR.

### 7.3 `x86_64` — new `reg_isel.rs` module (sketch)

**File (new):** `src/codegen/src/x86_64/reg_isel.rs`

**Public API:**

```rust
pub fn allocate_registers(
    func: &IRFunction,
    alloc: &crate::regalloc::RegAllocResult,
) -> Result<AllocatedFunction, BackendError> {
    // 1. Walk func.blocks[*].instructions.
    // 2. For each IRInstr, look up dst/lhs/rhs vregs in alloc.vreg_to_preg
    //    (or alloc.spill_slots).
    // 3. Emit register-form x86_64 encodings (encode_add_reg_reg,
    //    encode_mov_reg_reg, etc.) using the assigned physical registers.
    // 4. Insert spill_code (LDR/STR equivalent: mov [rbp-offset], rX /
    //    mov rX, [rbp-offset]) at positions from alloc.spill_code.
    // 5. Emit prologue: push rbp; mov rbp, rsp; sub rsp, frame_size;
    //    push rbx/r12-r15 (from alloc.used_callee_saved).
    // 6. Emit epilogue: pop r15-r12/rbx; mov rsp, rbp; pop rbp; ret.
    todo!()
}
```

**Wire-up in `x86_64/mod.rs:4141-4154`** (sketch):

```rust
fn allocate_registers(&self, func: &IRFunction) -> Result<AllocatedFunction, BackendError> {
    let real_regalloc = std::env::var("VUMA_REAL_REGALLOC_X86_64")
        .map(|v| v == "1")
        .unwrap_or(false);

    let mut allocated = if real_regalloc {
        if let Some(alloc) = try_real_regalloc(func) {
            reg_isel::allocate_registers(func, &alloc)?
        } else {
            stack_slot_isel::allocate_registers(func)?    // ← fallback
        }
    } else {
        stack_slot_isel::allocate_registers(func)?       // ← today's path
    };

    // Metadata annotation is still useful even when bytes are register-based
    // (it populates reads/writes from a separate allocator pass for consumers
    // that want the target-agnostic view).  Keep as-is.
    if let Some(alloc) = try_real_regalloc(func) {
        crate::regalloc_emit::annotate_with_regalloc(&mut allocated, &alloc);
    }

    Ok(allocated)
}
```

### 7.4 riscv64, ppc64 — analogous to 7.3

The same pattern: new `reg_isel` module per ISA, env-var gate, fallback to
stack-slot.  See §6 Phases 3 and 4.

### 7.5 Rename existing metadata-only `emit_function_regalloc` methods

To resolve the §1.1 naming collision:

| File:line | Current name | Proposed new name |
|-----------|--------------|-------------------|
| `backend.rs:2212` | `AArch64Backend::emit_function_regalloc` | `emit_function_with_regalloc_metadata` |
| `x86_64/mod.rs:3962` | `X86_64Backend::emit_function_regalloc` | `emit_function_with_regalloc_metadata` |
| `riscv64.rs:4162` | `RiscV64Backend::emit_function_regalloc` | `emit_function_with_regalloc_metadata` |
| `arm32/mod.rs:3465` | `Arm32Backend::emit_function_regalloc` | `emit_function_with_regalloc_metadata` |
| `loongarch64/mod.rs:2590` | `LoongArch64Backend::emit_function_regalloc` | `emit_function_with_regalloc_metadata` |

The `emit_function_with_regalloc` convenience methods (e.g.
`backend.rs:2280-2286`) keep their names but call the renamed
`_metadata` variant.  This frees the `emit_function_regalloc` name to be
re-used for byte-changing entry points on x86_64/riscv64/ppc64 (matching
the `Emitter` (emit.rs:1056) naming).

This is a **non-breaking rename** within the codegen crate (these methods
are `pub` but only called from `#[cfg(test)]` modules — verified by grep
at §1.1.B).

## DoD Check

- [x] Design doc exists at
      `/home/z/my-project/vuma/scripts/audit/followup_wave2_emit_regalloc_design.md`.
- [x] All 7 required sections present: §1 Current State, §2 What
      emit_function_regalloc Needs to Do, §3 The Gap, §4 Per-Backend
      Readiness, §5 Risk Assessment, §6 Phased Rollout Plan, §7 Concrete
      Code Changes.
- [x] Concrete line numbers cited for every code path to change
      (§3.1: `backend.rs:3180`; §3.2: `x86_64/mod.rs:4143`; §3.3:
      `riscv64.rs:6607`; §3.4: `ppc64/mod.rs:3084`; §3.5: `aarch64_be.rs:150`,
      `ppc64le.rs:400`; §7.1 before/after sketch with `backend.rs:3175-3184`;
      §7.2 `emit.rs:959-993`).
- [x] No source files edited (READ-ONLY audit — `git status --short`
      shows only this markdown file added under `scripts/audit/`).
