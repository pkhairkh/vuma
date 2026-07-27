import PMT.Basic

/-!
## IVE Soundness — L1L3Collapse (Wave 2 task IVE-2-E)

This module proves that IVE's `l1l3_collapse` check is sound: if the
collapse succeeds (L1 checks folded into L3), then every L1 check that
was discharged at compile time is valid — i.e., the type_hash matches
the IRType for each ChannelSend.

The Lean model mirrors the Rust function's specification. The actual Rust
function lives at `src/ive/src/verification.rs` (l1l3_collapse_from_ir).

This module is `sorry`-free.
-/

namespace PMT.IVE.Soundness

/-- Helper: hash a string to a Nat. This is a simplified model of the
BD inference's type_hash function. -/
def hash_string (s : String) : Nat :=
  s.foldl (fun acc c => acc + c.val.toNat) 0

/-- An L1 check: a type_hash that should match an IRType for a ChannelSend.
Mirrors the L1-level check that gets discharged at compile time. -/
structure L1Check where
  type_hash : Nat
  ir_type   : String
  deriving Repr

/-- The L1 check is valid if the type_hash matches the IRType's hash. -/
def l1_check_valid (c : L1Check) : Bool :=
  decide (c.type_hash = hash_string c.ir_type)

/-- An L3 check: the collapsed form of an L1 check. After L1L3 collapse,
the L1 check is replaced by an L3 check that's discharged at runtime
(only if the L1 check couldn't be folded). -/
structure L3Check where
  type_hash : Nat
  deriving Repr

/-- The L1L3 collapse: given a list of L1 checks, produce the list of
L3 checks that remain (i.e., the L1 checks that could NOT be discharged
at compile time). An L1 check is discharged iff l1_check_valid = true. -/
def l1l3_collapse (checks : List L1Check) : List L3Check :=
  checks.filterMap fun c =>
    if l1_check_valid c then none  -- discharged (folded into L3, no runtime check needed)
    else some { type_hash := c.type_hash : L3Check }  -- not discharged, remains as L3

/-- Soundness: if an L1 check is discharged (not in the L3 output), then
the L1 check is valid (type_hash matches ir_type). This is the Lean
rendering of the soundness obligation for the L1L3 collapse. -/
theorem l1l3_collapse_sound
    (checks : List L1Check)
    (c : L1Check)
    (h_mem : c ∈ checks)
    (h_not_in_l3 : ¬ c.type_hash ∈ (l1l3_collapse checks).map (fun l3 => l3.type_hash)) :
    l1_check_valid c = true := by
  -- If l1_check_valid c were false, then c would produce an L3Check with
  -- c.type_hash, contradicting h_not_in_l3.
  cases h_valid : l1_check_valid c with
  | true => rfl
  | false =>
    -- c produces an L3Check in the output.
    have h_in : { type_hash := c.type_hash : L3Check } ∈ l1l3_collapse checks := by
      rw [l1l3_collapse, List.mem_filterMap]
      refine ⟨c, h_mem, ?_⟩
      rw [h_valid]
      simp
    -- Its type_hash is in the mapped list.
    have h_hash_in : c.type_hash ∈ (l1l3_collapse checks).map (fun l3 => l3.type_hash) := by
      rw [List.mem_map]
      refine ⟨{ type_hash := c.type_hash : L3Check }, h_in, rfl⟩
    exact absurd h_hash_in h_not_in_l3

/-- Corollary: if ALL L1 checks are discharged (L3 output is empty), then
every L1 check is valid. -/
theorem l1l3_collapse_all_discharged
    (checks : List L1Check)
    (hverify : l1l3_collapse checks = []) :
    ∀ c : L1Check, c ∈ checks → l1_check_valid c = true := by
  intro c h_mem
  -- If c is in checks and the L3 output is empty, then c's type_hash is
  -- not in the (empty) mapped list.
  have h_not_in_l3 : ¬ c.type_hash ∈ (l1l3_collapse checks).map (fun l3 => l3.type_hash) := by
    rw [hverify]
    intro h
    cases h
  exact l1l3_collapse_sound checks c h_mem h_not_in_l3

end PMT.IVE.Soundness
