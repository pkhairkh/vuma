# A-2 — IR + Codegen + Backends Audit

**Scope**: `src/codegen/src/` (IR, scg_to_ir, regalloc, all 19 backends, runtime, opt, egraph, etc.), `src/pipeline.rs` bridge functions, `src/codegen/tests/`, `test_results/`.
**Method**: every claim verified by reading actual source at the cited line; every "newly surfaced" bug has file:line evidence and a fix sketch.
**Catalog referenced**: `/home/z/my-project/workspace/vuma/docs/vuma-side-problem-catalog.md` at commit `6786bd23` (HEAD of `main`).
**Test data referenced**: `test_results/{summary.json,failures.txt}` from commit `78e71a6b` (run 2026-07-31 23:46:38 UTC) — this run is **PRE-`1d72d296`** (the phi + regalloc liveness fix landed at `2026-08-01 00:37:31 UTC`, ~50 minutes after the test run). The catalog's 93.42% baseline therefore predates the most important recent codegen fix.

---

## 1. Verdicts on existing catalog claims

### V-34 — `bridge_type_to_ir_type` misses f32/f64 → **VERIFIED**

`src/pipeline.rs:6503–6528` — confirmed exactly as cataloged:

```rust
6503  fn bridge_type_to_ir_type(ty: &vuma_parser::ast::Type) -> vuma_codegen::ir::IRType {
6505      match ty {
6506          Type::BDBase(name) => match name.as_str() {
6507              "i8"  => IRType::I8,   "i16" => IRType::I16,
6508              "i32" => IRType::I32,  "i64" => IRType::I64,
6509              "u8"  => IRType::U8,   "u16" => IRType::U16,
6510              "u32" => IRType::U32,  "u64" => IRType::U64,
6515              _ => IRType::U64,    // ← "f32" / "f64" / "bool" / "ptr" all land here
6516          },
6517          Type::Ptr(_) | Type::RegionPtr { .. } => IRType::U64,
6523          Type::Channel { inner, .. } => IRType::Channel(Box::new(bridge_type_to_ir_type(inner))),
6526          _ => IRType::U64,    // outer fallthrough
6527      }
6528  }
```

`IRType::F32`/`F64` exist (`src/codegen/src/ir.rs:60–63`), and `ScgType::F32`/`F64` exist and map correctly via `ScgType::to_ir_type` (`src/codegen/src/scg_to_ir.rs:1031–1032`). The bug is strictly in this one bridge function. Catalog's 3-day effort estimate is reasonable.

### V-36 — `StateRead`/`StateWrite` hardcoded `IRType::I64` → **VERIFIED, and the underlying problem is worse than cataloged**

`src/codegen/src/scg_to_ir.rs:6002–6028` — confirmed exactly as cataloged (line 6011 and 6024 both have `ty: IRType::I64`, plus `offset: 0` placeholders at lines 6010 and 6023):

```rust
6002  P::StateRead { dst, src, layout_name, field_name } => {
6007      ir_func.current_block().push(IRInstruction::Load {
6008          dst: IRValue::Register(dst_vreg),
6009          addr,
6010          offset: 0, // placeholder — BD field offset not plumbed here
6011          ty: IRType::I64,        // ← HARDCODED
6012      });
6013      let _ = (layout_name, field_name); // diagnostic only
6014      Ok(())
6015  }
6017  P::StateWrite { ptr, val, layout_name, field_name } => {
6020      ir_func.current_block().push(IRInstruction::Store {
6021          value, addr,
6023          offset: 0, // placeholder
6024          ty: IRType::I64,        // ← HARDCODED
6025      });
6026      let _ = (layout_name, field_name); // diagnostic only
6027      Ok(())
6028  }
```

**Additional finding (not in catalog)**: `StateInit` immediately above at `scg_to_ir.rs:5990–6000` has the same shape of bug for the *allocation*:

```rust
5990  P::StateInit { dst, layout_name } => {
5994      ir_func.current_block().push(IRInstruction::Alloc {
5995          dst: IRValue::Register(dst_vreg),
5996          size: 0, // placeholder — BD LayoutRegistry not plumbed here
5997      });
5998      let _ = layout_name; // diagnostic only
5999      Ok(())
6000  }
```

`ArenaNew` (line 6044–6054) and `ArenaAlloc` (line 6056–6067) have the same `size: 0` placeholder. See **V-A2-1** in §2.

### V-03 — Legacy `bridge_type_size` still called by `build_pmt_layout_specs` → **VERIFIED**

`src/pipeline.rs`:
- Legacy `bridge_type_size` at line `6532` — has `_ => 8` (line 6540) for user-defined layout names.
- Fixed `bridge_type_size_with_layouts` at line `6557` — consults `layout_sizes: &HashMap<String, u64>` (line 6570).

Caller inventory (ripgrep over `src/`):

| Function | Defined | External callers |
|---|---|---|
| `bridge_type_size` | `pipeline.rs:6532` | `build_pmt_layout_specs` at `pipeline.rs:6724` (only) + its own recursion at `:6547` |
| `bridge_type_size_with_layouts` | `pipeline.rs:6557` | `build_layout_registry` Pass 2 at `:6652` + Pass 3 at `:6679` + its own recursion at `:6582` |

So the catalog is correct: `build_pmt_layout_specs` is the **only** external caller of the legacy function, and migrating it would leave `bridge_type_size` with zero external callers. The fix is mechanical — `build_pmt_layout_specs` needs the same two-pass `layout_sizes` iteration loop that `build_layout_registry` already has at lines 6641–6669.

### V-37 — `build_pmt_layout_specs` trailing padding → **PARTIALLY REFUTED**

The catalog claim:

> The current `build_pmt_layout_specs` uses `bridge_type_align` ... but does not propagate alignment back into the size table. After V-03 is fixed, the size table must also include trailing padding to `max_align`.

Reading `src/pipeline.rs:6715–6756`, the function **already** computes trailing padding to `max_align`:

```rust
6741  let alignment = max_align.max(1);
6742  if offset > 0 && !offset.is_multiple_of(alignment) {
6743      offset = (offset + alignment - 1) & !(alignment - 1);
6744  }
6745  layouts.insert(
6746      ld.name.clone(),
6747      vuma_ive::PmtLayoutSpec {
6748          name: ld.name.clone(),
6749          total_size: offset,   // ← padded
6750          fields,
6751      },
6752  );
```

The sibling `build_layout_registry` at lines 6660–6662 (Pass 2) and 6693–6695 (Pass 3) does the same trailing-padding computation. So **trailing padding is already implemented in both functions**. V-37's stated concern ("must also include trailing padding") is refuted by the code.

What V-37 *probably* meant: once V-03 lands and `build_pmt_layout_specs` adopts a `layout_sizes: HashMap<String, u64>` table for nested-layout resolution, that table must be populated *with* trailing-padding-adjusted sizes (which `build_layout_registry` Pass 2 already does). This is just V-03 done correctly — there is no separate V-37 work item. **Recommendation**: merge V-37 into V-03 and retitle it ("migrate `build_pmt_layout_specs` to `bridge_type_size_with_layouts`, sharing `build_layout_registry`'s two-pass loop, which already computes trailing padding").

