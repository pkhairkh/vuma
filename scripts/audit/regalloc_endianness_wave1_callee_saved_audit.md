# R1-a-audit — Wave 1 Callee-Saved Register Fix (READ-ONLY Audit)

- **Task ID:** R1-a-audit
- **Wave:** 1
- **Prior-run context:** F2-b-impl (`ee06b362`) wired `emit_function_regalloc`
  for aarch64 behind env-var gate `VUMA_REAL_REGALLOC_AARCH64=1`
  (default OFF). F2-c-test (`95a2963e`) found 8 regressions on
  callee-saved-register-pressure tests. Design doc §5.3 (`7083e1c7`)
  flagged this as **HIGH** risk: `LinearScanAllocator::used_callee_saved_gprs`
  incomplete. This wave fixes it.
- **HEAD before this task:** `c08e8c40 [regalloc-endianness-wave-0-dod-pass]`
- **Scope:** READ-ONLY audit (no source files edited).

## 1. Current `used_callee_saved_gprs` Implementation

`used_callee_saved_gprs` is **not a method** — it is a public field on the
`AllocationResult` struct returned by `LinearScanAllocator::allocate_function`.

### 1.1 Declaration

**File:** `src/codegen/src/regalloc.rs:492`

```rust
pub struct AllocationResult {
    // ...
    /// Set of callee-saved GPRs that must be saved/restored in prologue/epilogue.
    pub used_callee_saved_gprs: HashSet<Register>,           // ← :492
    /// Set of callee-saved SIMD/FP registers that must be saved/restored.
    pub used_callee_saved_simd: HashSet<SimdFpRegister>,     // ← :494
    // ...
}
```

There is no `LinearScanAllocator::used_callee_saved_gprs` accessor method
(the legacy `RegAllocator::used_callee_saved()` at `regalloc.rs:2412` returns
`Vec<Register>` and is unrelated — it backs the greedy emitter path, not the
linear-scan path).

### 1.2 Population sites (production path)

The set is populated **only** by `LinearScanAllocator::assign_gpr`
(`regalloc.rs:1446-1458`) on every GPR assignment:

```rust
fn assign_gpr(interval: &LiveInterval, preg: Register, result: &mut AllocationResult) {
    let phys = PhysReg::Gpr(preg);
    result.vreg_to_preg.insert(interval.vreg, phys);
    if preg.is_callee_saved() {
        result.used_callee_saved_gprs.insert(preg);          // ← :1450
    }
    // ... coalesced-vreg re-mapping ...
}
```

`assign_gpr` is called from `LinearScanAllocator::allocate_intervals`
(`regalloc.rs:1422` and `:1434`) — which is in turn called by
`allocate_function` (`regalloc.rs:1345`). So the production call-graph is:

```
allocate_function (regalloc.rs:1318)
  └─ allocate_intervals (regalloc.rs:1378)
       ├─ try_alloc_gpr (regalloc.rs:1476)
       │    └─ spill_gpr (regalloc.rs:1518)  ← may remove from the set (see 1.3)
       └─ assign_gpr (regalloc.rs:1446)      ← inserts into the set
```

The `is_callee_saved()` predicate (`arm64.rs:362-376`) returns `true` only for
`X19..=X28` — exactly the 10 GPRs in `LinearScanAllocator::callee_saved_gprs`
pool (`regalloc.rs:1248-1259`). So the set correctly contains every
callee-saved GPR **assigned to a vreg**.

### 1.3 Depopulation sites

The set is removed-from in exactly two places:

1. **Eviction in `spill_gpr`** (`regalloc.rs:1588`):

   ```rust
   let (evict_vreg, evict_reg, evict_end, evict_weight) = active[evict_idx];
   // ...
   result.vreg_to_preg.remove(&evict_vreg);
   result.used_callee_saved_gprs.remove(&evict_reg);          // ← :1588
   // ... gen_eviction_spill_reload(evict_vreg, PhysReg::Gpr(evict_reg), ...) ...
   active.push((interval.vreg, evict_reg, interval.end, interval.weight_per_length()));
   Ok(Some(evict_reg))                                          // ← returned to assign_gpr
   ```

   The evicted register is then immediately re-assigned to the new `interval`
   via `assign_gpr` back in `allocate_intervals` (`:1422`), which re-inserts
   it into `used_callee_saved_gprs` if it is callee-saved. So the net effect
   on the set across an eviction is **zero** when both intervals end up on a
   callee-saved register. This is correct.

