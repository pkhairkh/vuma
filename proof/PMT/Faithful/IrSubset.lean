import PMT.Faithful.Model

/-!
# `IrSubset` — IR instructions and their small-step semantics over the Arena

This file models the subset of the Rust SCG IR (`src/scg/src/ir.rs`)
consisting of eight instructions (alloc, free, stateRead, stateWrite,
stateTransform, chanNew, chanSend, chanRecv) as the Lean inductive
`IrInstr`, and gives a small-step operational semantics
`Step : IrInstr → Arena → Env → Arena → Env → Prop` over the
bump-allocator `Arena` model from `Model.lean`.

Note on `size`/`align` types. Rust's `usize` is modelled here as
`USize = Fin (2^64)` (the convention used throughout the `Pmt` layer), and
`Arena.alloc` takes `size align : USize`. The `Step.alloc_*` constructors
feed the *same* `size align` into both `Arena.alloc` and `IrInstr.alloc`,
so `IrInstr.alloc`'s `size`/`align` fields are typed `USize` (not `Nat`):
this is the only choice under which the six constructors type-check, and
it is the faithful mirror of Rust `usize`.
-/

namespace Pmt

/-! ## `IrInstr` — the eight-instruction IR subset. -/

/-- The IR subset: `alloc`, `free`, `stateRead`, `stateWrite`,
    `stateTransform`, plus the three IPC-channel instructions
    `chanNew`, `chanSend`, `chanRecv`, mirroring an extension of the
    Rust `Instr` enum. `size`/`align` are `USize` (Rust `usize`).
    `stateTransform dst src` moves the pointer held by `src` into
    `dst` *linearly*: `dst` receives the pointer, `src` is consumed
    (`none`), other variables are untouched. The three `chan*`
    instructions model IPC-channel allocation and message passing:
    `chanNew dst cap` allocates a fresh channel of capacity `cap`
    into `dst`; `chanSend chan val` sends `val` along `chan`;
    `chanRecv dst chan` receives a value from `chan` into `dst`
    (non-linearly: `chan` is *not* consumed). -/
inductive IrInstr where
  | alloc          (dst : String) (size align : USize)
  | free           (src : String)
  | stateRead      (dst src field : String)
  | stateWrite     (dst src field : String)
  | stateTransform (dst src : String)
  | chanNew        (dst : String) (cap : Nat)
  | chanSend       (chan val : String)
  | chanRecv       (dst chan : String)

/-! ## `Env` — a name → pointer environment. -/

/-- A variable environment maps each name to the pointer it currently
    holds (or `none` if unbound). -/
def Env := String → Option Ptr

/-! ## `Step` — small-step semantics.

    Each instruction has an explicit *ok* and *err* constructor. The error
    constructors are first-class (not merely the absence of the ok case):
    `alloc_err` / `free_err` / `read_err` / `write_err` / `transform_err`
    / `chanNew_err` / `chanSend_err` / `chanRecv_err` each leave both the
    arena and the environment unchanged. This is what the downstream
    simulation proof needs to case-split on the outcome of every
    instruction. The `write_ok` constructor also leaves the environment
    unchanged — a successful `stateWrite` mutates the *memory contents*
    pointed-to by `src`, but since we do not model memory contents in
    the `(Arena, Env)` state, the write is observationally a no-op on
    the state (the pointer in `dst` is unchanged; only the bytes it
    addresses change, which are outside the model). The `transform_ok`
    constructor asserts LINEAR semantics: `dst` is bound to the pointer
    previously held by `src`, `src` is consumed (`none`), and every
    other variable is left untouched.

    The three IPC-channel constructors are:
      * `chanNew_ok`  — for `cap > 0`, binds `dst` to the canonical
        fresh-channel pointer `{addr := 0, provenance := 0}` (the
        arena is unchanged; the channel object itself is outside the
        `(Arena, Env)` model). `chanNew_err` fires when `cap = 0`.
      * `chanSend_ok` — for `env chan = some p`, both arena and env
        are unchanged (a send mutates channel contents, which are
        outside the model). `chanSend_err` fires when `env chan = none`.
      * `chanRecv_ok` — for `env chan = some p`, binds `dst` to `p`
        NON-LINEARLY: `chan` is *not* consumed, so every other variable
        (including `chan`) is left untouched. `chanRecv_err` fires when
        `env chan = none`. -/

set_option linter.unusedVariables false in
/-- `Step i a env a' env'` holds when executing `i` in arena `a` under
    environment `env` yields arena `a'` and environment `env'`. -/
inductive Step : IrInstr → Arena → Env → Arena → Env → Prop where
  | alloc_ok  : ∀ a a' env p dst size align,
      Arena.alloc a size align = some (a', p) →
      Step (IrInstr.alloc dst size align) a env a'
        (fun x => if x = dst then some p else env x)
  | alloc_err : ∀ a env dst size align,
      Arena.alloc a size align = none →
      Step (IrInstr.alloc dst size align) a env a env
  | free_ok   : ∀ a env src p,
      env src = some p →
      Step (IrInstr.free src) a env a
        (fun x => if x = src then none else env x)
  | free_err  : ∀ a env src,
      env src = none →
      Step (IrInstr.free src) a env a env
  | read_ok   : ∀ a env src dst field p base off (sz : Nat),
      env src = some p →
      p.addr = base + off →
      Step (IrInstr.stateRead dst src field) a env a
        (fun x => if x = dst then some p else env x)
  | read_err  : ∀ a env src dst field,
      env src = none →
      Step (IrInstr.stateRead dst src field) a env a env
  | write_ok  : ∀ a env src dst field p,
      env src = some p →
      Step (IrInstr.stateWrite dst src field) a env a env
  | write_err : ∀ a env src dst field,
      env src = none →
      Step (IrInstr.stateWrite dst src field) a env a env
  | transform_ok : ∀ a env dst src p,
      env src = some p →
      Step (IrInstr.stateTransform dst src) a env a
        (fun x => if x = dst then some p else if x = src then none else env x)
  | transform_err : ∀ a env dst src,
      env src = none →
      Step (IrInstr.stateTransform dst src) a env a env
  | chanNew_ok  : ∀ a env dst cap,
      cap > 0 →
      Step (IrInstr.chanNew dst cap) a env a
        (fun x => if x = dst then some { addr := 0, provenance := 0 } else env x)
  | chanNew_err : ∀ a env dst cap,
      cap = 0 →
      Step (IrInstr.chanNew dst cap) a env a env
  | chanSend_ok : ∀ a env chan val p,
      env chan = some p →
      Step (IrInstr.chanSend chan val) a env a env
  | chanSend_err : ∀ a env chan val,
      env chan = none →
      Step (IrInstr.chanSend chan val) a env a env
  | chanRecv_ok : ∀ a env dst chan p,
      env chan = some p →
      Step (IrInstr.chanRecv dst chan) a env a
        (fun x => if x = dst then some p else env x)
  | chanRecv_err : ∀ a env dst chan,
      env chan = none →
      Step (IrInstr.chanRecv dst chan) a env a env

end Pmt
