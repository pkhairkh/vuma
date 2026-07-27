import PMT.Basic

/-!
## IVE Soundness — InformationFlow (Wave 2 task IVE-2-C)

This module proves that IVE's `verify_information_flow` function is sound:
if it accepts a program (no flow violations), then no high-security value
flows to a low-security sink — i.e., the Denning security-lattice invariant
holds.

The Lean model mirrors the Rust function's specification. The actual Rust
function lives at `src/ive/src/information_flow.rs`.

**Rust reference** (`src/ive/src/information_flow.rs::verify_information_flow`):
```rust
pub fn verify_information_flow(events: &[FlowEvent]) -> Vec<FlowViolation>
```
The Rust function walks a list of `FlowEvent`s and checks that each flow
respects the security lattice `Public ⊑ Internal ⊑ Secret ⊑ TopSecret`.
A flow from label `src` to label `dst` is permitted iff `src ⊑ dst`.

**Wave 2 task IVE-2-C scope**: The Lean proof covers the lattice-checking
logic (the core soundness guarantee). The Rust-side annotation threading
(parsing `#[secret]` annotations and lowering them to IR-level FlowEvents
with real labels, instead of the current hardcoded `Public`) is a
parser/codegen change documented as a known gap; the Lean proof is valid
for whatever labels the Rust side produces, as long as they respect the
lattice.

This module is `sorry`-free.
-/

namespace PMT.IVE.Soundness

/-- Security label in the Denning lattice.
Mirrors Rust `SecurityLabel` in `src/ive/src/information_flow.rs`.
The ordering is `Public ⊑ Internal ⊑ Secret ⊑ TopSecret`. -/
inductive SecurityLabel where
  | public   : SecurityLabel
  | internal : SecurityLabel
  | secret   : SecurityLabel
  | topsecret: SecurityLabel
  deriving Repr, DecidableEq, BEq

/-- The lattice order: `l1 ⊑ l2` means information can flow from `l1` to `l2`.
  - Public flows anywhere.
  - Internal flows to Internal, Secret, TopSecret.
  - Secret flows to Secret, TopSecret.
  - TopSecret flows only to TopSecret. -/
def flows_to : SecurityLabel → SecurityLabel → Bool
  | SecurityLabel.public,    _                              => true
  | SecurityLabel.internal,  SecurityLabel.internal         => true
  | SecurityLabel.internal,  SecurityLabel.secret           => true
  | SecurityLabel.internal,  SecurityLabel.topsecret        => true
  | SecurityLabel.secret,    SecurityLabel.secret           => true
  | SecurityLabel.secret,    SecurityLabel.topsecret        => true
  | SecurityLabel.topsecret, SecurityLabel.topsecret        => true
  | _,                        _                              => false

/-- A flow event: information flows from `src_label` to `dst_label`.
Mirrors a simplified `FlowEvent` (the Rust version has more fields like
`dst_vreg`, `at_node`, etc., but the soundness-critical part is just the
two labels). -/
structure FlowEvent where
  src_label : SecurityLabel
  dst_label : SecurityLabel
  deriving Repr

/-- A flow violation: a flow that doesn't respect the lattice. -/
structure FlowViolation where
  event : FlowEvent
  reason : String
  deriving Repr

/-- The Lean model of IVE's `verify_information_flow`.
Returns one `FlowViolation` per event that doesn't respect the lattice. -/
def verify_information_flow (events : List FlowEvent) : List FlowViolation :=
  events.filterMap fun e =>
    if flows_to e.src_label e.dst_label then none
    else some { event := e, reason := "flow violates security lattice" }

/-- Soundness: if `verify_information_flow` returns no violations, then
every flow respects the lattice (`src ⊑ dst`). This is the Lean rendering
of the soundness obligation for
`src/ive/src/information_flow.rs::verify_information_flow`. -/
theorem verify_information_flow_sound
    (events : List FlowEvent)
    (hverify : verify_information_flow events = [])
    (e : FlowEvent)
    (h_mem : e ∈ events) :
    flows_to e.src_label e.dst_label = true := by
  -- If flows_to were false, then `some {event := e, …}` would be in the
  -- output list (by List.filterMap), contradicting hverify : output = [].
  -- We case-split on flows_to; the false case leads to contradiction.
  cases h_flow : flows_to e.src_label e.dst_label with
  | true =>
    -- After cases, the goal's `flows_to e.src_label e.dst_label` is
    -- rewritten to `true` (by h_flow), so the goal is `true = true`.
    rfl
  | false =>
    -- Derive the contradiction: the violation is in the output list.
    have h_in : ({ event := e, reason := "flow violates security lattice" : FlowViolation })
        ∈ verify_information_flow events := by
      rw [verify_information_flow, List.mem_filterMap]
      refine ⟨e, h_mem, ?_⟩
      rw [h_flow]
      rfl
    -- But hverify says the output is [], so no element can be in it.
    rw [hverify] at h_in
    -- h_in : {…} ∈ [] — impossible (empty list has no elements).
    cases h_in

/-- Corollary: no Secret flows to Public. If all flows pass verification,
then no event has src_label = Secret and dst_label = Public (a direct leak). -/
theorem verify_information_flow_no_secret_to_public
    (events : List FlowEvent)
    (hverify : verify_information_flow events = [])
    (e : FlowEvent)
    (h_mem : e ∈ events)
    (h_src : e.src_label = SecurityLabel.secret)
    (h_dst : e.dst_label = SecurityLabel.public) :
    False := by
  have h_flow := verify_information_flow_sound events hverify e h_mem
  unfold flows_to at h_flow
  rw [h_src, h_dst] at h_flow
  -- flows_to Secret Public = false, but h_flow says = true → contradiction.
  simp at h_flow

end PMT.IVE.Soundness
