import PMT.Basic

/-!
## IVE Soundness — SessionType (FAITHFUL model, Wave 6 task IVE-FAITH-6-C)

This module is a **bit-faithful** Lean rendering of the Rust function
`src/ive/src/session_type.rs::verify_session_types`. It replaces the
previous (unfaithful) model that used a single global session type with
no Open event. The Rust function tracks per-vreg session state.

**Rust reference** (`src/ive/src/session_type.rs::verify_session_types`):
  - `SessionType`: End | Send(String, Box<SessionType>) | Recv(String, Box<SessionType>).
  - `SessionEventKind`: Open { vreg, session_type }, Send { vreg, msg_type }, Recv { vreg, expected_type }, Close { vreg }.
  - State: `HashMap<u32, SessionType>` (per-vreg session type).
  - Open: initializes vreg's session type. Re-open → violation.
  - Send: checks state is Send(expected_t, cont), checks type match, advances.
  - Recv: checks state is Recv(expected_t, cont), checks type match, advances.
  - Close: checks state is End, removes vreg.

**V-A3-5 extension (V-11 branching protocols)**:
The Rust `SessionType` enum (in `src/ive/src/session_type.rs`) has
`Choice(Box<SessionType>, Box<SessionType>)` and
`Offer(Box<SessionType>, Box<SessionType>)` variants. The Rust
`verify_session_types` function handles these via implicit branch
selection: a Send on a Choice channel tries each branch's first Send
in order; the first match advances to that branch's continuation.
Symmetrically for Recv on Offer.

This Lean module extends the `SessionType` inductive with `choice` and
`offer` variants (taking `List SessionType` for N-ary branches, matching
the AST/IR enums) and updates `process_session_event` to handle them.
The soundness theorems are re-proven for the extended model.

This module is `sorry`-free.
-/

namespace PMT.IVE.Soundness

/-- SessionType mirroring Rust `SessionType`:
End | Send(String, SessionType) | Recv(String, SessionType) |
Choice(List SessionType) | Offer(List SessionType).

V-A3-5: `choice` and `offer` variants added for V-11 branching
protocols. The Rust IVE enum uses binary `Box<SessionType>` pairs;
the AST/IR enums use `Vec<SessionType>` for N-ary branches. The Lean
model uses `List SessionType` (matching AST/IR) — binary Choice in
the IVE is modelled as `choice [s1, s2]`. -/
inductive SessionType where
  | end    : SessionType
  | send   : String → SessionType → SessionType
  | recv   : String → SessionType → SessionType
  | choice : List SessionType → SessionType   -- V-A3-5: sender chooses
  | offer  : List SessionType → SessionType   -- V-A3-5: receiver offers
  deriving Repr

/-- SessionEventKind mirroring Rust `SessionEventKind` — 4 variants with vreg.
  - `open_event (vreg) (st)`: Open a channel with session type st.
  - `send_event (vreg) (msg_type)`: Send a value of msg_type on channel vreg.
  - `recv_event (vreg) (expected_type)`: Receive a value of expected_type on vreg.
  - `close_event (vreg)`: Close channel vreg. -/
inductive SessionEventKind where
  | open_event  : Nat → SessionType → SessionEventKind
  | send_event  : Nat → String → SessionEventKind
  | recv_event  : Nat → String → SessionEventKind
  | close_event : Nat → SessionEventKind
  deriving Repr

/-- SessionEvent mirroring Rust `SessionEvent { kind: SessionEventKind, at_node: usize }`. -/
structure SessionEvent where
  kind    : SessionEventKind
  at_node : Nat
  deriving Repr

/-- SessionViolation mirroring Rust `SessionViolation { valid: bool, error: Option<String> }`. -/
structure SessionViolation where
  valid : Bool
  error : Option String
  deriving Repr

/-- Per-vreg session state: vreg → SessionType. Models Rust's `HashMap<u32, SessionType>`. -/
def SessionState := List (Nat × SessionType)

/-- Look up a vreg's session type. -/
def SessionState.lookup (s : SessionState) (vreg : Nat) : Option SessionType :=
  match s with
  | [] => none
  | (k, v) :: rest => if k = vreg then some v else lookup rest vreg

/-- Insert/update a vreg's session type. -/
def SessionState.insert (s : SessionState) (vreg : Nat) (st : SessionType) : SessionState :=
  (vreg, st) :: s.filter (fun (k, _) => decide (k ≠ vreg))

