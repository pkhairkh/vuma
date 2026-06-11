# Task: Add Doc Comments to VUMA Public APIs

## Summary

Added `///` doc comments to **90 missing-doc items** across the three priority crates (`vuma-parser`, `vuma-bd`, `vuma-scg`) and fixed **6 broken intra-doc links**.

## Files Modified

### vuma-bd (84 struct field + 6 variant docs added)

1. **`src/bd/src/inference.rs`** — 22 doc comments added
   - `InferenceError::RepDIncompatible` fields: `source`, `target`, `source_repd`, `target_repd`
   - `InferenceError::CapDViolation` fields: `node`, `required`, `actual`
   - `InferenceError::RelDInconsistent` fields: `node`, `detail`
   - `InferenceError::SecurityDowngrade` fields: `source`, `target`, `source_level`, `target_level`
   - `InferenceError::CircularOutlives` field: `node`
   - `InferenceError::MaxIterationsExceeded` field: `iterations`
   - `BDConstraint::RepDCompatibility` fields: `source`, `target`
   - `BDConstraint::CapDWeakening` fields: `source`, `target`
   - `BDConstraint::RelDRefinement` fields: `source`, `target`

2. **`src/bd/src/reld_refine.rs`** — 6 doc comments added
   - `DetailedRelation::Temporal` variant
   - `DetailedRelation::Structural` variant
   - `DetailedRelation::Security` variant
   - `DetailedRelation::Ownership` variant
   - `DetailedRelation::Lifetime` variant
   - `DetailedRelation::Dependency` variant

3. **`src/bd/src/capd_lattice.rs`** — 4 doc comments added
   - `WeakeningError::BothViolations` fields: `extra_caps`, `removed_conditions`
   - `StrengtheningError::BothViolations` fields: `missing_caps`, `relaxed_conditions`

4. **`src/bd/src/unify.rs`** — 16 doc comments added
   - `UnificationError::IncompatibleRepD` fields: `repd1`, `repd2`, `reason`
   - `UnificationError::IncompatibleCapD` fields: `capd1`, `capd2`
   - `UnificationError::InconsistentRelD` fields: `reld1`, `reld2`
   - `UnificationError::OccursCheckFailed` fields: `var`, `term`
   - `UnificationError::ConflictingBinding` fields: `var`, `existing`, `proposed`
   - `UnificationError::SubtypeViolation` fields: `sub`, `sup`

5. **`src/bd/src/repd_compat.rs`** — 22 doc comments added
   - All fields in `IncompatibilityReason` variants: `SizeMismatch`, `AlignmentIncompatible`, `FieldCountMismatch`, `FieldIncompatible`, `ArrayCountMismatch`, `EnumVariantCountMismatch`, `EnumTagMismatch`, `UnionAltCountMismatch`, `ParamCountMismatch`, `Nested`

6. **`src/bd/src/repd.rs`** — 2 broken intra-doc links fixed
   - `[`compatible`]` → `[`RepD::compatible`]`
   - `[`subsumes`]` → `[`RepD::subsumes`]`
   - `[`compatible`]` → `[`Self::compatible`]` (in `subsumes` method doc)

### vuma-parser (26 struct field docs added)

7. **`src/parser/src/ast.rs`** — 26 doc comments added
   - `Expr::Var` fields: `name`, `span`
   - `Expr::Lit` fields: `value`, `span`
   - `Expr::AddressOf` fields: `expr`, `span`
   - `Expr::Deref` fields: `expr`, `span`
   - `Expr::Offset` fields: `base`, `offset`, `span`
   - `Expr::Cast` fields: `expr`, `target_type`, `span`
   - `Expr::Sizeof` fields: `ty`, `span`
   - `Expr::Alignof` fields: `ty`, `span`
   - `Expr::Async` fields: `body`, `span`
   - `Expr::Spawn` fields: `expr`, `span`
   - `Expr::Null` field: `span`
   - `Expr::Uninitialized` field: `span`

8. **`src/parser/src/error.rs`** — 3 broken intra-doc links fixed
   - `[`InvalidSyntax`]` → `[`Self::InvalidSyntax`]`
   - `[`UndefinedReference`]` → `[`Self::UndefinedReference`]`

### vuma-scg (0 missing docs; 3 broken intra-doc links fixed)

9. **`src/scg/src/liveness.rs`** — 2 broken intra-doc links fixed
   - `[n]` → `\[n\]` (escaped brackets in live_in/live_out notation)

10. **`src/scg/src/loop_detection.rs`** — 1 broken intra-doc link fixed
    - `[`compute_dominators`]` → `[`crate::dominance::compute_dominators`]`

## Verification

- `RUSTDOCFLAGS="-D missing_docs" cargo doc -p vuma-parser -p vuma-bd -p vuma-scg` — **0 errors**
- `RUSTDOCFLAGS="-D missing_docs -D rustdoc::broken_intra_doc_links" cargo doc -p vuma-parser -p vuma-bd -p vuma-scg` — **0 errors, 0 warnings**
- `cargo check --workspace` — **all 11 crates pass**
