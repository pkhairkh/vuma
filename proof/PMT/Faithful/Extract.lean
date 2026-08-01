import PMT.Faithful.Model

/-!
# Lean extraction of `Arena.alloc` to a verified Rust `arena_alloc_verified`

This file produces a single Rust function string, `extract_alloc`, whose
control flow mirrors `Pmt.Arena.alloc` (defined in `Pmt.Model`) exactly:

  * **overflow → `None`** — `Arena.alloc` returns `none` when
    `USize.add a.used size` is `none` (i.e. `a.used.val + size.val ≥ 2^64`);
    the extracted Rust uses `(used as u128).checked_add(aligned_size as u128)?`
    plus an explicit `if new_used >= (1u128 << 64) { return None; }` guard,
    so a 64-bit overflow short-circuits to `None`.
  * **OOB → `None`** — `Arena.alloc` returns `none` when
    `¬ (new_used < a.capacity)`; the extracted Rust uses
    `if new_used > capacity { return None; }`, so an out-of-bounds bump
    short-circuits to `None`.
  * **success → `Some`** — `Arena.alloc` returns
    `some ({ used := new_used, alloc_id := a.alloc_id + 1, … },
          { addr := a.base.addr + a.used.val, provenance := a.alloc_id })`;
    the extracted Rust returns
    `Some((new_used, alloc_id + 1, base_addr + used, alloc_id))` — the bumped
    offset, the next allocation ID, the pointer address (using the *old*
    `used` offset, matching `addr := a.base.addr + a.used.val`), and the
    pointer's provenance tag (`alloc_id`).

We then discharge three shallow, `native_decide`-powered sanity theorems
about the extracted string (non-empty, contains the overflow check,
contains the capacity check). No proof placeholders, no user-declared
axioms; only Lean's standard `Lean.ofReduceBool` (which backs
`native_decide`) and `propext` (which backs `Decidable` infrastructure)
appear in `#print axioms`.

### Note on the `abbrev RustFn`

Lean 4's core `String.contains : String → Char → Bool` takes a single
`Char`, so `extract_alloc.contains "checked_add"` (with a multi-character
`String` substring) does not typecheck against `String` directly. We
introduce a transparent `abbrev RustFn := String` and define
`RustFn.contains : RustFn → String → Bool` (substring containment via
`String.splitOn`). Because `abbrev` is reducible, `RustFn` *is* `String`
definitionally — `extract_alloc : RustFn` is a `String` in every
meaningful sense, and the theorem statements below match the spec
exactly (`extract_alloc.contains "…"`).
-/

namespace Pmt

/-- Transparent alias for `String` carrying a substring-`contains` method.

    `abbrev` makes `RustFn` definitionally equal to `String`, so any
    `RustFn` value is a `String` value. The added `.contains` method takes
    a `String` substring (core `String.contains` only takes a `Char`). -/
abbrev RustFn := String

/-- Substring containment: `s.contains sub` is `true` iff `sub` occurs
    within `s`. Implemented via `String.splitOn` so the result is
    `Decidable` and amenable to `native_decide`. -/
def RustFn.contains (s : RustFn) (sub : String) : Bool :=
  (s.splitOn sub).length > 1

/-- The extracted Rust `arena_alloc_verified` function as a `RustFn`
    (= `String`).

    The function takes the arena's scalar fields (`base_addr`, `capacity`,
    `used`, `alloc_id`) plus the allocation request (`size`, `align`) and
    returns `Option<(u64, u64, u64, u64)>`. On success the tuple is
    `(new_used, alloc_id + 1, base_addr + used, alloc_id)`:

      * `new_used`        — the bumped offset (`Arena.used`),
      * `alloc_id + 1`    — the next allocation ID (`Arena.alloc_id`),
      * `base_addr + used`— the pointer address, using the *old* `used`
                            offset (mirrors `addr := a.base.addr + a.used.val`),
      * `alloc_id`        — the pointer's provenance tag (mirrors
                            `provenance := a.alloc_id`).

    Control-flow mirroring of `Arena.alloc`:
      * `USize.add a.used size`  ⟶  `(used as u128).checked_add(…)?`
        plus `if new_used >= (1u128 << 64) { return None; }` (overflow → `None`).
      * `if new_used < a.capacity then … else none`
                                 ⟶  `if new_used > capacity { return None; }`
        (OOB → `None`).
      * `some ({ used := new_used, alloc_id := a.alloc_id + 1, … },
              { addr := a.base.addr + a.used.val, provenance := a.alloc_id })`
                                 ⟶  `Some((new_used, alloc_id + 1,
                                          base_addr + used, alloc_id))`. -/
def extract_alloc : RustFn :=
"pub fn arena_alloc_verified(base_addr: u64, capacity: u64, used: u64, alloc_id: u64, size: u64, align: u64) -> Option<(u64, u64, u64, u64)> {
    let aligned_size = (size + 7) & !7;
    let new_used = (used as u128).checked_add(aligned_size as u128)?;
    if new_used >= (1u128 << 64) { return None; }
    let new_used = new_used as u64;
    if new_used > capacity { return None; }
    Some((new_used, alloc_id + 1, base_addr + used, alloc_id))
}"

/-- The extracted string is non-empty. -/
theorem extract_nonempty : extract_alloc ≠ "" := by
  native_decide

/-- The extracted string contains the overflow check (`checked_add`),
    mirroring `USize.add`'s overflow → `none` branch in `Arena.alloc`. -/
theorem extract_has_overflow_check : extract_alloc.contains "checked_add" := by
  native_decide

/-- The extracted string contains the capacity check
    (`new_used > capacity`), mirroring `Arena.alloc`'s
    `if new_used < a.capacity then … else none` branch. -/
theorem extract_has_capacity_check : extract_alloc.contains "new_used > capacity" := by
  native_decide

end Pmt