### V-13 — SIMD coverage → **VERIFIED for x86_64, PARTIALLY REFUTED for AArch64 (catalog overstates coverage)**

**x86_64** (`src/codegen/src/x86_64/stack_slot_isel.rs:3500–3527`, encoders in `x86_64/mod.rs:961–1050`):
- `paddq xmm, xmm` (SSE2) — `encode_sse_paddq` at `mod.rs:961`.
- `vpaddq xmm, xmm, xmm` (AVX, VEX.128) — `encode_avx_vpaddq` at `mod.rs:1045`.
- `psubd xmm, xmm` (SSE2) — `encode_sse_psubd` at `mod.rs:970`.
- `pmulld xmm, xmm` (SSE4.1) — `encode_sse_pmulld` at `mod.rs:979`.
- Fallback arm (line 3520–3524) emits a single NOP for any other `(op, elem_size)` combination — silently. This is a latent bug if the vectorizer is ever extended.

**AArch64** (`src/codegen/src/aarch64/mod.rs:3458–3501`, lowering at `:4816–4828`):
- `add v0.4s, v1.4s, v2.4s` (NEON, 4×i32) — `encode_neon_add_v4s` at `:3462`.
- `sub v0.4s, ...` (NEON, 4×i32) — `encode_neon_sub_v4s` at `:3469`.
- `mul v0.4s, ...` (NEON, 4×i32) — `encode_neon_mul_v4s` at `:3477`.
- `mla v0.4s, ...` (NEON, 4×i32) — `encode_neon_mla_v4s` at `:3485` (multiply-accumulate; not invoked by the lowering at `:4816`).
- `ld1`/`st1 {vt.4s}` (NEON load/store) at `:3493`/`:3501` (not invoked by the lowering).

**Catalog claim refuted**: the catalog says "Both backends support `{Add, Sub, Mul} × {i32, i64}`." This is **wrong for AArch64** — only 4×i32 (`4S`) is implemented; there is no `2D` (2×i64) NEON encoder. A grep for `2D`/`2d` in `aarch64/mod.rs` only finds the comment at line 3451 ("`2D` (2 × 64-bit) would be used for i64") — no encoder exists.

**New finding (V-A2-3 below)**: both backends lower `IRInstr::VectorOp` using **fixed hardcoded physical registers** (x86_64: `Xmm0, Xmm1, Xmm2`; AArch64: `V0, V1, V2`). See §2.

**Missing per catalog**: AVX2 (256-bit `ymm`), AVX-512 (512-bit `zmm`), `pmaxsd`/`pminsd`, gather, shuffle, SVE, MIPS MXU, LoongArch LSX. **All confirmed absent** — grep for these mnemonics finds nothing.

### V-40 — Legacy `bridge_type_size` should be deletable post-V-03 → **VERIFIED**

From the caller inventory in V-03 above: after migrating `build_pmt_layout_specs:6724` to `bridge_type_size_with_layouts`, the legacy function's only remaining caller is its own recursion at `:6547` (the `Type::Array` arm). Deleting the function and its recursion is a 1-day mechanical change.

### The 19-backend claim → **VERIFIED for the 19 count and 4 byte-swap wrappers; REFUTED for "14 have their own reg_isel.rs"**

All 19 backend directories exist under `src/codegen/src/`:

```
aarch64  aarch64_be  alpha  arm32  armeb  hppa  loongarch64  m68k  mips64
mips64be  ppc64  ppc64le  riscv32  riscv64  s390x  sparc64  wasm32  x86_32  x86_64
```

**`reg_isel.rs` inventory** — *all 19 backends have a `reg_isel.rs` file* (verified by `ls` of each dir). The 4 byte-swap wrappers have 6-line re-export files:

| Wrapper | reg_isel.rs LOC | Content |
|---|---|---|
| `aarch64_be/reg_isel.rs` | 6 | `pub use crate::aarch64::reg_isel::emit_function_regalloc_full;` |
| `armeb/reg_isel.rs` | 6 | `pub use crate::arm32::reg_isel::emit_function_regalloc_full;` (analogous) |
| `mips64be/reg_isel.rs` | 6 | `pub use crate::mips64::reg_isel::emit_function_regalloc_full;` (analogous) |
| `ppc64le/reg_isel.rs` | 6 | `pub use crate::ppc64::reg_isel::emit_function_regalloc_full;` (analogous) |

The catalog's V-41 footnote says "18 of 19: 14 native + 4 wrappers". This is **stale** — commit `f714a7a5` ("[19/19] ALL 19 backends now have reg_isel.rs — 74/76 tests pass") added the 19th `reg_isel.rs` after the V-41 text was written. Correct current count: **19 of 19 backends have a `reg_isel.rs`; 15 are substantive (≥hundreds of lines), 4 are 6-line re-export wrappers**. The catalog should say "15 native + 4 wrappers = 19", not "14 native + 4 wrappers = 18".

### wasm32 stack-machine emission → **VERIFIED**

`src/codegen/src/wasm32/mod.rs:4607` (module doc): "Wasm is a stack machine with no registers. Virtual registers from the IR are mapped to Wasm local variables." `Wasm32Backend::allocate_registers` at `:4634` calls `lower_function(func)` directly (line 4683) — no `LinearScanAllocator` involvement. Confirmed.

### The 93.42% pass-rate claim and per-backend breakdown → **VERIFIED headline, PARTIALLY REFUTED per-backend classification**

`test_results/summary.json` confirms: `total_runs=29963, matches=27992, pass_rate=93.42%, 1971 failures across 364 tests`. `test_results/failures.txt` parsed (loose regex catching `None` exit codes too) gives exactly 1971 tuples: **165 CR + 1504 MM + 302 TO**.

Per-backend pass rates (computed from `summary.json`, sorted weakest first):

| Backend | Match | Fail | Pass% | CR | MM | TO | Dominant mode |
|---|---|---|---|---|---|---|---|
| m68k | 1262 | 315 | 80.03% | 28 | 44 | **243** | TO (77%) |
| ppc64 | 1282 | 295 | 81.29% | 7 | **277** | 11 | MM (94%) |
| ppc64le | 1282 | 295 | 81.29% | 7 | **277** | 11 | MM (94%) |
| sparc64 | 1296 | 281 | 82.18% | 14 | **264** | 3 | MM (94%) |
| x86_32 | 1316 | 261 | 83.45% | 24 | **234** | 3 | MM (90%) |
| arm32 | 1526 | 51 | 96.76% | 16 | 31 | 4 | mixed |
| armeb | 1526 | 51 | 96.76% | 16 | 31 | 4 | mixed |
| riscv32 | 1532 | 45 | 97.15% | 9 | 34 | 2 | MM |
| riscv64 | 1532 | 45 | 97.15% | 9 | 34 | 2 | MM |
| aarch64 | 1533 | 44 | 97.21% | 3 | 37 | 4 | MM |
| aarch64_be | 1533 | 44 | 97.21% | 3 | 37 | 4 | MM |
| alpha | 1534 | 43 | 97.27% | 1 | 42 | 0 | MM |
| hppa | 1539 | 38 | 97.59% | 8 | 30 | 0 | MM |
| mips64 | 1540 | 37 | 97.65% | 5 | 30 | 2 | MM |
| mips64be | 1540 | 37 | 97.65% | 5 | 30 | 2 | MM |
| x86_64 | 1543 | 34 | 97.84% | 4 | 27 | 3 | MM |
| loongarch64 | 1546 | 31 | 98.03% | 1 | 28 | 2 | MM |
| s390x | 1553 | 24 | 98.48% | 5 | 17 | 2 | MM |
| wasm32 | 1577 | 0 | 100.00% | 0 | 0 | 0 | — |

