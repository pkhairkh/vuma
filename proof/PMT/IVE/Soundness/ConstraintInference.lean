import PMT.Basic

/-!
## IVE Soundness — ConstraintInference (FAITHFUL model, Wave 5 task IVE-FAITH-5-C)

This module is a **bit-faithful** Lean rendering of the Rust constraint
system in `src/ive/src/constraint.rs`. It replaces the previous (unfaithful)
model that used arithmetic constraints (le, ge, eq). The Rust system uses
5 string-description-based constraint types with string-containment checks.

**Rust reference** (`src/ive/src/constraint.rs`):
  - 5 constraint types, each with `description: String`:
    - `TemporalConstraint` — checks `self.description.contains(a) && self.description.contains(b)`
      for each `(a, b)` in `model.temporal_violations`.
    - `ResourceFlowConstraint` — same pattern with `model.blocked_flows`.
    - `SecurityConstraint` — checks `self.description.contains(violation)` for each in `model.security_violations`.
    - `ComplexityConstraint` — checks `!model.complexity_exceeded`.
    - `LivenessConstraint` — checks `!model.has_unanswered_requests`.
  - `ModelState` has: `observed_events`, `temporal_violations: Vec<(String,String)>`,
    `blocked_flows: Vec<(String,String)>`, `security_violations: Vec<String>`,
    `complexity_exceeded: bool`, `has_unanswered_requests: bool`.

This module is `sorry`-free.
-/

namespace PMT.IVE.Soundness

/-- ModelState mirroring Rust `src/ive/src/constraint.rs::ModelState`:
`{observed_events, temporal_violations, blocked_flows, security_violations, complexity_exceeded, has_unanswered_requests}`. -/
structure ModelState where
  observed_events         : List String
  temporal_violations     : List (String × String)
  blocked_flows           : List (String × String)
  security_violations     : List String
  complexity_exceeded     : Bool
  has_unanswered_requests : Bool
  deriving Repr

/-- The 5 constraint types mirroring Rust. Each has a `description : String`.
The `check_against` logic matches Rust's string-containment pattern. -/
inductive Constraint where
  | temporal      : String → Constraint
  | resource_flow : String → Constraint
  | security      : String → Constraint
  | complexity    : String → Constraint
  | liveness      : String → Constraint
  deriving Repr

/-- Helper: does `desc` contain `s` as a substring? Models Rust's `String::contains`.
Uses `List Char` representation and `List.isPrefixOf` for substring search. -/
def string_contains (desc s : String) : Bool :=
  let s_chars := s.toList
  if s_chars.isEmpty then true
  else
    -- Check if s_chars is a prefix of any suffix of desc.toList.
    let rec check : List Char → Bool
      | [] => false
      | l@(_ :: rest) => s_chars.isPrefixOf l || check rest
    check desc.toList

/-- Check a constraint against the model state. **Faithful** to Rust's
`check_against` for each constraint type:
  - `temporal`: returns false if any `(a, b)` in `model.temporal_violations`
    has `description.contains(a) && description.contains(b)`.
  - `resource_flow`: same pattern with `model.blocked_flows`.
  - `security`: returns false if any `v` in `model.security_violations`
    has `description.contains(v)`.
  - `complexity`: returns `!model.complexity_exceeded`.
  - `liveness`: returns `!model.has_unanswered_requests`. -/
def Constraint.check_against (c : Constraint) (model : ModelState) : Bool :=
  match c with
  | Constraint.temporal desc =>
    -- Rust: for (a, b) in &model.temporal_violations {
    --   if self.description.contains(a) && self.description.contains(b) { return false; }
    -- }
    -- true (if no violation matched)
    ¬ model.temporal_violations.any (fun (a, b) => string_contains desc a && string_contains desc b)
  | Constraint.resource_flow desc =>
    ¬ model.blocked_flows.any (fun (src, tgt) => string_contains desc src && string_contains desc tgt)
  | Constraint.security desc =>
    ¬ model.security_violations.any (fun v => string_contains desc v)
  | Constraint.complexity _ =>
    ¬ model.complexity_exceeded
  | Constraint.liveness _ =>
    ¬ model.has_unanswered_requests

/-- The Lean model of IVE's constraint verification. Given a list of
constraints and a model state, return the list of unsatisfied constraints
(those where `check_against` returns false). Mirrors the Rust pattern
where `verify_pmt` calls `check_against` for each constraint. -/
def verify_constraints (constraints : List Constraint) (model : ModelState) : List Constraint :=
  constraints.filter fun c => ¬ c.check_against model

/-- Soundness: if `verify_constraints` returns no unsatisfied constraints,
then every constraint is satisfied by the model state (`check_against` returns true).
This is the Lean rendering of the soundness obligation for the constraint system. -/
theorem verify_constraints_sound
    (constraints : List Constraint) (model : ModelState)
    (hverify : verify_constraints constraints model = []) :
    ∀ c : Constraint, c ∈ constraints → c.check_against model = true := by
  intro c h_mem
  -- If c.check_against model were false, then c would be in the unsatisfied list.
  cases h_check : c.check_against model with
  | true => rfl
  | false =>
    -- c is in the filter output (since ¬ false = true).
    have h_in : c ∈ verify_constraints constraints model := by
      rw [verify_constraints, List.mem_filter]
      refine ⟨h_mem, ?_⟩
      rw [h_check]
      simp
    rw [hverify] at h_in
    cases h_in

end PMT.IVE.Soundness
