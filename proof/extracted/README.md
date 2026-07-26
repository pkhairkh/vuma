# Extracted Verified Checkers

This directory contains the Rust translation of the Lean-verified bounds-checking
logic from `proof/PMT/Extraction.lean`.

> **Wave 0 status (IVE-0-B).** The `@[export]` annotations on
> `proof/PMT/Extraction.lean` are in place, but the Rust→C FFI bridge
> described in §Stage 2 below is **deferred to Wave 1** (IVE-1-*). The
> Rust checkers currently shipped at `src/codegen/src/runtime/pmt_check.rs`
> are a **hand-translation** of the Lean definitions, cross-checked by the
> parity test `tests/pmt_parity_test.rs` (5 tests on the 3 trivial checkers
> + the composed check). The Lean→Rust FFI linkage (build.rs that locates
> `.lake/build/ir/PMT_Extraction.c` + the Lean runtime, `extern "C"`
> declarations matching Lean's `lean_object*` ABI, runtime init, and
> marshaling code) is non-trivial as a code-only change without build
> verification and was therefore deferred. See `## Current Status` below.

## Source of Truth

The Lean definitions in `proof/PMT/Extraction.lean` are the formal specification.
Each function has a machine-checked soundness theorem:

| Lean Function | Soundness Theorem | Statement |
|---------------|-------------------|-----------|
| `verified_capacity_check` | `verified_capacity_check_correct` | If check returns true, then `used + size ≤ capacity` |
| `verified_field_bounds_check` | `verified_field_bounds_check_correct` | If check returns true, then `f.offset + f.size ≤ layout.total_size` |
| `verified_linearity_check` | `verified_linearity_check_correct` | If check returns true, then `var ∉ consumed` |
| `verified_pmt_check` | `verified_pmt_check_correct` | If check returns true, all three sub-checks hold |

## Extraction Pipeline

The extraction happens in three stages:

### Stage 1: Lean → C
`lake build` produces `.c` files in `proof/.lake/build/ir/`. These are the
Lean compiler's C backend output. The key files:
- `PMT_Extraction.c.o` — compiled object file
- `PMT_Extraction.olean` — Lean interface file

### Stage 2: C → Rust FFI  _(Wave 1 target — not yet implemented)_

The planned Rust wrapper module (`src/codegen/src/runtime/pmt_check.rs`)
would provide `extern "C"` declarations that call into the compiled Lean
C code. The exact signature sketch (subject to Lean 4.21's `lean_object*`
ABI for `Nat`/`String`/`List String`/`Field`/`Layout`, plus
`lean_initialize()` runtime init and `lean_dec()` for returned objects):

```rust
extern "C" {
    fn lean_verified_capacity_check(used: u64, size: u64, capacity: u64) -> bool;
    fn lean_verified_field_bounds_check(f_offset: u64, f_size: u64,
                                        layout_total: u64) -> bool;
    fn lean_verified_linearity_check(var: *const u8, var_len: usize,
                                     consumed: *const *const u8,
                                     consumed_len: usize) -> bool;
}

pub fn verified_capacity_check(used: u64, size: u64, capacity: u64) -> bool {
    unsafe { lean_verified_capacity_check(used, size, capacity) }
}
```

The actual `src/codegen/src/runtime/pmt_check.rs` today is a
**hand-translation** (not `extern "C"`); the signature sketch above is
the Wave 1 target once the build.rs + Lean-runtime linkage work lands.

### Stage 3: Integration + Parity Test  _(Wave 1 target — partial today)_

Wave 1 plan:
- Add `pmt-runtime-check` feature flag to `Cargo.toml` _(already done)_
- When enabled, the compiler would call the Lean-verified checkers via
  FFI instead of the hand-written Rust ones _(not yet — the feature is
  wired in `arena.rs` but currently dispatches to the hand-translation)_
- Parity test: run both checkers on all 1,536 gold-standard `.vuma`
  fixtures and verify they agree _(not yet — current parity test is
  5 cases on the 3 trivial checkers + the composed check, no fixtures)_

Today's partial state:
- `pmt-runtime-check` feature: defined in `src/codegen/Cargo.toml`,
  forwarded from root `Cargo.toml`.
- Feature is wired in `src/codegen/src/runtime/arena.rs` so the
  hand-translated `verified_capacity_check` runs on every arena alloc.
- Parity test `tests/pmt_parity_test.rs`: 5 tests on the 3 trivial
  checkers + composed check; matches expected Lean semantics computed
  by hand.

## Current Status

- ✅ Lean checkers defined and proven
- ✅ `@[export]` annotations on `proof/PMT/Extraction.lean` reserve the C
  symbols (`lean_verified_capacity_check`, `lean_verified_field_bounds_check`,
  `lean_verified_linearity_check`, `lean_verified_pmt_check`)
- ⏳ C extraction via `lake build` — works, produces `PMT_Extraction.c`
- ⏳ **Rust FFI wrapper — DEFERRED to Wave 1 (IVE-1-*)**; current Rust
  path is the hand-translation at `src/codegen/src/runtime/pmt_check.rs`
- ⏳ **Integration + parity test — PARTIAL**; `pmt-runtime-check` feature
  is wired into `arena.rs` (calling the hand-translation), and a 5-test
  parity test exists on the 3 trivial + composed checkers. Full
  1,536-fixture differential vs. the Lean C output is Wave 1.

## Build

```bash
# Build the Lean checkers (produces .c files)
cd proof && lake build PMT.Extraction

# The .c files are in:
ls .lake/build/ir/PMT_Extraction.c
```

## Verification

The soundness of each checker is machine-checked by Lean:

```bash
cd proof && lake build PMT.Extraction
# No sorry warnings — all theorems proven
```

## References

- `proof/PMT/Extraction.lean` — Lean source + soundness theorems