2. **Copy-coalescing reassignment in `coalesce_copies_post_alloc`**
   (`regalloc.rs:2049-2070`):

   ```rust
   // Remove dst_preg from callee-saved tracking (it may no longer be used).
   match dst_preg {
       PhysReg::Gpr(r) => { result.used_callee_saved_gprs.remove(&r); }      // ← :2049
       PhysReg::SimdFp(r) => { result.used_callee_saved_simd.remove(&r); }
   }
   result.vreg_to_preg.insert(dst_id, src_preg);
   // Insert src_preg to callee-saved tracking if it's callee-saved.
   match src_preg {
       PhysReg::Gpr(r) => { if r.is_callee_saved() {
           result.used_callee_saved_gprs.insert(r);                          // ← :2062
       } }
       // ...
   }
   ```

   **Critical observation:** `coalesce_copies_post_alloc` is **never called
   from the production path.** It is only called from two `#[cfg(test)]`
   tests (`regalloc.rs:5511`, `:5575`). `allocate_function` (`:1318-1352`)
   does NOT invoke it. The doc comment at `:1913` shows the intended call
   pattern (`let mut result = alloc.allocate_function(&func)?; let eliminated
   = alloc.coalesce_copies_post_alloc(&func, &mut result);`) — but the
   caller in `backend.rs:3212` only does the first line.

   **Net effect:** in production, the callee-saved set is populated only by
   `assign_gpr` and depopulated only by `spill_gpr` (eviction path). The
   coalescing code paths at `:2049`/`:2062` are dead in production but
   remain a latent correctness hazard if `coalesce_copies_post_alloc` is
   ever wired up (see §3 gap G4).

## 2. `emit_function_regalloc` Prologue/Epilogue Behavior

**File:** `src/codegen/src/emit.rs:1056-1277`

The byte-changing emitter **does** emit prologue saves and epilogue
restores for every register in `alloc.used_callee_saved_gprs`. It is
NOT skipping them. The relevant code paths:

### 2.1 Prologue (callee-saved saves)

`emit_function_regalloc` at `emit.rs:1148-1154`:

```rust
// ── Prologue ──────────────────────────────────────────────────
if callee_saved_count > 0 {
    self.emit_callee_saved_saves(&alloc.used_callee_saved_gprs, callee_saved_size)?;
}
// Standard FP/LR save: SUB SP, SP, #16 / STP X29, X30, [SP] / ADD X29, SP, #0
self.emit_instruction(Instruction::SUB { rd: SP, rn: SP, rm: Operand::Imm12(16) })?;
self.emit_instruction(Instruction::STP { rt1: X29, rt2: X30, rn: SP, offset: 0 })?;
self.emit_instruction(Instruction::ADD { rd: X29, rn: SP, rm: Operand::Imm12(0) })?;
```

`emit_callee_saved_saves` (`emit.rs:2628-2690`) sorts the set, emits
`SUB SP, SP, #frame_bytes` to make room, then `STP Xi, Xi+1, [SP, #k*16]`
pairs (and a trailing `STR` for an odd count). This is correct.

### 2.2 Epilogue (callee-saved restores)

For `IRTerminator::Return`, `emit_terminator_regalloc` (`emit.rs:2827-2894`)
emits the return-value moves, the standard `LDP X29, X30, [SP]` /
`ADD SP, SP, #16` FP/LR restore, then (`emit.rs:2879-2885`):

```rust
// (Step 2) Restore callee-saved GPRs.
if !alloc.used_callee_saved_gprs.is_empty() {
    self.emit_callee_saved_restores(&alloc.used_callee_saved_gprs, callee_saved_size)?;
}
self.emit_instruction(Instruction::RET { rn: None })?;
```

