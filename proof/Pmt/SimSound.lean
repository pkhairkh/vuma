import Init.Prelude
import Init.Data.Fin.Basic

/-!
# Simulation soundness (simplified)

This file models a tiny fragment of the Pmt pipeline: a Lean-side simulator
(`lean_exec`) and a Rust-side IR executor (`rust_exec`) that operate over
matching arenas. The headline result `simulation` relates the success /
failure behaviour of the two executors: under the hypothesis that whenever
the Lean simulator succeeds the Rust executor succeeds as well, a Rust trap
implies a Lean trap. This is the contrapositive of "Lean success implies
Rust success", i.e. the two executors agree on failure whenever they agree
on success.

The proof proceeds by induction on the Lean program, analysing each
constructor of `LeanStep` (`alloc` / `free` / `read`) in the cons case.
-/

namespace Pmt

@[reducible] def USize := Fin (2^64)

def USize.add (a b : USize) : Option USize :=
  if a.val + b.val < 2^64 then some (Fin.add a b) else none

structure Ptr where
  addr : Nat
  provenance : Nat

structure Arena where
  base     : Ptr
  capacity : USize
  used     : USize
  alloc_id : Nat

abbrev Env := String → Option Ptr

structure LeanArena where
  base     : Nat
  capacity : Nat
  used     : Nat

abbrev LeanEnv := String → Option Nat

set_option linter.unusedVariables false in
def sim_state (la : LeanArena) (ra : Arena) (le : LeanEnv) (re : Env) : Prop :=
  la.used = ra.used.val ∧ la.capacity = ra.capacity.val

-- Lean execution result
inductive LeanResult where
  | ok : LeanArena → LeanEnv → LeanResult
  | trap : Nat → LeanResult

-- A single Lean step: either alloc or free or read
inductive LeanStep where
  | alloc (x : String) (size align : Nat)
  | free (x : String)
  | read (dst src field : String)

-- Execute one Lean step
set_option linter.unusedVariables false in
def lean_step (la : LeanArena) (le : LeanEnv) : LeanStep → Option (LeanArena × LeanEnv)
  | LeanStep.alloc x size _ =>
    if la.used + size ≤ la.capacity then
      some ({ la with used := la.used + size }, le)
    else none
  | LeanStep.free x => some (la, fun z => if z = x then none else le z)
  | LeanStep.read dst src _ => some (la, le)

-- Execute a list of Lean steps
def lean_exec : List LeanStep → LeanArena → LeanEnv → LeanResult
  | [], la, le => LeanResult.ok la le
  | s :: rest, la, le =>
    match lean_step la le s with
    | none => LeanResult.trap 1
    | some (la', le') => lean_exec rest la' le'

-- Rust execution result
inductive RustResult where
  | ok : Arena → Env → RustResult
  | trap : Nat → RustResult

inductive IrInstr where
  | alloc (dst : String) (size align : Nat)
  | free (src : String)
  | stateRead (dst src field : String)

-- Execute a list of Rust IR instructions (simplified: always succeeds for free/read,
-- alloc succeeds iff used+size <= capacity).
-- The guard is written with `if h : ...` so that the `by omega` proof of the
-- fresh `Fin (2^64)` bound can use the branch hypothesis together with the
-- `Fin` bounds of `ra.used` / `ra.capacity`.
def rust_exec : List IrInstr → Arena → Env → RustResult
  | [], ra, re => RustResult.ok ra re
  | i :: rest, ra, re =>
    match i with
    | IrInstr.alloc dst size _ =>
      if h : ra.used.val + size ≤ ra.capacity.val then
        have hb : ra.used.val + size < 2^64 := by omega
        rust_exec rest { ra with used := ⟨ra.used.val + size, hb⟩ } re
      else RustResult.trap 1
    | IrInstr.free src => rust_exec rest ra (fun z => if z = src then none else re z)
    | IrInstr.stateRead dst src field => rust_exec rest ra re

set_option linter.unusedVariables false in
/-- Simulation soundness (contrapositive form).