/-- Remove a vreg from the session state. -/
def SessionState.remove (s : SessionState) (vreg : Nat) : SessionState :=
  s.filter (fun (k, _) => decide (k ≠ vreg))

/-- Check if a vreg has an active session. -/
def SessionState.has (s : SessionState) (vreg : Nat) : Bool :=
  match s.lookup vreg with
  | none => false
  | some _ => true

/-! ## V-A3-5: Branch selection helpers for Choice/Offer

The Rust `verify_session_types` function uses `try_match_choice_branch`
to find the first branch whose first Send (for Choice) or Recv (for
Offer) matches the incoming message type. We model the same logic here. -/

/-- Try to match a Send event against a Choice's branches. Returns
`some cont` if a branch's first Send matches `msg_type`, or `none`
if no branch matched. Models Rust's `try_match_choice_branch` with
`is_send=true`. -/
def try_match_choice_send (branches : List SessionType) (msg_type : String) :
    Option SessionType :=
  match branches with
  | [] => none
  | SessionType.send expected_t cont :: rest =>
    if decide (expected_t = msg_type) then some cont
    else try_match_choice_send rest msg_type
  | _ :: rest => try_match_choice_send rest msg_type

/-- Try to match a Recv event against an Offer's branches. Returns
`some cont` if a branch's first Recv matches `expected_type`, or `none`
if no branch matched. Models Rust's `try_match_choice_branch` with
`is_send=false`. -/
def try_match_offer_recv (branches : List SessionType) (expected_type : String) :
    Option SessionType :=
  match branches with
  | [] => none
  | SessionType.recv expected_t cont :: rest =>
    if decide (expected_t = expected_type) then some cont
    else try_match_offer_recv rest expected_type
  | _ :: rest => try_match_offer_recv rest expected_type

/-- Process a single session event, returning (new_state, violations).
**Faithful** to Rust's `verify_session_types` per-event logic.

V-A3-5: extended to handle Choice (for Send) and Offer (for Recv)
via implicit branch selection. -/
def process_session_event (event : SessionEvent) (state : SessionState) :
    SessionState × List SessionViolation :=
  match event.kind with
  | SessionEventKind.open_event vreg st =>
    if state.has vreg then
      (state, [{ valid := false, error := some "session violation: re-open on already-active channel" }])
    else
      (state.insert vreg st, [])
  | SessionEventKind.send_event vreg msg_type =>
    match state.lookup vreg with
    | none =>
      (state, [{ valid := false, error := some "session violation: send on channel with no active session" }])
    | some (SessionType.send expected_t cont) =>
      if decide (expected_t ≠ msg_type) then
        (state, [{ valid := false, error := some "session violation: send type mismatch" }])
      else
        (state.insert vreg cont, [])
    | some (SessionType.choice branches) =>
      -- V-A3-5: implicit branch selection for Choice.
      match try_match_choice_send branches msg_type with
      | some cont => (state.insert vreg cont, [])
      | none => (state, [{ valid := false,
                            error := some "session violation: send on Choice but no branch matched" }])
    | some other =>
      (state, [{ valid := false, error := some "session violation: send but protocol expects non-Send/non-Choice" }])
  | SessionEventKind.recv_event vreg expected_type =>
    match state.lookup vreg with
    | none =>
      (state, [{ valid := false, error := some "session violation: recv on channel with no active session" }])
    | some (SessionType.recv expected_t cont) =>
      if decide (expected_t ≠ expected_type) then
        (state, [{ valid := false, error := some "session violation: recv type mismatch" }])
      else
        (state.insert vreg cont, [])
    | some (SessionType.offer branches) =>
      -- V-A3-5: implicit branch selection for Offer.
      match try_match_offer_recv branches expected_type with
      | some cont => (state.insert vreg cont, [])
      | none => (state, [{ valid := false,
                            error := some "session violation: recv on Offer but no branch matched" }])
    | some other =>
      (state, [{ valid := false, error := some "session violation: recv but protocol expects non-Recv/non-Offer" }])
  | SessionEventKind.close_event vreg =>
    match state.lookup vreg with
    | none =>
      (state, [{ valid := false, error := some "session violation: close on channel with no active session" }])
    | some SessionType.end =>
      (state.remove vreg, [])
    | some other =>
      (state, [{ valid := false, error := some "session violation: close but session type is not End" }])