`emit_callee_saved_restores` (`emit.rs:2700-2759`) emits `LDP` pairs (and
a trailing `LDR`) in the same sort order as the prologue, then
`ADD SP, SP, #frame_bytes` to deallocate. This is correct.

### 2.3 Frame layout

Documented at `emit.rs:1038-1045`:

```text
  [spill area]            ← SP after prologue
  [FP/LR save pair]       ← X29 points here (16 bytes)
  [callee-saved save area]← N bytes (only if used_callee_saved_gprs non-empty)
  [caller's frame]        ← SP on entry
```

Frame size computed at `emit.rs:1146`:
`self.frame_size = spill_area_aligned + callee_saved_size;`

### 2.4 Verdict on the emitter

The emitter correctly consumes `used_callee_saved_gprs` and emits matching
prologue/epilogue for every register in the set. The bug is **not** that
the emitter skips prologue/epilogue — it does not. The bug is that the
**set itself is wrong in three latent ways** (§3 below), and there is
**no verifier pass** to catch the resulting silent corruption.

## 3. The Gap (with line numbers)

### Gap G1 — Eviction spill code is emitted at position 0, not at the eviction position; reloads are not generated at all

**File:** `src/codegen/src/regalloc.rs:1807-1827` (`gen_eviction_spill_reload`)

```rust
fn gen_eviction_spill_reload(
    evict_vreg: IRValueId,
    evict_preg: PhysReg,
    _evict_end: u32,
    slot: &SpillSlot,
    result: &mut AllocationResult,
) {
    // Spill the evicted value to its slot.
    let spill = SpillCode::Spill {
        vreg: evict_vreg,
        preg: evict_preg,
        slot: slot.clone(),
    };
    result.spill_code.entry(0).or_default().push(spill);    // ← :1820  WRONG POSITION

    // For a proper implementation, we would need the use positions of the
    // evicted interval to generate reloads. Since we only track the vreg
    // and end position in the active list, we record a generic spill.
    // The emitter will need to handle reloads when it encounters uses of
    // spilled vregs.                                     // ← :1822-1826  NO RELOADS
}
```

**Two defects:**

1. **Spill at position 0 (`:1820`):** `result.spill_code.entry(0)` keys the
   spill by instruction-position 0 (the prologue area). At position 0,
   `evict_preg` does **not yet hold the evicted value** — the eviction
   happens at some later position `P`. The emitter's
   `emit_function_regalloc` (`emit.rs:1228-1234`) only emits reloads before
   position `P` and spills after position `P+1`; a spill keyed at position 0
   emits an `STR evict_preg, [X29, slot]` in the prologue, storing whatever
   garbage is in `evict_preg` at function entry.

2. **No reloads (`:1822-1826`):** The comment admits it. When the evicted
   vreg is later used, the emitter's `reg_alloc.resolve_reg(vreg)` falls
   back to the greedy allocator's on-the-fly caller-saved pick — it does
   **not** read from the spill slot. So the evicted value is permanently
   lost.

**Callee-saved impact:** When the evicted register is callee-saved (X19-X28,
the `crosses_call` preferential pick at `regalloc.rs:1486-1490`), `spill_gpr`
removes it from `used_callee_saved_gprs` (`:1588`) and re-inserts it for the
new interval (`:1450`). So the set itself is fine — but the spilled **value**
is lost, and the caller sees corruption in the callee-saved register because
the prologue saved the *new* interval's value (which was never spilled back
to the slot).

### Gap G2 — Entirely-spilled intervals use `X0` as the spill-code scratch, which is invisible to `used_callee_saved_gprs` AND to the instruction's register resolution

**File:** `src/codegen/src/regalloc.rs:1532, 1571, 1662, 1687` (`gen_spill_reload` calls in `spill_gpr` / `spill_simd`)

```rust
// In spill_gpr, "spill current interval" path (regalloc.rs:1565-1574):
let slot = SpillSlot::new(slot_idx, offset, RegClass::Gpr);
Self::gen_spill_reload(interval, PhysReg::Gpr(Register::X0), &slot, result);
result.spill_slots.insert(interval.vreg, slot);
```

