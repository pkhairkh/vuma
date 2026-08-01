import PMT.Basic

/-!
## IVE Soundness — InformationFlow (FAITHFUL model, Wave 6 task IVE-FAITH-6-A)

This module is a **bit-faithful** Lean rendering of the Rust function
`src/ive/src/information_flow.rs::verify_information_flow`. It replaces
the previous (unfaithful) model that had only 1 flow kind and NO implicit
flow — a soundness hole.

**Rust reference** (`src/ive/src/information_flow.rs`):
  - `SecurityLabel`: Public, Internal, Secret, TopSecret (total order).
  - `can_flow_to(self, other)`: `self ⊑ other` (Public flows anywhere, etc.).
  - `join(self, other)`: LUB (Public∧x=x, Internal∧x=x, Secret∧x=x, TopSecret∧TopSecret=TopSecret).
  - `FlowKind`: 4 variants — Assign, BinOp, ChannelSend, Branch (implicit flow).
  - `verify_information_flow`: sorts by at_node, checks each event.

This module is `sorry`-free.
-/

namespace PMT.IVE.Soundness

/-- SecurityLabel mirroring Rust `SecurityLabel`:
Public ⊑ Internal ⊑ Secret ⊑ TopSecret. -/
inductive SecurityLabel where
  | public    : SecurityLabel
  | internal  : SecurityLabel
  | secret    : SecurityLabel
  | topsecret : SecurityLabel
  deriving Repr, DecidableEq, BEq

/-- `can_flow_to l1 l2` = `l1 ⊑ l2` (l1 can flow to l2).
**Faithful** to Rust's `SecurityLabel::can_flow_to`. -/
def can_flow_to : SecurityLabel → SecurityLabel → Bool
  | SecurityLabel.public,    _                              => true
  | SecurityLabel.internal,  SecurityLabel.internal         => true
  | SecurityLabel.internal,  SecurityLabel.secret           => true
  | SecurityLabel.internal,  SecurityLabel.topsecret        => true
  | SecurityLabel.secret,    SecurityLabel.secret           => true
  | SecurityLabel.secret,    SecurityLabel.topsecret        => true
  | SecurityLabel.topsecret, SecurityLabel.topsecret        => true
  | _,                        _                              => false

/-- `join l1 l2` = LUB (least upper bound) of l1 and l2.
**Faithful** to Rust's `SecurityLabel::join`:
  - (Public, x) | (x, Public) => x
  - (Internal, x) | (x, Internal) => x
  - (Secret, x) | (x, Secret) => x
  - (TopSecret, TopSecret) => TopSecret -/
def join : SecurityLabel → SecurityLabel → SecurityLabel
  | SecurityLabel.public, x    => x
  | x, SecurityLabel.public    => x
  | SecurityLabel.internal, x  => x
  | x, SecurityLabel.internal  => x
  | SecurityLabel.secret, x    => x
  | x, SecurityLabel.secret    => x
  | SecurityLabel.topsecret, SecurityLabel.topsecret => SecurityLabel.topsecret

/-- FlowKind mirroring Rust `FlowKind` — all 4 variants:
  - `assign`: dst = src (checks `can_flow_to src dst`).
  - `binop`: dst = lhs op rhs (checks `can_flow_to (join lhs rhs) dst`).
  - `channel_send`: channel_send(ch, msg) (checks `can_flow_to msg channel`).
  - `branch`: if cond { ... } (IMPLICIT FLOW — checks `can_flow_to cond var` for each var). -/
inductive FlowKind where
  | assign       : Nat → SecurityLabel → SecurityLabel → FlowKind
  | binop        : Nat → SecurityLabel → SecurityLabel → SecurityLabel → FlowKind
  | channel_send : SecurityLabel → SecurityLabel → FlowKind
  | branch       : SecurityLabel → List SecurityLabel → FlowKind
  deriving Repr

/-- FlowEvent mirroring Rust `FlowEvent { kind: FlowKind, at_node: usize }`. -/
structure FlowEvent where
  kind    : FlowKind
  at_node : Nat
  deriving Repr

/-- FlowViolation mirroring Rust `FlowViolation { valid: bool, error: Option<String> }`. -/
structure FlowViolation where
  valid : Bool
  error : Option String
  deriving Repr

/-- Check a single FlowKind for violations. **Faithful** to Rust's
`verify_information_flow` per-event logic:
  - `assign`: `can_flow_to src_label dst_label`.
  - `binop`: `can_flow_to (join lhs_label rhs_label) dst_label`.
  - `channel_send`: `can_flow_to msg_label channel_label`.
  - `branch`: for each `var_label`, `can_flow_to cond_label var_label` (implicit flow). -/
def check_flow_kind (kind : FlowKind) : Bool :=
  match kind with
  | FlowKind.assign _ dst_label src_label =>
    can_flow_to src_label dst_label
  | FlowKind.binop _ dst_label lhs_label rhs_label =>
    can_flow_to (join lhs_label rhs_label) dst_label
  | FlowKind.channel_send channel_label msg_label =>
    can_flow_to msg_label channel_label
  | FlowKind.branch cond_label branch_var_labels =>
    branch_var_labels.all (fun var_label => can_flow_to cond_label var_label)

/-- The Lean model of IVE's `verify_information_flow`. **Faithful** to the
Rust function at `src/ive/src/information_flow.rs::verify_information_flow`:
  - Processes events (assumed sorted by at_node — or sorts them first).
  - For each event, checks the FlowKind against the security lattice.
  - Returns one FlowViolation per failing event. -/
def verify_information_flow (events : List FlowEvent) : List FlowViolation :=
  events.filterMap fun e =>
    if check_flow_kind e.kind then none
    else some { valid := false, error := some "information-flow violation" }

/-- Soundness: if `verify_information_flow` returns no violations, then
every flow respects the security lattice. This covers ALL 4 FlowKind
variants, including **implicit flow** (Branch).

**Critical**: This closes the soundness hole from the previous model
which omitted the Branch variant. A program leaking a secret via
`if secret { public = 1 }` is now REJECTED (the Branch check requires
`can_flow_to cond_label var_label` for each branch variable). -/
theorem verify_information_flow_sound
    (events : List FlowEvent)
    (hverify : verify_information_flow events = [])
    (e : FlowEvent)
    (h_mem : e ∈ events) :
    check_flow_kind e.kind = true := by
  cases h_check : check_flow_kind e.kind with
  | true => rfl
  | false =>
    have h_in : ({ valid := false, error := some "information-flow violation" : FlowViolation })
        ∈ verify_information_flow events := by
      rw [verify_information_flow, List.mem_filterMap]
      refine ⟨e, h_mem, ?_⟩
      rw [h_check]
      rfl
    rw [hverify] at h_in
    cases h_in

/-- Corollary: no implicit-flow leak. If all events pass verification,
then every Branch event has `can_flow_to cond_label var_label = true`
for all branch variables. This is the "no secret controls public" guarantee. -/
theorem verify_information_flow_no_implicit_leak
    (events : List FlowEvent)
    (hverify : verify_information_flow events = [])
    (e : FlowEvent)
    (h_mem : e ∈ events)
    (cond_label : SecurityLabel)
    (branch_var_labels : List SecurityLabel)
    (h_branch : e.kind = FlowKind.branch cond_label branch_var_labels) :
    branch_var_labels.all (fun var_label => can_flow_to cond_label var_label) = true := by
  have h_check := verify_information_flow_sound events hverify e h_mem
  rw [h_branch] at h_check
  unfold check_flow_kind at h_check
  exact h_check

end PMT.IVE.Soundness