**Catalog errors refuted**:

1. *"Strongest: `x86_64` (97.8%)"* — **wrong**. `s390x` (98.48%) and `loongarch64` (98.03%) are both stronger; `wasm32` is at 100% (catalog didn't mention it).

2. *"Weakest: `m68k`, `sparc64`, `hppa`, `alpha`, `x86_32`"* — **wrong on hppa and alpha**. The actual weakest five are `m68k`, `ppc64`, `ppc64le`, `sparc64`, `x86_32`. `hppa` (97.59%, 38 failures) and `alpha` (97.27%, 43 failures) are in the *middle* of the pack, not weak. The catalog completely missed that **ppc64/ppc64le are the 2nd/3rd weakest backends** with 295 failures each (more than sparc64's 281).

3. *"CR (crash) — concentrated in `m68k`, `arm32`, `armeb`, `riscv32`, `x86_32`"* — **partially wrong**. The actual top-CR backends are: `m68k` (28), `x86_32` (24), `arm32`/`armeb` (16 each), `sparc64` (14), `riscv32`/`riscv64` (9 each). `sparc64` was missed; `riscv32` is overstated (only 9 CR, and riscv64 also has 9 but wasn't mentioned).

4. *"TO (timeout) — concentrated in `aarch64`, `x86_64`, `mips64`"* — **badly wrong**. The actual TO distribution: `m68k` **243** (80% of all 302 TOs), `ppc64`/`ppc64le` 11 each, `aarch64`/`aarch64_be` 4, `arm32`/`armeb` 4, `x86_64` 3, `x86_32`/`sparc64`/`loongarch64`/`riscv32`/`riscv64`/`mips64`/`mips64be`/`s390x` 2–3 each. **m68k alone accounts for 80% of all timeouts** — the catalog completely missed this.

### Important caveat on all V-39 numbers

The test run is from commit `78e71a6b` (2026-07-31 23:46:38 UTC). The very next commit, `1d72d296` (2026-08-01 00:37:31 UTC, ~51 minutes later), is the phi + regalloc liveness fix whose commit message explicitly states it fixed `arith_fibonacci` on 9 backends (x86_64, aarch64, riscv64, arm32, mips64, loongarch64, s390x, alpha, hppa). The `failures.txt` shows `arith_fibonacci exp=55` failing with MM=1 on exactly those 9 backends + 5 more — meaning **the catalog's 93.42% baseline predates the fix that addresses a large fraction of the MM failures**. The real current pass rate is likely 2–4 percentage points higher (i.e. ~96%). **Recommendation**: re-run the full suite on `main` HEAD before treating V-39 numbers as ground truth.

---

## 2. Newly surfaced bugs

### V-A2-1 — `StateInit` Alloc emits `size: 0` (state buffers are zero-sized) — **P0, blocks the entire typed-state API**

*(Note: bug IDs V-A2-* are scoped to this A-2 report to avoid collision with A-1's V-42..V-49 parser/SCG bug IDs and A-3's V-A3-* Lean/capability IDs.)*

**Severity**: P0 — every program using `state_new(Layout)` writes through a pointer to a zero-sized stack slot, corrupting adjacent stack memory.
**Files**: `src/codegen/src/scg_to_ir.rs:5990–6000` (StateInit), `:6044–6054` (ArenaNew), `:6056–6067` (ArenaAlloc).

```rust
5990  P::StateInit { dst, layout_name } => {
5994      ir_func.current_block().push(IRInstruction::Alloc {
5995          dst: IRValue::Register(dst_vreg),
5996          size: 0, // placeholder — BD LayoutRegistry not plumbed here
5997      });
5998      let _ = layout_name; // diagnostic only
5999      Ok(())
6000  }
```

The x86_64 backend's `Alloc` lowering (`src/codegen/src/x86_64/stack_slot_isel.rs:1059–1067`) computes the aligned stack size as `(*size as i32 + 15) & !15` — for `size=0` this gives **0**, not 16. The alloc pointer (`alloc_offsets[id]` at `:1088–1089`) then points to `[rbp - 0]` = the saved RBP, so subsequent `StateWrite`/`StateRead` clobber the saved frame pointer.

**Confirmed test impact**: `tests/gold_standard/float_mem/{f32_store_load,f64_store_load,f64_struct_field}.vuma` all fail with `MM=0` on **all 17 non-wasm32 backends** (the strongest signal of a systematic bug in the entire failure file). The test source (`f32_store_load.vuma`):

```vuma
layout Cell = { v: f64 }
transform main() -> i32 {
    let c = state_new(Cell);    // ← Alloc { size: 0 } — c points to saved RBP
    c.v = 7.25;                 // ← Store 8 bytes through c, overwriting saved RBP
    let val = c.v;              // ← Load 8 bytes through c — reads whatever is at [rbp]
    let result = floattoint(val);
    return result;              // ← returns 0 instead of 7
}
```

**Fix sketch**: thread `BD LayoutRegistry` (or a pre-computed `layout_sizes: &HashMap<String, u64>` table — the same one V-03 needs) into `IRBuilder`, and have `StateInit` look up `layout_name`'s size: `let size = layout_sizes.get(layout_name).copied().unwrap_or(0); ir_func.current_block().push(IRInstruction::Alloc { dst, size });`. Apply the same fix to `ArenaNew` (use the capacity expression) and `ArenaAlloc` (use the layout's size).
**Effort**: 1 week (same epic as V-03/V-35 — they all need the layout registry plumbed into `IRBuilder` — see also A-1's V-42 "register_layout propagation" for the parser-side equivalent).
**Unblocks**: every test under `tests/gold_standard/{float_mem,pmt_state,pmt_session,pointers,atomics}/*` that uses `state_new`.
**Unblocks**: every test under `tests/gold_standard/{float_mem,pmt_state,pmt_session,pointers,atomics}/*` that uses `state_new`.

### V-A2-2 — `inttofloat`/`floattoint`/`uinttofloat`/`floattouint` casts are hardcoded to `I64 ↔ F64` — **P1, blocks f32 casts and 32-bit int↔float casts**

**Severity**: P1 — any `inttofloat` of an i32, `floattoint` to an i32, or any f32 cast produces wrong types.
**File**: `src/codegen/src/scg_to_ir.rs:5250–5291`.

```rust
5250  let (from_ty, to_ty) = match kind {
5251      CastKind::IntToFloat | CastKind::UIntToFloat => {
5252          (Some(IRType::I64), Some(IRType::F64))    // ← hardcoded source = I64
5253      }
5254      CastKind::FloatToInt | CastKind::FloatToUInt => {
5255          (Some(IRType::F64), Some(IRType::I64))    // ← hardcoded result = I64
5256      }
5257      CastKind::FloatToFloat => { /* handled correctly with src/dst probing */ }
5282      _ => unreachable!(),
5283  };
5284  let result_ty = match kind {
5285      CastKind::IntToFloat | CastKind::UIntToFloat => IRType::F64,    // ← hardcoded
5286      CastKind::FloatToInt | CastKind::FloatToUInt => IRType::I64,    // ← hardcoded
5287      CastKind::FloatToFloat => match &to_ty { Some(t) => t.clone(), None => IRType::F64 },
5291      _ => unreachable!(),
5292  };
```

**Impact**:
1. `inttofloat(5u32)` produces a Cast with `from_ty=I64, to_ty=F64`. The x86_64 backend at `stack_slot_isel.rs:2870–2904` (the `IntToFloat` arm) treats the source as i64 — if the source vreg's stack slot contains a u32 value zero-extended to 8 bytes, this happens to work; if it contains garbage in the high 32 bits, the result is wrong.
2. `floattoint(x)` to an i32 destination produces `to_ty=I64` regardless. Downstream code that expects an i32 will read 8 bytes from the result's stack slot instead of 4.
3. There is **no way** to express `f64 → f32` (narrowing) or `f32 → f64` (widening) via these builtins — only `floattofloat` handles that, and it probes `vreg_types`/`fn_var_types` to determine direction.

**Fix sketch**: thread source/dest types from the SCG cast node (which already carries them — see `ScgExpr::Call` parsing). Replace the hardcoded `IRType::I64`/`F64` with the actual source/dest types. (See A-1's V-43 "infer_expr_type misnamed" for the parser-side type-inference work that feeds this.)
**Effort**: 3–4 days.
**Regression test gap**: `regression.rs::test_fp_conversion_not_noop_all_backends` (line 1019) only checks that the opcodes contain a conversion mnemonic — it does **not** execute the code or verify the result, and it only tests `I64↔F64` (exactly the hardcoded path). No test exists for `I32→F32`, `F64→I32`, or any f32 cast.

### V-A2-3 — SIMD lowering hardcodes physical registers (Xmm0/Xmm1/Xmm2 on x86_64; V0/V1/V2 on AArch64) — **P1, vectorizer is non-functional for real loops**

**Severity**: P1 — the SLP vectorizer (`vectorize::slp_vectorize_block`) runs and emits `IRInstr::VectorOp` with proper vreg operands, but the backend lowering throws those operands away and uses fixed physical registers. Any function with more than one vector op, or any vector op interleaved with scalar ops that use Xmm0–2, will produce wrong code.
**Files**:
- x86_64: `src/codegen/src/x86_64/stack_slot_isel.rs:3493–3527` — `dst: _`, `lhs: _`, `rhs: _` are ignored; encoders called with `Xmm::Xmm0, Xmm::Xmm1, Xmm::Xmm2`.
- AArch64: `src/codegen/src/aarch64/mod.rs:4816–4828` — `dst: _`, `lhs: _`, `rhs: _` ignored; encoders called with `0, 1, 2` (V0/V1/V2). The comment at line 4813–4815 is explicit: "Full vector-vreg → physical-V register allocation is deferred; the IR-level vregs are tracked for dataflow but the encoded word uses fixed V0/V1/V2."

**Fix sketch**: extend the linear-scan register allocator to handle a `VregClass::Vector` (XMM on x86_64, V on AArch64) in addition to the existing `VregClass::Int` and `VregClass::Float`. Lower `IRInstr::VectorOp` to use the allocated physical registers.
**Effort**: 2–3 weeks (regalloc extension + both backends + per-backend smoke tests).
**Note**: this should be sequenced *after* V-13's missing-encoders work (AVX2, pmaxsd/pminsd, etc.) — there's no point in regalloc support for encoders that don't exist yet. Note also that A-1's V-42..V-49 are different bugs (parser/SCG layer); my V-A2-* are scoped to IR/codegen.

### V-A2-4 — `IRInstr::Transform`, `BulkCopy`, `BulkFill`, `StarkProof`, and all 6 channel IR instructions are silent no-ops on most backends — **P1, multiple load-bearing IR instructions disappear**

**Severity**: P1 — programs using state transforms, memcpy/memset intrinsics, STARK proofs, or channel IR (the `IRInstr::Channel*` variants, not the `Call{func:"channel_open"}` form) silently produce no code on every backend except x86_64 (and even x86_64 only implements `BulkCopy`/`BulkFill`; `Transform` is a pointer-copy stub, `StarkProof` is a no-op).
**Files** (representative; the pattern repeats in every backend):

```rust
// aarch64/mod.rs:4830–4843 (identical in alpha, arm32, hppa, loongarch64, m68k, mips64, ppc64, riscv32, riscv64, s390x, sparc64, wasm32, x86_32)
IRInstr::ChannelOpen { .. } | IRInstr::ChannelSend { .. }
| IRInstr::ChannelRecv { .. } | IRInstr::ChannelRecvTimeout { .. }
| IRInstr::ChannelRecvResult { .. } | crate::ir::IRInstr::CallIndirect { .. }
| IRInstr::ChannelClose { .. }
| IRInstr::StarkProof { .. }
| IRInstr::BulkCopy { .. }
| IRInstr::BulkFill { .. } | IRInstr::Transform { .. } => {}
```

The comment is stale: "no frontend generates channel IR yet" — but `lower_channel_open` at `scg_to_ir.rs:5848` *does* emit `IRInstr::ChannelOpen`, and `P::StateTransform` at `scg_to_ir.rs:6030–6042` *does* emit `IRInstr::Transform`.

**x86_64 exceptions** (`x86_64/stack_slot_isel.rs`):
- `BulkCopy` at `:4376–4394` — implemented as `REP MOVSB` (correct).
- `BulkFill` at `:4396–4409` — implemented as `REP STOSB` (correct).
- `Transform` at `:4419–4428` — **just a pointer copy** (`load src → RAX; store RAX → dst`), not a real layout transform. The source state's buffer pointer is assigned to dst, so dst aliases src — any subsequent write to dst corrupts src.
- `StarkProof` at `:4308–4316` — emits nothing (`Vec::new()`), only sets `instr_opcode = "stark_proof"`.

**Confirmed test impact** (from `failures.txt`):
- `memory/mem_copy_buffer.vuma`: 14 backends CR=`-11` (SIGSEGV) — matches the 14 backends where `BulkCopy` is a no-op (all except x86_64 + 4 wrappers that inherit x86_64's behavior — but actually wrappers inherit their *parent's*, so only x86_64 itself works).
- `ipc/stark_proof.vuma`: 11 backends CR=`-11`, 7 backends MM=`0` — matches the no-op `StarkProof`.

**Fix sketch**:
1. For `Transform`: either implement a real layout-transform mem-to-mem copy (walk field offsets, emit per-field loads/stores with byte width from the layout registry), or lower it to `BulkCopy { dst, src, len: layout_size }` (which then needs V-A2-1's layout-size plumbing).
2. For `BulkCopy`/`BulkFill` on non-x86_64 backends: lower to a `Call { func: "memcpy" / "memset" }` (the runtime already has these via libc), or emit a per-backend copy loop.
3. For `StarkProof`: lower to `Call { func: "stark_prove" }` (the runtime stub exists — `ipc_lowering.rs:6282` references `stark_table`).
4. For `Channel*` IR: lower to `Call { func: "channel_open" / "channel_send" / ... }` (the Call-form path already exists at `scg_to_ir.rs:5483–5499`).
**Effort**: 2 weeks (Transform is the hardest; the rest are 1–2 day mechanical lowerings).

### V-A2-5 — `current_return_type` is parsed from the function *name* and clobbers the correct type for f32/f64/ptr returns — **P2, breaks load-width inference for FP-returning functions on big-endian backends**

**Severity**: P2 — affects `lower_access` load-width inference, which is critical for big-endian backends (ppc64, sparc64, mips64be) where U8 store + U32 load reads the wrong byte.
**File**: `src/codegen/src/scg_to_ir.rs:1997–2016`.

```rust
1976  if func.results.len() == 1 {
1977      self.current_return_type = Some(func.results[0].to_ir_type());   // ← correct
1978  } else {
1979      self.current_return_type = None;
1980  }
...
1997  if let Some(open) = func.name.rfind('(') {
1998      if let Some(close) = func.name.rfind(')') {
1999          if close > open {
2000              let ret_ty_str = &func.name[open + 1..close];
2001              if !ret_ty_str.is_empty() && ret_ty_str != "void" {
2002                  self.current_return_type = match ret_ty_str {
2003                      "u8" | "U8" => Some(IRType::U8),
...
2010                      "i64" | "I64" => Some(IRType::I64),
2011                      _ => None,   // ← "f32" / "f64" / "ptr" / "bool" / "Channel" clobber to None
2012                  };
2013              }
2014          }
2015      }
2016  }
```

The code first sets `current_return_type` correctly from `func.results[0].to_ir_type()` (line 1977), then **overwrites** it by parsing the function name's `(type)` suffix. The suffix parser handles only the 8 integer types and falls through to `None` for `"f32"`, `"f64"`, `"ptr"`, `"bool"`, `"Channel<...>"`, etc. So a function named `read_float(f64)` whose `results` field correctly says `ScgType::F64` gets `current_return_type = None` — defeating the load-width inference at line 1977.

**Fix sketch**: delete lines 1997–2016 entirely. The `func.results[0].to_ir_type()` path at line 1977 is already correct and handles all `ScgType` variants including F32/F64/Ptr/Channel. The name-parsing path is a stale workaround from before `func.results` existed.
**Effort**: 1 day (delete the dead code + add a regression test for `f64`-returning functions on ppc64).

### V-A2-6 — `channel_open` builtin hardcodes `Channel<I64>` payload type — **P2, `Channel<f32>` and `Channel<Struct>` are mistyped**

**Severity**: P2 — the IVE linear-type checker and any future session-type checker see the wrong payload type.
**File**: `src/codegen/src/scg_to_ir.rs:5483–5499`.

```rust
5483  if call.func == "channel_open" {
5484      if let Some(IRValue::Register(vreg)) = &dst {
5485          // Default to Channel<I64> — the Call-form path doesn't
5486          // carry the type parameter (the parser drops it before
5487          // SCG lowering for channel_open).
5490          self.vreg_types.insert(
5491              *vreg,
5492              crate::ir::IRType::Channel(Box::new(crate::ir::IRType::I64)),  // ← hardcoded
5493          );
5494          self.channel_handle_vregs.insert(*vreg);
5495      }
5496  }
```

The comment is explicit: "the parser drops it before SCG lowering for channel_open." So `Channel<f32>` becomes `Channel<i64>` at the IR layer — the IVE checker can't verify that sends/receive match the declared payload type. The `lower_channel_open` path at `:5835–5853` (Statement-form) does correctly use `co.elem_ty.to_ir_type()` (line 5850), so the bug is specifically in the Call-form lowering.

**Fix sketch**: thread the channel's type parameter from the parser through the SCG `Call` node (currently dropped), then use it at line 5492 instead of the hardcoded `IRType::I64`.
**Effort**: 3–4 days (touches parser, SCG node, and scg_to_ir).

### V-A2-7 — HPPA F64 softfloat stubs are partial — `sub`/`mul`/`div` return 0; `lt` returns 0 for negative operands; F32 is entirely stubbed — **P1, HPPA cannot do runtime FP arithmetic**

**Severity**: P1 — any HPPA program doing `a - b`, `a * b`, `a / b`, or `a < b` (with at least one Register operand and at least one negative value) on f64 gets wrong results; any F32 operation on Register operands returns 0.0. Constant-folded operations (both operands Immediate) are correct because they bypass the stubs.
**File**: `src/codegen/src/hppa/mod.rs`.

```rust
2211  /// Build `__vuma_f64_lt` — f64 < f64 → i32 (0 or 1).
2213  /// PARTIAL: correct for non-negative values (unsigned comparison of bit
2214  /// patterns). For negative values, returns 0 (TODO: handle both-negative).
2690  /// Implemented as `a + (-b)`: flip the sign bit of b ...
2696  /// NOTE: because the underlying add stub returns 0 for different-sign
2697  /// inputs, `sub` currently only returns non-zero when ...
2703  /// which is TODO.  Constant-folded subtractions (both operands
2704  /// Immediate) are unaffected and compute the correct result.
2719  /// Build `__vuma_f64_mul` and `__vuma_f64_div` — placeholder stubs that
2720  /// return 0.0.  A full implementation requires a 53x53→106-bit multiply
3697  // TODO: implement F32 soft-float stubs.
3698  code.extend(ss_load_imm(S0, 0));  // ← F32 Register operand: store 0.0
4201  // F32 Register: stub (store 0). TODO: F32 compare.
4716  // F32 Register: stub (store 0). TODO: F32→I64.
```

The HPPA backend has no FPU (PA-RISC 1.1 coprocessor is stubbed out — line 2721–2722: "XMPYU lives in the coprocessor, which this backend stubs out"). All FP arithmetic goes through software emulation stubs, and only `__vuma_f64_add` (same-sign) and `__vuma_f64_eq` are correct.

**Fix sketch**: either (a) implement the missing stubs (`__vuma_f64_sub` opposite-sign path, `__vuma_f64_mul` via shift-add, `__vuma_f64_div` via shift-subtract, `__vuma_f64_lt` both-negative case, and all F32 stubs) — ~3 weeks of bit-twiddling work, OR (b) lower FP BinOp on HPPA to `Call { func: "__vuma_f64_add" / ... }` and have the runtime provide the stubs via libc / soft-fp — 1 week, but adds a runtime dependency.
**Effort**: 1 week (option b) or 3 weeks (option a).

### V-A2-8 — m68k F32 softfloat stubs return 0.0 — **P1, m68k cannot do F32 arithmetic on Register operands**

**Severity**: P1 — same class as V-A2-7 but for m68k. Constant-folded F32 is correct; Register-operand F32 returns 0.0.
**File**: `src/codegen/src/m68k/mod.rs:3904–3921`.

```rust
3904  } else {
3905      // F32 with Register operand: not yet supported via soft-float
3906      // (would require __addsf3 / __subsf3 / __mulsf3 / __divsf3
3907      // stubs). Stub with 0.0 as a safe default.  Constant-folded
3908      // F32 immediates are handled by the Immediate path above.
3909      // TODO: wire up F32 soft-float stubs.
3910      code.extend(ss_load_imm(S0, 0));
3911      code.extend(ss_st(S0, dst_off));
```

The m68k backend delegates f64 Register-operand arithmetic to `__adddf3`/`__subdf3`/`__muldf3`/`__divdf3` (line 3758–3759) but has no equivalent for f32. The comment names the required stubs (`__addsf3` etc.) — these are libgcc's standard soft-float entry points.

**Fix sketch**: implement `__addsf3`/`__subsf3`/`__mulsf3`/`__divsf3` stubs (either inline in m68k/mod.rs following the f64 stub pattern, or as a runtime library that m68k links against). The f32 stubs are simpler than f64 (23-bit mantissa vs 52-bit).
**Effort**: 1 week.

---

## 3. Backend-by-backend test failure analysis

(All numbers from `test_results/{summary.json,failures.txt}` at commit `78e71a6b` — pre-`1d72d296`.)

### m68k — 315 failures (80.0% pass) — **TO-dominated (243 of 315 = 77%)**

**Pattern**: 243 timeouts + 28 CR + 44 MM. The TO failures concentrate in:
- All `arith_*` tests with loops (`fibonacci`, `factorial`, `tribonacci`, `count_bits`, `integer_sqrt`, `power`, `mul_table`, `sub_chain`, `reverse_digits`, `palindrome`).
- All `nested_loops/*` tests.
- All `atomics/*` tests (4 tests, all MM — atomics are partial on m68k).
- All `pointers/*` tests (5 tests, mostly MM with wrong bit patterns).

**Likely root cause**: m68k has no working loop-counter register allocation. The 1d72d296 phi+regalloc fix should help (m68k is not in the "fixed" list of 9 backends — its failures are attributed to "backend-specific reg_isel bugs" in the commit message). Combined with V-A2-7's F32 stubs and the m68k-specific `D2 not_allocatable` fix from commit `1bf5d9d5`, m68k is the most buggy backend.

**Specific m68k issues**:
- 68881 FPU encodings were replaced with soft-float calls (commit `7c6c1ddb` "Fix m68k: bsr.l call, lea Alloc, D2 not_allocatable, Bcc/Bra 16-bit encoding") — but F32 soft-float is still stubbed (V-A2-8).
- `pointers/ptr_store_xor_load.vuma exp=255, m68k returns 170` — 170 = 0xAA, suggesting a byte-swap or partial-load issue.

### ppc64 / ppc64le — 295 failures each (81.3% pass) — **MM-dominated (277 of 295 = 94%)**

**Pattern**: 277 MM + 11 TO + 7 CR. The MM failures are remarkably uniform — many return `0` when a non-zero value is expected:
- `arith_add_chain exp=55 → 0`
- `arith_clamp exp=100 → 150` (over by 50)
- `arith_count_bits exp=7 → 124 TO`
- `arith_gcd exp=6 → 48` (8x expected)
- `arith_is_power_of_two exp=1 → 0`
- `arith_lcm exp=12 → 36` (3x expected)
- `arith_next_power_of_two exp=64 → 50`
- `arith_sub_chain exp=45 → 100` (ppc64 only, others get 9)

**Likely root cause**: ppc64's regalloc has a register-reuse bug similar to the one 1d72d296 fixed for the other 9 backends, but the ppc64-specific `reg_isel.rs` wasn't included in that fix. The "return 0" pattern is the classic symptom of a register being clobbered before its value is consumed. ppc64le inherits ppc64's failures via the wrapper.

**Specific ppc64 issues**:
- `ppc64` and `ppc64le` have **identical** failure counts (295 each, same MM/CR/TO split) — confirming ppc64le is a pure wrapper that inherits all of ppc64's bugs.
- `struct_chain.vuma exp=3 → 134 CR` — ppc64/ppc64le/sparc64 all crash with exit 134 (SIGABRT). Suggests an assertion or trap in the ppc64 backend for struct field access.

### sparc64 — 281 failures (82.2% pass) — **MM-dominated (264 of 281 = 94%)**

**Pattern**: 264 MM + 14 CR + 3 TO. Similar to ppc64: many MM failures return 0 or wrong-but-nonzero values:
- `arith_clamp exp=100 → 150` (same as ppc64)
- `arith_gcd exp=6 → 48` (same as ppc64)
- `nl_countdown exp=0 → 246` (sparc64 returns 246 when 0 expected)

**Likely root cause**: same class of regalloc/liveness bug as ppc64. The shared `arith_clamp exp=100 → 150` and `arith_gcd exp=6 → 48` patterns across ppc64/ppc64le/sparc64/alpha suggest a common root cause in the shared `regalloc.rs` liveness analysis that 1d72d296's Phase 1b didn't fully fix for these backends' specific CFG shapes.

**Specific sparc64 issues**:
- `bounds_basic/inbounds_loop.vuma exp=10 → 134 CR` — sparc64 crashes (SIGABRT) on in-bounds loop checks.
- `uaf_negative.vuma exp=42 → -11 CR` — sparc64 SIGSEGV on use-after-free.

### x86_32 — 261 failures (83.5% pass) — **MM-dominated (234 of 261 = 90%)**

**Pattern**: 234 MM + 24 CR + 3 TO. MM failures often return 0 or wrong values:
- `arith_clamp exp=100 → 114` (different from ppc64's 150 — x86_32-specific)
- `arith_modular_exp exp=1 → 0`
- `arith_next_power_of_two exp=64 → 114`
- `arith_reverse_digits exp=54 → 194`
- `complex_stores/*` — 6 tests, all MM (x86_32 has 32-bit pointer truncation issues with complex stores)
- `structs/*` — 4 tests, MM (struct field access on x86_32)

**Likely root cause**: x86_32 has 32-bit pointers but the IR uses 64-bit `IRValue::Address(u64)` and `IRValue::Immediate(i64)`. The `as_address_32bit` helper exists (per `ir.rs:1031–1033` doc) but is likely not called consistently — leading to pointer truncation. The `complex_stores` and `structs` failures fit this pattern.

### arm32 / armeb — 51 failures each (96.8% pass) — **mixed CR/MM**

**Pattern**: 16 CR + 31 MM + 4 TO. CR failures concentrate in:
- `arena_*` tests (5 tests, all CR) — arm32 crashes on arena operations.
- `arith_mul_table exp=36 → -11 CR`.
- `bounds_safe/uaf_negative.vuma exp=42 → -11 CR`.

armeb inherits arm32's failures via the wrapper.

---

## 4. Register allocator + phi construction analysis (post-`1d72d296`)

### What 1d72d296 fixed

Two root-cause bugs, both in shared code:

1. **Non-deterministic phi construction** (`scg_to_ir.rs`): `lower_if`, `lower_switch`, `lower_loop`, and `resolve_phis` used `HashSet<String>` iteration to determine phi-vreg allocation order. HashSet iteration is randomized per-process (Rust's SipHash seed). Fix: sort `all_modified` / `sorted_names` / `sorted_modified` alphabetically before allocating phi vregs; sort `copies_by_pred` keys by label before emitting parallel copies.

2. **Linear-scan liveness missing loop back-edges** (`regalloc.rs:1074–1102`, new "Phase 1b"): the original `LiveRangeComputer::compute` used linear position numbering based only on def/use positions. A vreg defined before a loop and used inside got an interval ending at the use, missing the blocks between the use and the back-edge. The allocator then reused the vreg's physical register inside the loop body, clobbering the loop-invariant value. Fix: after the linear scan, run `LivenessAnalysis::compute` (standard iterative dataflow) and extend each vreg's interval to cover every block where it's `live_in` or `live_out`.

### Verified determinism of the phi construction post-fix

I traced every phi-emission site in `scg_to_ir.rs`:

| Site | Sort applied? | Line |
|---|---|---|
| `lower_if` then/else merge | YES — `all_modified_sorted.sort()` | `:2815–2819` |
| `lower_loop` header phis | YES — `sorted_names.sort()` | `:3139–3140` |
| `lower_loop` exit phis | YES — `sorted_modified.sort()` | `:3563–3564` |
| `lower_switch` arm merge | YES — `all_modified_sorted.sort()` | `:4178–4180` |
| `resolve_phis` parallel-copy emission | YES — `sorted_preds.sort_by(|a,b| a.0.cmp(b.0))` | `:3738–3739` |

The `emit_parallel_copies` algorithm (`:3842–3939`) is deterministic given sorted inputs — it picks the first ready copy in `copies` order, and the cycle-breaking path picks `copies[0]` deterministically.

### Remaining edge cases (potential bugs not addressed by 1d72d296)

1. **`LivenessAnalysis::compute` does not special-case Phi nodes** (`regalloc.rs:4927–5060`). The standard dataflow formulation at line 4994–5000 computes `live_out(b) = ∪ live_in(s)` for all successors `s`. For phi nodes `dst = phi(val_a from blk_A, val_b from blk_B)`, the value `val_a` is live-out from `blk_A` along the edge `blk_A → phi_block` but is **killed** at the phi (it's not live-in to `phi_block`). The current algorithm treats `val_a` as live-in to `phi_block` (because it appears in the phi's `used_regs`), which over-approximates liveness. This is **safe** (over-approximation never causes clobbering) but causes the allocator to be more conservative than necessary — potentially increasing spill count. Not a correctness bug.

2. **The Phase 1b interval extension is overly broad**. `regalloc.rs:1093–1100`:
   ```rust
   for (label, start, end) in &block_pos_ranges {
       let is_live_in = liveness.block_live_in(label).contains(&vreg);
       let is_live_out = liveness.block_live_out(label).contains(&vreg);
       if is_live_in || is_live_out {
           interval.extend_to(*start);
           interval.extend_to(*end);
       }
   }
   ```
   This extends the interval to cover the **entire** block if the vreg is live-in or live-out. For a vreg that's live-in to a 100-instruction block but only used in instruction 1, the interval now spans all 100 instructions — preventing register reuse for the other 99. Again **safe but conservative**.

3. **`contains_fork` opt-out** — assessed as **correct but over-conservative**:
   - Present in: `backend.rs:3230` (aarch64), `x86_64/mod.rs:4349`, `riscv64/mod.rs:6806`, `riscv32/mod.rs:8702`, `arm32/mod.rs:9771`, `s390x/mod.rs:3111`, `hppa/mod.rs:5633`, `loongarch64/mod.rs:2635`, `mips64/mod.rs:3735`, `sparc64/mod.rs:4903`, `m68k/mod.rs:4597`, `alpha/mod.rs:3126`, `wasm32/mod.rs:4658` (observational only — no fallback).
   - Behavior: when a function contains `Call{fname="spawn_worker"|"fork"}` or `Syscall{nr=56|58|220|221}` (clone/vfork on x86_64/aarch64), the backend falls back to stack-slot ISel instead of using register-based allocation.
   - x86_64 (`x86_64/mod.rs:4368–4376`) explicitly broadens this to "ANY syscall that has a Register arg AND a dst" — comment says "overly broad but safe — the stack-slot path handles all syscalls correctly. The regalloc path can be re-enabled for these once the allocator's live-range analysis is fixed to not reuse an arg's register for the dst when the arg is live across the syscall."
   - **Assessment**: the opt-out correctly works around a real regalloc bug (callee-saved prologue/epilogue doesn't interact correctly with `clone()` because the child runs with a different register state). The deeper underlying issue — regalloc reuses an arg's register for the syscall's dst when the arg is live across the syscall — is **not fixed**. The opt-out is a workaround, not a fix. **Recommendation**: file V-A2-9 ("regalloc live-range analysis doesn't model syscall arg/dst interference") and track it as the prerequisite for removing the `contains_fork` opt-out.

4. **No regression test for the phi determinism fix**. `regression.rs` has 13 tests, none of which exercise the phi + liveness interaction on a loop with a loop-invariant variable. A test like `assert_eq!(compile_and_run(fibonacci_vuma), 55)` run on ALL_BACKENDS would have caught the 1d72d296 bug pre-fix and would catch any regression. **Recommendation**: add a `test_loop_invariant_not_clobbered_all_backends` regression test.

---

## 5. Test coverage gaps

### `src/codegen/tests/` contents

Only **one** file: `arena_differential.rs` (183 lines) — a randomized differential fuzzer for `lean_alloc_mirror` vs the arena's `alloc_raw` decision function. It does **not** exercise any backend, does not compile real VUMA source, and does not test the codegen pipeline.

All other codegen tests live in `src/tests/src/` (24 files, 27,123 lines total). The most relevant for IR/codegen:

| File | LOC | # tests | Focus |
|---|---|---|---|
| `regression.rs` | 1,375 | 13 | Per-backend instruction-encoding regressions (arm64 ror/rol, loongarch64 atomics, ppc64 atomics, riscv64 CAS, wasm32 CAS, arm32 CAS, mips64 ror/rol, FP conversion, etc.) |
| `cross_backend.rs` | 2,773 | 17 | Cross-backend consistency (simple return, arithmetic, memory, function call, output format, code size, name, wasm32 module, ELF header, syscall conformance, etc.) |
| `codegen.rs` | 726 | 10 | Basic codegen (simple add, stack allocation, load/store, if/else, loop, function call, multi-function ELF, type system calling conv, bare-metal raw, arm64 encoding) |
| `abi_conformance.rs` | 2,053 | — | ABI conformance across backends |
| `sha256d_backends.rs` | 1,868 | — | SHA256d kernel across backends |
| `framework.rs` | 2,405 | — | Test framework helpers (parses VUMA, builds SCG, runs IVE) |

### Regression tests for P0/P1 catalog bugs

| Bug | Regression test exists? | Evidence |
|---|---|---|
| V-34 (f32/f64 bridge) | **NO** | grep for `f32.*state`, `measured_w`, `bridge_type_to_ir_type` in `src/tests/` returns nothing. |
| V-35 (type_size_from_name for layouts) | **NO** | grep for `type_size_from_name`, `nested.*layout` returns nothing. |
| V-36 (StateRead/StateWrite hardcoded I64) | **NO** | grep for `StateRead`, `StateWrite`, `state_new.*f32` returns nothing in tests. |
| V-03 (legacy bridge_type_size) | **NO** | only references are in `framework.rs:665, 703` which *call* `build_pmt_layout_specs` — they don't test its correctness. |
| V-37 (trailing padding) | **NO** | no test exercises a layout with `max_align > last_field_align`. |
| V-13 (SIMD) | **Partial** | `regression.rs` has no SIMD test. `x86_64/mod.rs:6429–6473` has unit tests for the NEON encoder *bit patterns* (encode_neon_add_v4s etc.) but no execution test. |
| V-A2-1 (Alloc size=0) | **NO** | the `float_mem/*` gold-standard tests would catch it but they're in `tests/gold_standard/` (run via the test matrix, not via `cargo test`), and they're currently failing on all backends without a code-level regression test. |
| V-A2-2 (cast hardcoded I64/F64) | **Partial** | `regression.rs::test_fp_conversion_not_noop_all_backends` (line 1019) only checks opcode mnemonics — does not execute, does not test F32 or I32 destinations. |
| V-A2-3 (SIMD hardcoded Xmm0/1/2) | **NO** | no test executes a function with multiple vector ops. |
| V-A2-4 (Transform/BulkCopy/Channel no-ops) | **NO** | no test checks that `IRInstr::Transform` produces actual code on any backend. The `cross_backend.rs::test_syscall_conformance_all_backends` (line 2077) tests syscalls but not channel/transform IR. |

### Backends lacking a smoke test

`src/codegen/src/wrapper_smoke.rs` provides a smoke test (`test_smoke_compile_simple_program_all_4_wrappers`) for **only the 4 thin-wrapper backends** (armeb, aarch64_be, mips64be, ppc64le). The smoke test compiles a trivial empty function and checks `encode_function` produces ≥4 bytes.

**The other 15 backends have no equivalent smoke test** — there's no test that verifies "aarch64 can compile a non-trivial function without panicking". The `cross_backend.rs::test_cross_backend_simple_return` (line 475) does this for simple returns, but it's an integration test that depends on the full pipeline; a unit-level smoke test per backend would catch regressions earlier.

**Recommendation**: add a `smoke_compile_all_19_backends` test that compiles a minimal-but-non-trivial function (1 add, 1 load, 1 store, 1 branch) on each backend and asserts non-empty output. This would have caught the V-A2-4 no-op IR instruction bugs (the encoded output would be suspiciously small).

### `src/codegen/src/wrapper_smoke.rs` is also stale

Lines 13–14 and 78–79 say "This deliberately avoids `IRInstr::Syscall` so that the `unimplemented!()` panic in the `mips64` and `ppc64` parents does not affect this test." But commit `7998eaa1` ("[Wave-D2] Fix s390x: add generic clone (nr=220/221) to contains_fork check") and the mips64be/mod.rs doc at line 15 ("None — `IRInstr::Syscall` works via inheritance... The earlier ' PENDING' status reported by Task 1-c's survey is STALE — `unimplemented!()` was removed") indicate the `unimplemented!()` panics have been removed. The smoke test's rationale is stale — it could now exercise Syscall too.

---

## Summary of recommendations

1. **Re-run the full test suite on `main` HEAD** (post-`1d72d296`) before treating V-39's 93.42% as ground truth. The phi+regalloc fix likely moved 50–100 MM failures to passing.
2. **Add V-A2-1** (StateInit Alloc size=0) to the P0 bridge-fix epic — it's the same plumbing work as V-03/V-35 and unblocks `float_mem/*`, `pmt_state/*`, `pointers/*` tests.
3. **Merge V-37 into V-03** — trailing padding is already implemented; V-37 is just "do V-03 correctly".
4. **Correct the catalog's V-39 per-backend classification**: ppc64/ppc64le are the 2nd/3rd weakest (not hppa/alpha); m68k is TO-dominated (not CR-dominated); the "strongest" backend is s390x or wasm32 (not x86_64).
5. **Correct the V-41 backend count**: "19 of 19 backends have `reg_isel.rs`; 15 substantive + 4 wrappers" (not "18 of 19: 14 native + 4 wrappers").
6. **Correct the V-13 AArch64 claim**: AArch64 NEON only supports 4×i32 (`4S`), not `{i32, i64}`. There is no `2D` encoder.
7. **File V-A2-3** (SIMD hardcoded registers) and **V-A2-4** (Transform/BulkCopy/Channel no-ops) as P1 follow-ups — they're not in the catalog but cause real test failures.
8. **File V-A2-9** (regalloc syscall arg/dst interference — the prerequisite for removing the `contains_fork` opt-out) as a P1 follow-up. Currently the opt-out masks the bug; removing the opt-out before fixing the regalloc would re-expose ~30 MM failures on x86_64 alone.
9. **Add regression tests** for V-34, V-35, V-36, V-A2-1, V-A2-2 — currently zero code-level coverage for any of the P0 bridge bugs.
10. **Add a per-backend smoke test** for the 15 non-wrapper backends — `wrapper_smoke.rs` only covers the 4 wrappers.

### V-A2-9 — regalloc doesn't model syscall arg/dst interference (root cause of `contains_fork` opt-out) — **P1, masked workaround**

**Severity**: P1 — currently masked by the `contains_fork` opt-out (which falls back to stack-slot ISel for any function containing clone/vfork/spawn_worker, plus any syscall with a Register arg + dst on x86_64). Removing the opt-out before fixing this would re-expose ~30+ MM failures.
**Files**: `src/codegen/src/regalloc.rs` (linear-scan allocator), `src/codegen/src/backend.rs:3230–3242` (aarch64 opt-out), `src/codegen/src/x86_64/mod.rs:4349–4384` (x86_64 opt-out + W4-fix broadening).

The x86_64 backend's comment at `mod.rs:4368–4376` is explicit:

```
// W4-fix: ALSO fall back for ANY syscall that has a
// Register arg AND a dst (return value). These are
// the syscalls with the register-reuse hazard (e.g.
// try_recv's read() syscall). This is overly broad
// but safe — the stack-slot path handles all syscalls
// correctly. The regalloc path can be re-enabled for
// these once the allocator's live-range analysis is
// fixed to not reuse an arg's register for the dst
// when the arg is live across the syscall.
```

The Phase 1b fix from `1d72d296` (CFG-aware interval extension at `regalloc.rs:1074–1102`) addresses loop-back-edge liveness but does **not** model the specific interference between a syscall's arg vregs and its dst vreg. A syscall is modeled as a single instruction that uses its arg vregs and defines its dst vreg — but the linear-scan allocator may still assign the dst to the same physical register as one of the args if the arg's interval ends at the syscall (which is wrong: the arg must remain live until after the syscall completes, because the syscall reads the arg's physical register to set up the kernel call).

**Fix sketch**: extend `LiveRangeComputer::compute` to treat `IRInstr::Syscall { .. }` (and `IRInstr::Call { .. }`) specially — for each syscall, mark all arg vregs as live-through (extend their intervals past the syscall's position) regardless of whether they have a later use. This is similar to how `crosses_call` is already tracked at `regalloc.rs:1104–1112`, but applied to arg-specific liveness rather than just callee-saved register spilling.
**Effort**: 1 week (regalloc change + per-backend re-enablement of the `contains_fork` opt-out + regression tests).