`gen_spill_reload` (`regalloc.rs:1770-1798`) emits `SpillCode::Spill { vreg, preg, slot }`
at every def position and `SpillCode::Reload { vreg, preg, slot }` at every
use position — but with `preg = X0` hardcoded.

The emitter (`emit.rs:1227-1251`) then emits:
- Before each use: `LDR X0, [X29, slot.offset]` (via `emit_spill_reload`).
- The instruction itself: `emit_ir_instr(instr)` calls `reg_alloc.resolve_reg(vreg)`,
  which (because the vreg has **no entry** in `vreg_to_preg` — it was entirely
  spilled) falls back to a fresh caller-saved register from the greedy pool,
  **not X0**.
- After each def: `STR X0, [X29, slot.offset]` — stores whatever's in X0
  (which may be a return value, an argument, or some intermediate), not the
  def's actual destination register.

**Callee-saved impact:** X0 is caller-saved, so it is **not** missing from
`used_callee_saved_gprs` — that set is technically complete. But the bug
is the broader correctness gap §5.3 warns about: spill code uses a scratch
register that the rest of the pipeline doesn't know about. If a future
change ever made `gen_spill_reload` use a callee-saved scratch (e.g. X19),
the set would silently miss it. **The verifier pass (§6 Option B) is the
only defense.**

### Gap G3 — `coalesce_copies_post_alloc` is dead code in production; if wired up, its callee-saved set maintenance is fragile

**File:** `src/codegen/src/regalloc.rs:1916-2084` (`coalesce_copies_post_alloc`)

Called only from tests (`regalloc.rs:5511, 5575`). `allocate_function`
(`:1318-1352`) does **not** call it. If a future PR wires it up (per the
doc-comment example at `:1913`), the set maintenance at `:2049`/`:2062`
would activate — and it is fragile: it removes `dst_preg` from the set
unconditionally on coalescing, but if `dst_preg` is still in use by **another
live interval** (the safety check at `:2010-2037` only checks intervals
assigned to `src_preg`, not `dst_preg`), the removal is wrong.

**Callee-saved impact:** Latent — not a current bug because the function is
dead code, but a footgun for any future wire-up.

### Gap G4 — No verifier pass to catch silent callee-saved corruption

**File:** (missing) — recommended by design doc §5.3 mitigation.

There is no pass that walks each `AllocatedInstruction`'s `reads`/`writes`/
`encoded` and asserts every physical register used is either (i) caller-saved,
(ii) in `used_callee_saved_gprs`, or (iii) `X29`/`X30`/`SP` (handled by the
standard prologue). Without this, gaps G1 and G2 manifest as **silent
correctness bugs** that QEMU smoke tests on small fixtures may not catch —
exactly as §5.3 predicts.

### Gap G5 — Frame-size mismatch (out of scope for this audit, but noted)

`backend.rs:3234` uses `aarch64_compute_frame_size(func)` to set
`AllocatedFunction.frame_size`, but the byte-changing emitter computes its
own `frame_size` at `emit.rs:1146` (which **includes** the callee-saved
area). The mismatch only affects debug/unwind info, not the emitted bytes.
Design doc §7.2 calls for the `EmitResult` API change to fix this; out of
scope for R1-a (callee-saved correctness).

## 4. AArch64 Callee-Saved Register Set (per AAPCS64 ABI)

Per the AAPCS64 (documented in `regalloc.rs:37-53` and `arm64.rs:25-50`):