If Lean-success implies Rust-success, then a Rust trap forces a Lean trap.
The proof analyses, by induction on the Lean program, that `lean_exec` can
only yield `ok _ _` or `trap 1`. In the `ok` case the hypothesis hands us a
Rust `ok`, contradicting the assumed Rust trap; in the `trap 1` case we are
done immediately. -/
theorem simulation (lean_prog : List LeanStep) (rust_prog : List IrInstr)
    (la : LeanArena) (ra : Arena) (le : LeanEnv) (re : Env)
    (h_sim : sim_state la ra le re) :
    -- If lean_exec succeeds, rust_exec also succeeds
    -- (we don't prove exact state correspondence, just success/failure matching)
    (∀ a e, lean_exec lean_prog la le = LeanResult.ok a e →
      ∃ a' e', rust_exec rust_prog ra re = RustResult.ok a' e') →
    rust_exec rust_prog ra re = RustResult.trap 1 →
    lean_exec lean_prog la le = LeanResult.trap 1 := by
  -- Induction on lean_prog.
  -- nil case: lean_exec [] = ok, so the Lean result is `ok`, handled by the
  --   `ok` branch below (it clashes with the assumed Rust trap).
  -- cons case: case split on the head step.
  intro h_ok_impl h_rust_trap
  -- First, by induction on the Lean program, establish that `lean_exec` can
  -- only produce `ok _ _` or `trap 1` (never a trap with a different code).
  have h_shape :
      (∃ a e, lean_exec lean_prog la le = LeanResult.ok a e) ∨
      lean_exec lean_prog la le = LeanResult.trap 1 := by
    -- The shape only depends on `lean_prog`, `la`, `le`; clear the hypotheses
    -- that mention them so the induction hypothesis stays clean.
    clear h_ok_impl h_rust_trap h_sim
    induction lean_prog generalizing la le with
    | nil =>
      exact Or.inl ⟨la, le, rfl⟩
    | cons s rest IH =>
      cases s with
      | alloc x size al =>
        by_cases h : la.used + size ≤ la.capacity
        · -- guard true: `lean_step` yields `some (_,_)`, recurse on the rest
          have heq :
              lean_exec (LeanStep.alloc x size al :: rest) la le =
                lean_exec rest { la with used := la.used + size } le := by
            simp only [lean_exec, lean_step, if_pos h]
          rw [heq]
          specialize IH { la with used := la.used + size } le
          cases IH with
          | inl h1 => exact Or.inl h1
          | inr h1 => exact Or.inr h1
        · -- guard false: `lean_step` yields `none`, so `lean_exec` traps
          have heq :
              lean_exec (LeanStep.alloc x size al :: rest) la le =
                LeanResult.trap 1 := by
            simp only [lean_exec, lean_step, if_neg h]
          rw [heq]
          exact Or.inr rfl
      | free x =>
        have heq :
            lean_exec (LeanStep.free x :: rest) la le =
              lean_exec rest la (fun z => if z = x then none else le z) := by
          simp only [lean_exec, lean_step]
        rw [heq]
        specialize IH la (fun z => if z = x then none else le z)
        cases IH with
        | inl h1 => exact Or.inl h1
        | inr h1 => exact Or.inr h1
      | read dst src f =>
        have heq :
            lean_exec (LeanStep.read dst src f :: rest) la le =
              lean_exec rest la le := by
          simp only [lean_exec, lean_step]
        rw [heq]
        specialize IH la le
        cases IH with
        | inl h1 => exact Or.inl h1
        | inr h1 => exact Or.inr h1
  -- Now use the shape to finish.
  rcases h_shape with ⟨a, e, hl⟩ | ht
  · -- Lean succeeded with `ok a e`; the hypothesis gives a Rust `ok`,
    -- contradicting the assumed Rust trap.
    exfalso
    have := h_ok_impl a e hl
    rcases this with ⟨a', e', hr⟩
    rw [hr] at h_rust_trap
    contradiction
  · -- Lean already traps with code 1.
    exact ht

end Pmt