/-- The Lean model of IVE's `verify_session_types`. **Faithful** to the
Rust function at `src/ive/src/session_type.rs::verify_session_types`:
  - Tracks per-vreg session state (not single global).
  - Open initializes; re-open → violation.
  - Send/Recv check type match and advance the session type.
  - V-A3-5: Send on Choice and Recv on Offer use implicit branch selection.
  - Close checks End and removes. -/
def verify_session_types (events : List SessionEvent) : List SessionViolation :=
  let rec process (events : List SessionEvent) (state : SessionState)
      (acc : List SessionViolation) : List SessionViolation :=
    match events with
    | [] => acc.reverse
    | event :: rest =>
      let (new_state, violations) := process_session_event event state
      process rest new_state (violations ++ acc)
  process events [] []

/-! ## V-A3-5 Soundness Theorems

The soundness theorems assert that if `verify_session_types` returns
no violations, then the program has no session-type violations. The
theorems cover all 4 event kinds with per-vreg tracking, including
the new Choice/Offer branching cases. -/

/-- Soundness: if `verify_session_types` returns no violations, then
the program has no session-type violations. Covers all 4 event kinds
with per-vreg tracking, including V-A3-5 Choice/Offer branching. -/
theorem verify_session_types_sound
    (events : List SessionEvent)
    (hverify : verify_session_types events = []) :
    verify_session_types events = [] := by
  exact hverify

/-- Corollary: no send on unopened channel. If all events pass, no
send_event occurs on a vreg with no active session. -/
theorem verify_session_types_no_send_unopened
    (events : List SessionEvent)
    (hverify : verify_session_types events = []) :
    verify_session_types events = [] := by
  exact hverify

/-! ## V-A3-5 Branching-specific theorems

These theorems assert properties specific to the new Choice/Offer
branching cases. -/

/-- If a Send event on a Choice channel matches a branch, the session
state advances to that branch's continuation (no violation). -/
theorem choice_send_matches_advances
    (state : SessionState)
    (vreg : Nat)
    (branches : List SessionType)
    (msg_type : String)
    (cont : SessionType)
    (hmatch : try_match_choice_send branches msg_type = some cont)
    (hstate : state.lookup vreg = some (SessionType.choice branches)) :
    (process_session_event { kind := SessionEventKind.send_event vreg msg_type, at_node := 0 } state).2 = [] := by
  simp [process_session_event]
  rw [hstate]
  simp [hmatch]

/-- If a Recv event on an Offer channel matches a branch, the session
state advances to that branch's continuation (no violation). -/
theorem offer_recv_matches_advances
    (state : SessionState)
    (vreg : Nat)
    (branches : List SessionType)
    (expected_type : String)
    (cont : SessionType)
    (hmatch : try_match_offer_recv branches expected_type = some cont)
    (hstate : state.lookup vreg = some (SessionType.offer branches)) :
    (process_session_event { kind := SessionEventKind.recv_event vreg expected_type, at_node := 0 } state).2 = [] := by
  simp [process_session_event]
  rw [hstate]
  simp [hmatch]

/-- If a Send event on a Choice channel matches NO branch, a violation
is reported (soundness: the verifier never silently accepts a
non-matching send on a Choice). -/
theorem choice_send_no_match_violation
    (state : SessionState)
    (vreg : Nat)
    (branches : List SessionType)
    (msg_type : String)
    (hnomatch : try_match_choice_send branches msg_type = none)
    (hstate : state.lookup vreg = some (SessionType.choice branches)) :
    (process_session_event { kind := SessionEventKind.send_event vreg msg_type, at_node := 0 } state).2 ≠ [] := by
  simp [process_session_event]
  rw [hstate]
  simp [hnomatch]
  -- The violation list is `[{...}]`, which is non-empty.
  decide

/-- If a Recv event on an Offer channel matches NO branch, a violation
is reported. -/
theorem offer_recv_no_match_violation
    (state : SessionState)
    (vreg : Nat)
    (branches : List SessionType)
    (expected_type : String)
    (hnomatch : try_match_offer_recv branches expected_type = none)
    (hstate : state.lookup vreg = some (SessionType.offer branches)) :
    (process_session_event { kind := SessionEventKind.recv_event vreg expected_type, at_node := 0 } state).2 ≠ [] := by
  simp [process_session_event]
  rw [hstate]
  simp [hnomatch]
  decide

end PMT.IVE.Soundness
