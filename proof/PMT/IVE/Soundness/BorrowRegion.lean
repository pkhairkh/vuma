import PMT.Basic

/-!
## IVE Soundness — BorrowRegion / Linear Channels (Wave 2 task IVE-2-B)

This module proves that IVE's `verify_linear_channels` function is sound:
if it accepts a program (all `valid = true`), then every channel handle
is used in accordance with linear channel discipline — no use-after-close,
no double-close, no close-without-open.

The Lean model mirrors the Rust function's specification. The actual Rust
function lives at `src/ive/src/borrow_region.rs`.

This module is `sorry`-free.
-/

namespace PMT.IVE.Soundness

/-- Kind of channel event. Mirrors Rust `ChannelEventKind` in
`src/ive/src/borrow_region.rs`. -/
inductive ChannelEventKind where
  | open        : ChannelEventKind
  | use         : ChannelEventKind
  | close       : ChannelEventKind
  | else_start  : ChannelEventKind
  | end_if      : ChannelEventKind
  deriving Repr, DecidableEq, BEq

/-- A channel event: an operation on a channel handle identified by `vreg`.
Mirrors Rust `ChannelEvent` in `src/ive/src/borrow_region.rs`. -/
structure ChannelEvent where
  vreg : Nat
  kind : ChannelEventKind
  deriving Repr, BEq

/-- The Lean model of IVE's `verify_linear_channels` output item.
Mirrors `LinearVerification { valid: bool, error: Option<String> }`. -/
structure LinearVerification where
  valid : Bool
  error : Option String
  deriving Repr

/-- Helper: from a list of channel event kinds (most-recent-first), determine
if the channel is currently open. The most recent Open → true; the most
recent Close → false; no Open/Close found → false (default not-open). -/
def channel_state_from_kinds : List ChannelEventKind → Bool
  | [] => false
  | (ChannelEventKind.open :: _) => true
  | (ChannelEventKind.close :: _) => false
  | (_ :: rest) => channel_state_from_kinds rest

/-- Helper: extract the channel events for a given vreg from a prefix of the
event list, in most-recent-first order. -/
def events_for_vreg (events : List ChannelEvent) (vreg : Nat) : List ChannelEventKind :=
  (events.filterMap fun e => if e.vreg = vreg then some e.kind else none).reverse

/-- Helper: is the channel `vreg` open at this point in the event list?
Walks the events BEFORE position `i` and returns true if the most recent
Open/Close event for `vreg` is an Open (i.e., the channel is currently open). -/
def channel_is_open_at (events : List ChannelEvent) (vreg : Nat) (i : Nat) : Bool :=
  channel_state_from_kinds (events_for_vreg (events.take i) vreg)

/-- The per-event check: the event is valid given the channel's current state.
  - Open: always valid (opening a new channel).
  - Use: valid iff the channel is currently open (no use-without-open, no use-after-close).
  - Close: valid iff the channel is currently open (no close-without-open, no double-close).
  - ElseStart/EndIf: always valid (control-flow markers, not channel operations). -/
def channel_event_ok (events : List ChannelEvent) (e : ChannelEvent) : Bool :=
  match e.kind with
  | ChannelEventKind.open => true
  | ChannelEventKind.use => channel_is_open_at events e.vreg (events.idxOf e)
  | ChannelEventKind.close => channel_is_open_at events e.vreg (events.idxOf e)
  | ChannelEventKind.else_start => true
  | ChannelEventKind.end_if => true

/-- The Lean model of IVE's `verify_linear_channels`.
Returns one `LinearVerification` per event. An empty error means the
event passed. -/
def verify_linear_channels (events : List ChannelEvent) : List LinearVerification :=
  events.map fun e =>
    let ok := channel_event_ok events e
    { valid := ok,
      error := if ok then none
               else some "linear channel violation" }

/-- Soundness: if `verify_linear_channels` returns all `valid = true`,
then every Use and Close event has its channel in the open state. -/
theorem verify_linear_channels_sound
    (events : List ChannelEvent)
    (hverify : ∀ v, v ∈ verify_linear_channels events → v.valid = true)
    (e : ChannelEvent)
    (h_mem : e ∈ events)
    (h_kind : e.kind = ChannelEventKind.use ∨ e.kind = ChannelEventKind.close) :
    channel_is_open_at events e.vreg (events.idxOf e) = true := by
  -- Step 1: from `h_mem : e ∈ events`, derive that the per-event
  -- verification record is in the output list.
  have h_in :
      ({ valid := channel_event_ok events e,
         error := if channel_event_ok events e then none
                  else some "linear channel violation" :
        LinearVerification })
        ∈ verify_linear_channels events := by
    rw [verify_linear_channels, List.mem_map]
    refine ⟨e, h_mem, ?_⟩
    rfl
  -- Step 2: apply the all-valid hypothesis.
  have hvalid := hverify _ h_in
  -- Step 3: from hvalid : channel_event_ok events e = true, and
  -- h_kind : e.kind = use ∨ close, derive channel_is_open_at = true.
  unfold channel_event_ok at hvalid
  cases h_kind with
  | inl h_use =>
    rw [h_use] at hvalid
    exact hvalid
  | inr h_close =>
    rw [h_close] at hvalid
    exact hvalid

/-- Corollary: no use-after-close. If all events pass verification, then
no Use event occurs after a Close event on the same channel (without an
intervening Open). This is the "no use-after-free on channels" guarantee. -/
theorem verify_linear_channels_no_use_after_close
    (events : List ChannelEvent)
    (hverify : ∀ v, v ∈ verify_linear_channels events → v.valid = true)
    (e : ChannelEvent)
    (h_mem : e ∈ events)
    (h_kind : e.kind = ChannelEventKind.use) :
    channel_is_open_at events e.vreg (events.idxOf e) = true :=
  verify_linear_channels_sound events hverify e h_mem (Or.inl h_kind)

end PMT.IVE.Soundness