| Register(s) | Role                          | Class          | Tracked in `used_callee_saved_gprs`? |
|-------------|-------------------------------|----------------|--------------------------------------|
| X0–X7       | Argument / result             | Caller-saved   | n/a                                  |
| X8          | Indirect result location      | Caller-saved   | n/a                                  |
| X9–X15      | Caller-saved temporaries      | Caller-saved   | n/a                                  |
| X16–X17     | IP0/IP1 (linker scratch)      | Caller-saved   | n/a (excluded from alloc pool)       |
| X18         | Platform register             | Reserved       | n/a (excluded from alloc pool)       |
| **X19–X28** | **Callee-saved**              | **Callee-saved** | **Yes — `arm64.rs:362-376`**         |
| X29         | Frame pointer (FP)            | Reserved       | No — handled by standard prologue (`emit.rs:1165-1175` STP X29,X30; `:2866-2878` LDP X29,X30) |
| X30         | Link register (LR)            | Reserved       | No — handled by standard prologue (same STP/LDP pair as X29) |
| SP          | Stack pointer                 | Reserved       | No — implicitly saved/restored by `SUB SP`/`ADD SP` frame adjustments |

The `is_callee_saved()` predicate (`arm64.rs:362-376`) returns `true` for
exactly X19–X28. The `LinearScanAllocator` callee-saved pool
(`regalloc.rs:1248-1259`) contains exactly X19–X28. These match.

**The set definition is correct.** The gap is not in the set definition —
it is in the spill-code generation paths (G1, G2) and the absence of a
verifier (G4).

## 5. Verification on 8 Failing Tests (all involve callee-saved pressure)

All 8 failing tests trigger `interval.crosses_call = true` on multiple
vregs (because they contain `state_new`, `channel_send`, `channel_recv`,
`spawn_worker`, `wait_worker`, or user-function calls). Per
`try_alloc_gpr` (`regalloc.rs:1486-1490`):

```rust
let reg = if interval.crosses_call {
    free_callee.pop().or_else(|| free_caller.pop())  // ← prefer callee-saved
} else {
    free_caller.pop().or_else(|| free_callee.pop())
};
```

…call-crossing vregs are preferentially assigned to X19–X28, exhausting the
callee-saved pool and triggering either (a) eviction via `spill_gpr`
(exposing **G1**) or (b) the "spill current interval" path (exposing **G2**).
Either way, the function emits bytes that silently corrupt callee-saved
state, and the caller (parent function or runtime) observes wrong values.

| Test | Callee-saved pressure source | Likely gap triggered |
|------|------------------------------|----------------------|
| `complex_stores/cs_overwrite_last.vuma`     | 3× `state_new` calls (line 16-18); vregs `a`,`b`,`c`,`s` live across all 3 calls | G1 / G2 |
| `complex_stores/cs_two_buf_sum.vuma`        | 2× `state_new` calls (line 13-14); vregs `v1`,`v2`,`s` live across both | G1 / G2 |
| `complex_stores/cs_three_cell_sum.vuma`     | 3× `state_new` calls (line 14-16); vregs `a`,`b`,`c`,`s` live across all 3 | G1 / G2 |
| `multi_function/mf_pass_through.vuma`       | `main`→`f1`→`f2` chain; param `x` and return `v` cross calls in each function | G1 / G2 |
| `multi_function/mf_chained_adders.vuma`     | 4-deep call chain `main`→`f1`→`f2`→`f3`→`f4`; each function's `x`/`one`/`s`/`r` cross its own call | G1 / G2 |
| `multi_function/mf_square_pair_sum.vuma`    | `square` called twice from `main`; vregs `a`,`b`,`s` live across both calls | G1 / G2 |
| `ipc/simple_send.vuma`                      | `channel_open`/`spawn_worker`/`channel_send`/`channel_recv`/`channel_close`/`wait_worker` — 6 runtime calls; `ch`,`pid`,`x`,`status` all call-crossing | G1 / G2 |
| `ipc/ping_pong.vuma`                        | 2× `channel_open` + `spawn_worker` + 2× `channel_send` + 2× `channel_recv` + 2× `channel_close` + `wait_worker` — 10 runtime calls; `ch1`,`ch2`,`pid`,`x`,`result` all call-crossing | G1 / G2 |

**All 8 tests involve callee-saved-register pressure** via call-crossing
live ranges. The common failure mode: a callee-saved register (X19-X28)
holds a vreg that crosses a call → the allocator evicts it (or spills the
current interval) → G1 or G2 silently loses the value → the function
returns wrong data to its caller → exit code mismatch.

