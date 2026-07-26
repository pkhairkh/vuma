# Extracted Verified Checkers

This directory contains the Rust translation of the Lean-verified bounds-checking
logic from `proof/PMT/Extraction.lean`.

## Source of Truth

The Lean definitions in `proof/PMT/Extraction.lean` are the formal specification.
Each function has a machine-checked soundness theorem:

| Lean Function | Soundness Theorem | Statement |
|---------------|-------------------|-----------|
| `verified_capacity_check` | `verified_capacity_check_correct` | If check returns true, then `used + size ≤ capacity` |
| `verified_field_bounds_check` | `verified_field_bounds_check_correct` | If check returns true, then `f.offset + f.size ≤ layout.total_size` |
| `verified_linearity_check` | `verified_linearity_check_correct` | If check returns true, then `var ∉ consumed` |
| `verified_pmt_check` | `verified_pmt_check_correct` | If check returns true, all three sub-checks hold |

## Extraction Pipeline (Waves 27-29)

The extraction happens in three stages:

### Stage 1 (Wave 27): Lean → C
`lake build` produces `.c` files in `proof/.lake/build/ir/`. These are the
Lean compiler's C backend output. The key files:
- `PMT_Extraction.c.o` — compiled object file
- `PMT_Extraction.olean` — Lean interface file

### Stage 2 (Wave 28): C → Rust FFI
A Rust wrapper module (`src/codegen/src/runtime/pmt_check.rs`) provides
`extern "C"` declarations that call into the compiled Lean C code:

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

### Stage 3 (Wave 29): Integration + Parity Test
- Add `pmt-runtime-check` feature flag to `Cargo.toml`
- When enabled, the compiler uses the Lean-verified checkers instead of
  the hand-written Rust ones
- Parity test: run both checkers on all 1,536 gold-standard `.vuma` fixtures
  and verify they agree

## Current Status

- ✅ Lean checkers defined and proven (Wave 11)
- ⏳ C extraction via `lake build` — works, produces `PMT_Extraction.c`
- ⏳ Rust FFI wrapper — TODO Wave 28
- ⏳ Integration + parity test — TODO Wave 29

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
- `docs/verification-reports/W3-wave-plan.md` — Waves 27-29 plan
- `docs/verification-reports/W11-build-test.md` — Wave 11 status
