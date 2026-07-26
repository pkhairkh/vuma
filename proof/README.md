# Pmt — Faithful Lean Model of the VUMA Compiler

## Modules

| Module | Description |
|--------|-------------|
| `Model.lean` | USize (Fin 2^64), Ptr (provenance), Arena (overflow-checked bump allocator), Layout (field disjointness) |
| `Agreement.lean` | Agreement theorem: Lean Arena.alloc decision matches Rust mirror |
| `IrSubset.lean` | 8-instruction IR subset (alloc, free, stateRead, stateWrite, stateTransform, chanNew, chanSend, chanRecv) with 16-constructor Step relation |
| `Simulation.lean` | sim_state relation + sim_alloc theorem (Lean↔Rust alloc agreement) |
| `Simulation2.lean` | sim_free + sim_read theorems |
| `SimWrite.lean` | sim_write theorem (stateWrite preserves env) |
| `SimTransform.lean` | sim_transform theorem (linear move semantics) |
| `SimIpc.lean` | sim_chan_new/send/recv theorems (channel operations) |
| `SimSound.lean` | Top-level simulation theorem (3-instruction induction) |
| `SimSound2.lean` | Top-level simulation theorem (8-instruction, 118 tactic lines) |
| `Sep.lean` | Separation logic from scratch: HeapModel, sep (disjoint domains), Ptsto, FracPtsto |
| `CMRA.lean` | CMRA class (core, valid, 3 laws), Excl, Auth resource algebras |
| `WP.lean` | Inductive weakest precondition (2 constructors: wp_done, wp_step) + wp_frame, wp_bind |
| `FancyUpdate.lean` | Fancy updates (P |==> Q), Inv, invariant opening/closing, cap_bnd_inv_alloc |
| `WPSafety.lean` | wp_alloc_safe (wp_step + Step.alloc_ok), wp_safety |
| `ArenaInv.lean` | [cap_bnd] invariant: CapBnd structure, cap_bnd_init, cap_bnd_alloc, cap_bnd_never_exceeds |
| `GuardInv.lean` | [guard] invariant: GuardInv structure, guard_and_cap_frame, guard_alloc_safe |
| `OverflowProof.lean` | Arena overflow safety with Fin(2^64) arithmetic (2 theorems, 15+ and 20+ tactic lines) |
| `UafProof.lean` | No-UAF with pointer aliasing: no_uaf_ptr, no_uaf_alias (handles y=x and y≠x cases) |
| `Extract.lean` | Extract Arena.alloc to Rust string, prove extract_nonempty/has_overflow_check/has_capacity_check |
| `ExtractCorrect.lean` | extraction_correct: Arena.alloc returns None iff overflow OR OOB (35 tactic lines) |

## Build

```
cd proof && lake build
```

All modules are sorry-free. `grep -rc sorry Pmt/` returns 0 for every file.