## 6. Proposed Fix (Option A + B + C recommendation)

### Option A — Fix `used_callee_saved_gprs` tracking (and the spill-code paths that feed it)

**A1.** Fix `gen_eviction_spill_reload` (Gap G1) to:
- Insert the spill at the **current allocation position** (passed in as a
  new parameter), not position 0.
- Generate reloads at every future use position of the evicted vreg (read
  from `interval.use_positions` — requires passing the evicted interval,
  not just the vreg, into `gen_eviction_spill_reload`).

**A2.** Fix `gen_spill_reload` (Gap G2) to either:
- (a) Use a dedicated spill scratch register tracked in
  `used_callee_saved_gprs` (e.g., reserve X18 — currently excluded — or
  X28), OR
- (b) Better: refactor so the spilled vreg's `vreg_to_preg` entry is
  preserved as the spill-code `preg` (assign a temporary physical register
  for the spill slot, even if the vreg is "entirely spilled"), OR
- (c) Best: rewrite the spill path to emit `LDR`/`STR` against the same
  physical register the emitter's `resolve_reg` will use (requires
  coordinating with `RegAllocator::preassign`).

**A3.** Fix `coalesce_copies_post_alloc` (Gap G3) callee-saved-set
maintenance: only remove `dst_preg` from the set if **no other live
interval** is still using it (check all intervals, not just those assigned
to `src_preg`). And wire it up from `allocate_function` so the
`eliminated_copies` set is non-empty in production (currently dead code).

### Option B — Add a verifier pass (per §5.3)

Add a `verify_callee_saved` function that, for each `AllocatedInstruction` in
the emitted `AllocatedFunction`, walks `reads`/`writes`/`encoded` and asserts
every physical register used is either:
- (i) caller-saved (X0–X18, V0–V7, V16–V31), OR
- (ii) in `used_callee_saved_gprs` / `used_callee_saved_simd`, OR
- (iii) `X29`/`X30`/`SP` (handled by the standard prologue), OR
- (iv) `XZR`.

Run it:
- In `#[cfg(test)]` always.
- In production behind `VUMA_VERIFY_CALLEE_SAVED=1` (panic on violation).

This catches G1/G2/G3 failures as **loud** panics instead of silent
corruption.

### Option C — Both A and B (defense in depth) — **RECOMMENDED**

A fixes the root causes; B catches any future regressions. The §5.3 design
doc explicitly recommends this combination. Per the prompt's protocol §9,
Option C is the recommendation.

## 7. Concrete Code Changes (specific functions to modify, with before/after sketches)

> **Note:** This is an audit document. The actual edits are made by R1-b-impl
> and R1-c-test (subsequent tasks in this wave). The sketches below are
> recommended starting points.

### 7.1 `gen_eviction_spill_reload` — fix position + add reloads (Gap G1)

**File:** `src/codegen/src/regalloc.rs:1807-1827`

**Before:**

```rust
fn gen_eviction_spill_reload(
    evict_vreg: IRValueId,
    evict_preg: PhysReg,
    _evict_end: u32,
    slot: &SpillSlot,
    result: &mut AllocationResult,
) {
    let spill = SpillCode::Spill {
        vreg: evict_vreg,
        preg: evict_preg,
        slot: slot.clone(),
    };
    result.spill_code.entry(0).or_default().push(spill);  // ← WRONG: position 0
    // ... no reloads ...
}
```

**After (sketch):**

```rust
fn gen_eviction_spill_reload(
    evict_vreg: IRValueId,
    evict_preg: PhysReg,
    evict_interval: &LiveInterval,    // ← NEW: pass the interval (has use_positions)
    current_pos: u32,                  // ← NEW: position of the eviction
    slot: &SpillSlot,
    result: &mut AllocationResult,
) {
    // 1. Spill at the eviction position (NOT position 0).
    let spill = SpillCode::Spill {
        vreg: evict_vreg,
        preg: evict_preg,
        slot: slot.clone(),
    };
    result.spill_code.entry(current_pos + 1).or_default().push(spill);

    // 2. Reload at every future use position of the evicted vreg.
    for &use_pos in evict_interval.use_positions.iter().filter(|&&p| p > current_pos) {
        let reload = SpillCode::Reload {
            vreg: evict_vreg,
            preg: evict_preg,           // ← same physical register; the vreg must be re-preassigned
            slot: slot.clone(),
        };
        result.spill_code.entry(use_pos).or_default().push(reload);
    }
    // 3. Keep the vreg in vreg_to_preg so resolve_reg returns evict_preg,
    //    NOT a fresh caller-saved pick. (This requires spill_gpr to NOT
    //    remove the entry at :1587 — instead, mark it as "spilled-but-
    //    reloaded-at-uses".)
}
```

**Caller update:** `spill_gpr` at `:1593-1599` must pass the evicted interval
and current position. The `active` list at `:1386` must be extended to carry
a reference to the `LiveInterval` (currently it only carries
`(vreg, preg, end, weight)` — needs `(vreg, preg, end, weight, &LiveInterval)`
or a separate `interval_by_vreg: HashMap<IRValueId, &LiveInterval>` lookup).

### 7.2 `gen_spill_reload` — fix the X0 scratch (Gap G2)

**File:** `src/codegen/src/regalloc.rs:1532, 1571, 1662, 1687` (call sites) and
`:1770-1798` (definition).

**Option C-recommended fix:** assign the entirely-spilled interval a
"reloaded-at-uses" physical register (caller-saved, scratch), and emit the
spill/reload against that register, AND ensure the emitter's `resolve_reg`
returns the same register for the vreg.

**Before (call site, `:1571`):**

```rust
Self::gen_spill_reload(interval, PhysReg::Gpr(Register::X0), &slot, result);
result.spill_slots.insert(interval.vreg, slot);
// ... no vreg_to_preg entry — emitter's resolve_reg picks a DIFFERENT reg ...
```

**After (sketch):**

```rust
// Reserve a dedicated spill-scratch register (e.g. X15 — caller-saved, not
// an argument register, not used by the greedy emitter's pool because we
// remove it from the pool here).  Track it in a new field
// `result.spill_scratch_gprs: HashSet<Register>` so the verifier (§7.3)
// knows it's allowed.
const SPILL_SCRATCH: Register = Register::X15;
result.spill_scratch_gprs.insert(SPILL_SCRATCH);
// Pre-assign the vreg to the scratch so resolve_reg returns it.
result.vreg_to_preg.insert(interval.vreg, PhysReg::Gpr(SPILL_SCRATCH));
Self::gen_spill_reload(interval, PhysReg::Gpr(SPILL_SCRATCH), &slot, result);
result.spill_slots.insert(interval.vreg, slot);
// Also remove SPILL_SCRATCH from the greedy emitter's free pool so it
// doesn't double-assign.  (Requires plumbing through Emitter::reg_alloc.)
```

(`X15` is chosen because it's caller-saved, not an argument register, and
not used as a scratch by the prologue's large-immediate sequence at
`emit.rs:1186-1209` which uses `X9`/`X10`. `X15` is also single-use:
`emit_address_with_offset` uses `X9`. Verify no other emitter path uses
`X15` before adopting.)

### 7.3 Add `verify_callee_saved` verifier pass (Gap G4 / Option B)

**File (new function in `src/codegen/src/regalloc.rs` or `emit.rs`):**

```rust
/// Verify that every physical register used by the emitted instructions
/// is either caller-saved, in `used_callee_saved_gprs`/
/// `used_callee_saved_simd`, or X29/X30/SP/XZR.
///
/// Panics on the first violation.  Run from `#[cfg(test)]` and from
/// production when `VUMA_VERIFY_CALLEE_SAVED=1`.
pub fn verify_callee_saved(
    allocated: &AllocatedFunction,
    alloc: &AllocationResult,
) -> Result<(), String> {
    let allowed: HashSet<u32> = caller_saved_gpr_indices()
        .chain(alloc.used_callee_saved_gprs.iter().map(|r| r.encoding()))
        .chain([29, 30, 31 /* SP */, 31 /* XZR — same encoding, allowed */])
        .collect();
    for instr in &allocated.instructions {
        for &reg_idx in instr.reads.iter().chain(instr.writes.iter()) {
            if !allowed.contains(&reg_idx) {
                return Err(format!(
                    "verify_callee_saved: instruction {:?} uses non-allowed \
                     physical register X{} (not caller-saved, not in \
                     used_callee_saved_gprs, not X29/X30/SP/XZR)",
                    instr.opcode, reg_idx
                ));
            }
        }
    }
    Ok(())
}
```

**Wire-up in `backend.rs:3213` (aarch64 `allocate_registers`):**

```rust
let code = if real_regalloc {
    let allocator = crate::regalloc::LinearScanAllocator::new();
    match allocator.allocate_function(func) {
        Ok(alloc) => {
            let code = emitter.emit_function(func, Some(&alloc))?;
            if std::env::var("VUMA_VERIFY_CALLEE_SAVED").map(|v| v == "1").unwrap_or(false) {
                let allocated = build_allocated_function(&code, &alloc, func);
                crate::regalloc::verify_callee_saved(&allocated, &alloc)
                    .expect("callee-saved verifier failed");
            }
            code
        }
        Err(e) => { /* ... fallback ... */ }
    }
} else { /* ... */ };
```

And in the aarch64 `emit_function_regalloc` test (`emit.rs:9289-9299`):

```rust
let alloc = LinearScanAllocator::new().allocate_function(&func).unwrap();
let allocated = build_allocated_function(...);
crate::regalloc::verify_callee_saved(&allocated, &alloc)
    .expect("callee-saved verifier must pass on a well-formed function");
```

### 7.4 Wire up `coalesce_copies_post_alloc` (Gap G3) — OPTIONAL, separate PR

**File:** `src/codegen/src/regalloc.rs:1318-1352` (`allocate_function`)

**Before:**

```rust
let mut result = self.allocate_intervals(&intervals, &call_positions)?;
result.interference_graph = build_merged_interference(func);
result.function_name = func.name.clone();
Ok(result)
```

**After (sketch):**

```rust
let mut result = self.allocate_intervals(&intervals, &call_positions)?;
result.interference_graph = build_merged_interference(func);
result.function_name = func.name.clone();
self.coalesce_copies_post_alloc(func, &mut result);   // ← NEW
Ok(result)
```

But FIRST fix the `:2049` callee-saved-set removal to check all intervals
(not just those assigned to `src_preg`). This is a non-trivial change and
should be its own PR — out of scope for R1-a/R1-b/R1-c (which focus on the
callee-saved correctness gap, not the coalescing wire-up).

## DoD Check

- [x] Audit doc exists at
      `/home/z/my-project/vuma/scripts/audit/regalloc_endianness_wave1_callee_saved_audit.md`.
- [x] All 7 required sections present: §1 Current `used_callee_saved_gprs`
      Implementation, §2 `emit_function_regalloc` Prologue/Epilogue
      Behavior, §3 The Gap (with line numbers), §4 AArch64 Callee-Saved
      Register Set, §5 Verification on 8 Failing Tests, §6 Proposed Fix
      (Option A + B + C), §7 Concrete Code Changes.
- [x] Concrete line numbers cited for every gap:
      - G1: `regalloc.rs:1807-1827` (spill at `:1820`, no reloads `:1822-1826`)
      - G2: `regalloc.rs:1532, 1571, 1662, 1687` (X0 scratch);
        `regalloc.rs:1770-1798` (definition); `emit.rs:1227-1251` (consumer)
      - G3: `regalloc.rs:1916-2084` (dead in production);
        `regalloc.rs:2049, 2062` (fragile set maintenance)
      - G4: missing — design doc §5.3 mitigation
      - G5: `backend.rs:3234` vs `emit.rs:1146` (out of scope)
- [x] No source files edited (READ-ONLY audit — `git status --short` shows
      only this markdown file added under `scripts/audit/`).
