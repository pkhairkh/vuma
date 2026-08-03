# A-3 — IVE + Lean Proofs + Capability Model Audit

**Subagent**: A-3
**Task**: Deep audit of VUMA IVE + Lean proofs + capability model for the problem catalog
**Scope**: `src/ive/src/`, `proof/`, `src/codegen/src/capability.rs`, `src/codegen/src/effects.rs`, `src/codegen/src/proof_artifacts.rs`, `src/codegen/src/memory_safety.rs`, `womb/crypto/`
**Repo state**: `main` at `6dc97e18` (2026-08-01)
**Audit method**: Read source only; no modifications except this report and worklog.

---

## Verdicts on existing catalog claims

### V-14 — f32 PMT Lean proof is greenfield — **VERIFIED** (with one nuance)

Every claim in the catalog is confirmed by reading the Lean source.

**`proof/PMT/Basic.lean`** (read in full):

- `Arena` is `base : Nat, capacity : Nat, used : Nat` (lines 38–42). `Nat`, not `Float`/`Real`/IEEE-754 bit-pattern.
- `Field` is `name : String, offset : Nat, size : Nat, type_name : String` (lines 51–56). `Nat` offset/size; no `value`/`payload`/IEEE bits field.
- `alloc_preserves_capacity` is at **`Basic.lean:134–143`** — exactly as the catalog claims. The proof body is:
  ```lean
  theorem alloc_preserves_capacity
      (a : Arena) (l : Layout)
      (_hcap : CapacityInvariant a)
      (_hwf  : WF_Layout l)
      (hfit : l.total_size + a.used ≤ a.capacity) :
      CapacityInvariant (alloc a l) := by
    show a.used + l.total_size ≤ a.capacity
    omega
  ```
  It discharges only `used + size ≤ capacity`. There is no lemma about value contents, no `FloatArena`, no `verified_float_add`, no NaN/inf/ULP reasoning.

**`proof/PMT/PmtInstr.lean`** (1001 lines, read in full):

- `IRType` (lines 177–194) has `f32`/`f64` as **bare tag constructors** (`| f32 : IRType  | f64 : IRType`) with no payload. The docstring at lines 200–207 states: *"They are pure tag types: `PmtInstr.well_typed` does not inspect them (the arithmetic `PmtInstr` variants are all `True`-well-typed), so the … IRType argument is never consulted for arithmetic type-checking."*
- Every arithmetic `PmtInstr` variant is `True`-well-typed — `PmtInstr.lean:800–867`. Concretely lines 810–821:
  ```lean
  | .bin_op _ _ _ _ _ => True
  | .unary_op _ _ _ _ => True
  | .cast _ _ _ _ _ => True
  | .add _ _ _ _ => True
  | .sub _ _ _ _ => True
  | .mul _ _ _ _ => True
  | .div _ _ _ _ => True
  | .cmp _ _ _ _ _ => True
  | .select _ _ _ _ _ => True
  | .ct_select _ _ _ _ _ => True
  | .ct_eq _ _ _ _ => True
  | .get_address _ _ => True
  ```
- `pmt_soundness` (`proof/PMT/Soundness.lean:245–525`) covers capacity preservation (`final_used ≤ s.arena.capacity`) and trap canonicality (`code = 1 ∨ code = 134 ∨ code = 135`). No NaN/±inf/ULP/rounding/distributivity/associativity lemma exists anywhere in `proof/PMT/`.

**Nuance / doc-bug**: The docstring at `Soundness.lean:541–546` claims:
> "The following two theorems replace those tautologies with non-trivial correctness properties. Both statements are non-tautological; their proofs are admitted with `sorry` (with `-- TODO:` documentation), because they require strengthening the inductive hypothesis…"

This is **false advertising** — `pmt_soundness_correct` at line 573 actually closes via `omega` (line 836) and `trivial` (line 839), with **no `sorry` token** anywhere in the file. The docstring is stale. The theorem is real, but the prose around it lies about how it was proved. (See "Missed bugs" below.)

### V-16 — FNV-1a × 4 capability signatures — **VERIFIED** (with two critical amplifications)

**`src/codegen/src/ipc.rs::compute_signature`** (lines 996–1007):
```rust
pub fn compute_signature(token: &CapabilityToken, signing_key: &[u8]) -> [u8; 32] {
    let base = signature_input(token, signing_key);
    let mut sig = [0u8; 32];
    for i in 0..4u8 {
        let mut chunk = Vec::with_capacity(base.len() + 1);
        chunk.push(i);
        chunk.extend_from_slice(&base);
        let h = fnv1a_64(&chunk);
        sig[(i as usize) * 8..(i as usize + 1) * 8].copy_from_slice(&h.to_le_bytes());
    }
    sig
}
```

- `fnv1a_64` delegates to `vuma_scg::hash::fnv1a_64` (line 960–962) — FNV-1a 64-bit with offset basis `0xcbf29ce484222325`, prime `0x100000001b3`.
- Four passes, each prefixed with a 1-byte salt `0, 1, 2, 3`; the four 8-byte u64 outputs are concatenated into 32 bytes. Exactly as the catalog describes.
- The signing key is mixed into the input via `signature_input` (`ipc.rs:970–984`) by `buf.extend_from_slice(signing_key)` followed by the token fields — so the key does influence the digest, but via FNV-1a XOR-mixing, not HMAC's padded-XOR construction. This is **not** HMAC and is collision-forgible by anyone who can read the source.
- The module-level SECURITY NOTE (`capability.rs:31–54`) admits: *"This is NOT HMAC, NOT a MAC, and NOT resistant to a determined adversary with access to `signing_key`."*

**`womb/crypto/mac_kdf/hmac.vuma`** (192 lines, read in full) **does contain a real RFC-2104 HMAC-SHA-256 implementation** (lines 75–116: `transform hmac_sha256(key, keylen, msg, msglen, out)`), and `womb/crypto/hash/sha256_sha224.vuma` (1525 lines) has the underlying SHA-256. So the catalog's claim that "the `womb/crypto/` module already has an HMAC-SHA256 implementation that the capability layer could adopt" is technically true.

**Amplification 1 (catalog misses)**: The HMAC-SHA-256 implementation in `womb/crypto/` is itself a **VUMA source program** (`.vuma` files), not a Rust library. `compute_signature` lives in `src/codegen/src/ipc.rs` (Rust). To replace FNV-1a × 4 with HMAC-SHA-256, the codegen must either (a) re-implement HMAC-SHA-256 in Rust inside `ipc.rs` (duplicating the .vuma implementation), (b) compile the .vuma HMAC-SHA-256 program at codegen-time and link its symbols (complex, requires VUMA self-compilation), or (c) add a Rust crypto crate (`sha2` + `hmac`) as a workspace dependency (violates the small-deps policy noted in the worklog). The catalog's 5-week estimate ("3 weeks code + 2 weeks Lean proof updates") does not budget for any of these — the "plumbing" is non-trivial.

**Amplification 2 (catalog misses)**: `capability.rs:49–54` discloses an even worse gap than V-16:
> "`verify_capability` and `verify_delegation_chain` are **never called from emitted VUMA binaries**. The compiler's `channel_recv` codegen checks only `cap_count == 0` (rejecting any message that carries capability tokens, because it cannot verify them inline). A `.vuma` program that calls `channel_recv` on a frame with `cap_count > 0` receives `-4` (PERMISSION_DENIED)."

So the FNV-1a × 4 signature is **never actually verified at runtime** — even if it were HMAC-SHA-256, the receiver rejects any non-zero-cap_count message outright. The capability model is effectively a one-way write-only ledger: tokens are minted and signed, but no emitted binary ever checks a signature. V-16's "fix" (HMAC migration) would not change this — the verify path is dead code.

### V-11 — Session types lack `Choice`/`Offer` — **PARTIALLY VERIFIED** (catalog undersells the divergence)

**`src/parser/src/ast.rs:1633–1647`** (read in full):
```rust
pub enum SessionType {
    Send(Box<Type>, Box<SessionType>),
    Recv(Box<Type>, Box<SessionType>),
    End,
    Recurse,
}
```
Only 4 variants. `Recurse` is a bare marker with no body binder. No `Choice`, `Offer`, `Rec`, `Var`. Confirmed.

**`src/codegen/src/ir.rs:167–176`** (read in full):
```rust
pub enum SessionType {
    Send(IRType, Box<SessionType>),
    Recv(IRType, Box<SessionType>),
    End,
    Recurse,
}
```
Same 4 variants. Confirmed.

**BUT** — `src/ive/src/session_type.rs:38–56` (the IVE's own internal `SessionType`) is *much richer*:
```rust
pub enum SessionType {
    End,
    Send(String, Box<SessionType>),
    Recv(String, Box<SessionType>),
    Choice(Box<SessionType>, Box<SessionType>),   // ← exists in IVE!
    Offer(Box<SessionType>, Box<SessionType>),    // ← exists in IVE!
    Rec(Box<SessionType>),                        // ← exists in IVE!
    Var(u32),                                     // ← exists in IVE!
}
```

The IVE has `dual()`, `is_dual()`, `unfold()`, `substitute()` for these variants (lines 64–131). The catalog says V-11 "blocks IME channels, any protocol with branching" — but the IVE *has* the language to express branching protocols. What's missing is:
1. AST/IR parser support for emitting `Choice`/`Offer`/`Rec`/`Var` (so user programs can't express them).
2. The lowering from AST `Recurse` (a bare marker) to IVE `Rec(Box<SessionType>)` (a binder) — there's no binder body in the AST, so the IVE can't reconstruct one.

So V-11 is real but the fix is parser/AST work, not IVE work — the IVE side already supports branching.

**Lean proof divergence**: `proof/PMT/IVE/Soundness/SessionType.lean:27–31` models the IVE's `SessionType` as **only 3 variants**:
```lean
inductive SessionType where
  | end  : SessionType
  | send : String → SessionType → SessionType
  | recv : String → SessionType → SessionType
```
This **omits `Choice`, `Offer`, `Rec`, `Var`** that exist in the Rust IVE. The Lean model is behind the Rust implementation by 4 variants. The catalog's V-11 fix step 3 ("Re-prove session-type soundness lemmas in `proof/PMT/IVE/Soundness/SessionType.lean`") is more work than implied — the Lean model needs to first *catch up* to the Rust IVE before any new proof work.

### V-03 — Legacy `bridge_type_size` still used by `build_pmt_layout_specs` — **VERIFIED** (and worse than the catalog says)

**`src/pipeline.rs:6532–6550`** — `bridge_type_size` (legacy, `_ => 8` catch-all at line 6540):
```rust
fn bridge_type_size(ty: &vuma_parser::ast::Type) -> u64 {
    use vuma_parser::ast::Type;
    match ty {
        Type::BDBase(name) => match name.as_str() {
            "i8" | "u8" | "bool" => 1,
            ...
            _ => 8,                         // ← user-defined layouts land here
        },
        ...
        _ => 8,
    }
}
```

**`src/pipeline.rs:6557–6586`** — `bridge_type_size_with_layouts` (fixed, looks up `layout_sizes` map at lines 6570–6575). Confirmed.

**`src/pipeline.rs:6715–6756`** — `build_pmt_layout_specs` calls `bridge_type_size(ftype)` at **line 6724** (the LEGACY variant). The codegen-side `build_layout_registry` uses `bridge_type_size_with_layouts` at lines 6652 and 6679. Confirmed V-03.

**Worse than the catalog says**: The IVE's own `verify_layout_consistency` cross-check (`src/ive/src/verification.rs:336–365`) is structurally designed to **never catch this bug**. The IVE re-derives the layout from the field list using `rederive_layout` (line 268), which calls `type_align_size` (line 299) with the **same `_ => 8` catch-all** at line 325. The IVE's docstring at lines 264–267 admits this explicitly:

> *"anything else (user-defined layout name, etc.) → align 8, size 8 (matches the pipeline's `_ => 8` catch-all — known small-layout bug; this verifier faithfully reproduces it so that consistency checks pass on pipeline-provided layouts)."*

So the IVE "consistency check" passes by construction for any nested-layout program — both the pipeline and the IVE reproduce the same wrong layout. The catalog's V-03 fix (migrate to `_with_layouts`) would NOT be caught by the existing IVE consistency check; it would only be caught by the actual `verify_state_reads`/`verify_state_writes` field-bounds check, which uses the pipeline-provided (wrong) offsets. See newly-surfaced bug **V-A3-1** below.

---

## Lean proof layer analysis

### What's actually proved (real theorems)

- `alloc_preserves_capacity` (`Basic.lean:134–143`) — capacity preservation, `omega`-closed, sorry-free.
- `pmt_soundness` (`Soundness.lean:245–525`) — well-typed programs either succeed (with `final_used ≤ capacity`) or trap with a canonical exit code. Sorry-free, by structural induction with `omega`-closed sub-goals.
- `single_step_preserves_capacity` (`Soundness.lean:867–873`) — single-step preservation, delegates to `alloc_preserves_capacity`.
- `trap_implies_nonempty` (`Soundness.lean:851–860`) — vacuous-trap exclusion.
- `pmt_soundness_correct` (`Soundness.lean:573–839`) — determinism + `final_used ≤ initial_used + Σ layout_sizes`. **Real proof** despite the misleading docstring claiming `sorry`.
- `verify_information_flow_sound` (`proof/PMT/IVE/Soundness/InformationFlow.lean:118–134`) — real theorem by contradiction: if any event's `check_flow_kind` returns `false`, a violation appears in the output, contradicting `hverify`. This is the *only* IVE Soundness proof that proves something non-tautological.
- `Pmt.SimSound.simulation` (`proof/PMT/Faithful/SimSound.lean:129–205`) — real simulation theorem (3-instruction subset `{alloc, free, stateRead}`) by contrapositive induction: Rust-trap ⇒ Lean-trap.

### What's a tautology / vacuous (despite the name)

- **`verify_session_types_sound`** (`proof/PMT/IVE/Soundness/SessionType.lean:140–144`):
  ```lean
  theorem verify_session_types_sound
      (events : List SessionEvent)
      (hverify : verify_session_types events = []) :
      verify_session_types events = [] := by
    exact hverify
  ```
  The hypothesis IS the conclusion. Proves nothing.
- **`verify_session_types_no_send_unopened`** (`SessionType.lean:148–152`) — same shape, `exact hverify`. Same tautology.
- **`l1l3_collapse_sound`** (`proof/PMT/IVE/Soundness/L1L3Collapse.lean:150–154`):
  ```lean
  theorem l1l3_collapse_sound
      (events : List ChannelTypeEvent)
      (hverify : (l1l3_collapse events).failures = []) :
      (l1l3_collapse events).failures = [] := by
    exact hverify
  ```
  Same tautology.
- **`single_step_exists_tautology`** (`proof/PMT/Faithful/SimSound2.lean:59–216`) — honestly renamed; the statement is `∃ a' env', P ∨ ¬ P`, a classical tautology. The docstring at lines 28–32 admits: *"despite the elaborate 8-way split, the statement of `single_step_exists_tautology` is `∃ a' env', P ∨ ¬ P` — a classical tautology that holds vacuously. The split is kept as evidence of the tautology but proves nothing substantive."*
- **`simulation_full_tautology`** (`SimSound2.lean:229–259`) — same; docstring at lines 218–224 admits it is *"a tautology and should not be trusted as a simulation-soundness proof; use `Pmt.SimSound.simulation` instead."*

### `sorry` audit

`grep -rEn '(^|[^a-zA-Z_])sorry([^a-zA-Z_]|$)' proof/ --include='*.lean'` (run via `Bash`) returns **zero actual `sorry` tokens** — every match is the substring inside the comment phrases "sorry-free", "no `sorry`", "without `sorry`", "the `sorry`", etc. The CI gate `scripts/check-lean.sh` greps the `lake build` *output* (not the source) for `sorry` warnings; that audit would catch any real `sorry` token at build time. So the proof library is genuinely sorry-free as advertised.

**However** — `scripts/check-lean.sh` line 77 contains a *stale comment*:
> *"The 4 documented sorries in RawArena.lean and SimRel.lean are intentional TODOs for the simulation relation proofs. Strict mode will be enabled in CI once those proofs land."*

There are no sorries in `RawArena.lean` or `SimRel.lean` in the current tree (verified by grep). The CI gate is set to strict mode (`PROOF_CHECK_STRICT=1` in `.github/workflows/proof-verify.yml:122`), and the build passes — so the comment is dead prose from a previous era.

### `decide` / `native_decide` shortcuts

Real `decide`/`native_decide` token usage (excluding comments):

- **`proof/PMT/Faithful/Extract.lean:99, 104, 110`** — three theorems (`extract_nonempty`, `extract_has_overflow_check`, `extract_has_capacity_check`) discharged via `native_decide`. The file docstring (lines 27–32) is honest about this: *"three shallow, `native_decide`-powered sanity theorems about the extracted string (non-empty, contains the overflow check, contains the capacity check). No proof placeholders, no user-declared axioms; only Lean's standard `Lean.ofReduceBool` (which backs `native_decide`) and `propext` (which backs `Decidable` infrastructure) appear in `#print axioms`."*

  What this actually proves: the hardcoded Rust string `extract_alloc` (lines 87–95, a 7-line function) is *non-empty* and *contains the substrings `"checked_add"` and `"new_used > capacity"`*. It does **not** prove that the Rust code is semantically equivalent to the Lean `Arena.alloc`, nor that the extracted code is type-safe, nor that the overflow check is correct. It is a substring sanity check dressed up as a theorem. The CI gate does not flag `native_decide` (it only greps for `sorry`).

- **`proof/PMT/RawArena.lean:629`** — `exact absurd hdest' (by decide)` inside a proof. Legitimate kernel-reduced `decide`, not a shortcut.
- **`proof/PMT/Iris/FractionalPerm.lean:71, 75, 78`** — `by decide` to prove `0 < 1`, `0 < 2`, `0 < 4` for `Frac.full`/`half`/`quarter`. Legitimate.
- Various `decide` uses in `proof/PMT/Test/*.lean` — legitimate test-time decision procedures.

### Is the Lean proof layer actually wired into CI?

Yes, but in two parallel workflows:

1. **`.github/workflows/proof-verify.yml`** (read in full) — runs `lake build` and `scripts/check-lean.sh` in strict mode (`PROOF_CHECK_STRICT=1`). This gates the *formal spec* only. Per the README (`proof/README.md:198–201`): *"This CI job gates the formal spec only — it does not gate the compiler build, which is gated by the regular `ci.yml` build / test jobs."*

2. **`.github/workflows/lean-rust-parity.yml`** (read in full) — claims to BIND the Lean↔Rust bridge via a `needs:` chain: `lean_build → cargo_check → smoke_test / e2e_test / extraction_diff → master_harness`. The `cargo_check` job (line 135) runs `cargo check --features pmt-runtime-check` and claims (lines 23–28) that this *"activates `build.rs`, which — when `lake` is on PATH and `LEAN_HOME` is set — attempts the real `lake build` → `lean --emit-c` → `cc::Build` FFI pipeline."*

   **This claim is FALSE per `build.rs`**. The actual `build.rs` (read in full, 106 lines) says at lines 6–9: *"Lean FFI bridge removed. Z3-based contract discharge replaces it. PMT state verification uses hand-written Rust verifiers."* And at lines 35–40: *"The `pmt-runtime-check` Cargo feature is RETAINED as a no-op so existing CI commands (`cargo build --features pmt-runtime-check`) continue to work; it no longer triggers any Lean linkage here."* The `lean_ffi_linked` cfg is declared via `cargo::rustc-check-cfg` (line 66) but **NEVER emitted** (line 50).

   The `lean-rust-parity.yml` workflow is therefore testing a non-existent FFI bridge. See newly-surfaced bug **V-A3-4** below.

### Lean `Arena`/`Field` divergence from the Rust runtime arena

There are **three** Lean models of the arena, and they disagree:

| Location | Arena fields | Notes |
|---|---|---|
| `proof/PMT/Basic.lean:38–42` | `base : Nat, capacity : Nat, used : Nat` | 3 fields. Used by `pmt_soundness`. |
| `proof/PMT/Faithful/Model.lean:49–53` | `base : Ptr, capacity : USize, used : USize, alloc_id : Nat` | 4 fields. `Ptr = {addr, provenance}`. Used by `Pmt.SimSound.simulation`. |
| `src/codegen/src/runtime/arena.rs:68–82` | `base : *mut u8, offset : usize, capacity : usize, layout : Layout, created_thread : ThreadId` | 5 fields. The actual runtime. |

Divergences:

1. **`used` vs `offset`**: Lean models call the bump pointer `used`; Rust calls it `offset`. Same semantics, different name — not a soundness issue but a faithfulness gap for any Lean↔Rust agreement theorem.
2. **`alloc_id` / `provenance`**: Modeled in `Faithful/Model.lean` but NOT in `Basic.lean`. The `pmt_soundness` proof (which uses `Basic.lean`'s `Arena`) cannot reason about provenance — a use-after-free via aliasing is invisible to it. `Faithful/UafProof.lean` exists but uses the `Pmt` model, not the `PMT` model, so its theorems are not composed with `pmt_soundness`.
3. **`layout` and `created_thread`**: Not modeled anywhere in Lean. The `layout` field is needed for `dealloc` (Rust's `GlobalAlloc::dealloc` requires the original `Layout`); forgetting it would be UB. The Lean model has no dealloc reasoning. `created_thread` enforces single-thread usage — the Lean model has no thread-safety reasoning, despite `pmt_soundness` being explicitly single-threaded (per `Soundness.lean:44–55`).

So the catalog's V-14 "Lean model is purely memory-safety" understates the gap: the Lean `pmt_soundness` proof reasons about a 3-field `Arena` that does not match the 5-field runtime struct, and the provenance/thread-safety/division-by-layout concerns are out of scope by construction.

---

## Capability model analysis

### FNV-1a × 4 confirmed (see V-16 above)

The signature function at `src/codegen/src/ipc.rs:996–1007` is FNV-1a × 4 with 1-byte salts, exactly as the catalog describes. The signing key is mixed into the input via `signature_input` (line 970) but via FNV-1a's XOR-multiply folding, not HMAC's ipad/opad construction.

### Bypass paths

**No `#[trusted]` annotations, no debug-mode bypass, no `cfg(test)` skip-verify paths** in `capability.rs` or `ipc.rs::verify_capability`/`verify_delegation_chain`. Searched via:
```
grep -rn "trusted|bypass|skip_verify|always_pass|TEST_ONLY" src/codegen/src/capability.rs src/codegen/src/ipc.rs src/ive/src/
```
The `TrustLevel::Untrusted` references in `ipc.rs` (lines 1517, 1524, 1547, etc.) are about FFI/driver sandboxing (syscall filter profiles), not capability verification bypasses — they always default to `Untrusted` (the secure setting) and the public API forces it (line 4394: *"Always `TrustLevel::Untrusted` — see struct doc"*).

### Hardcoded signing key in `delegate_capability`

`src/codegen/src/capability.rs:117–118`:
```rust
let mut signing_key: Vec<u8> = b"vuma_dev_signing_key".to_vec();
signing_key.extend_from_slice(&parent_token_id.to_le_bytes());
```

The signing key for delegated capabilities is the literal ASCII string `"vuma_dev_signing_key"` (20 bytes) concatenated with the parent's token id (8 bytes). This is:
1. **Hardcoded in source** — anyone reading the repo can forge delegated capabilities for any parent token id.
2. **The same string for every delegation** — no per-process or per-domain secret.
3. **Mixed with parent_token_id via FNV-1a** — so child signatures are bound to their parent's id (the comment at lines 80–83 claims this binding prevents re-parenting), but the underlying key is public.

Additionally, `delegate_capability` hardcodes `source_pid: 1, target_pid: 2` (lines 129–130) for every delegated token — every delegated token claims to be from PID 1 to PID 2, regardless of the actual delegator/delegatee.

See newly-surfaced bug **V-A3-2** below.

### Runtime verify path is dead (see V-16 amplification 2)

`capability.rs:49–54` admits that `verify_capability` and `verify_delegation_chain` are **never called from emitted VUMA binaries**. The `channel_recv` codegen rejects any frame with `cap_count > 0` outright (returns `-4` PERMISSION_DENIED). So the entire signature machinery is write-only — tokens are minted and signed, but no emitted binary ever checks a signature.

### HMAC migration path

Three options, in increasing order of effort:

1. **Add `sha2` + `hmac` Rust crates** as workspace dependencies (smallest code change, but violates the small-deps policy per worklog).
2. **Re-implement HMAC-SHA-256 in Rust** inside `src/codegen/src/ipc.rs`, mirroring the .vuma implementation in `womb/crypto/mac_kdf/hmac.vuma` + `womb/crypto/hash/sha256_sha224.vuma`. ~600 lines of Rust, no new deps, but duplicates logic.
3. **Self-compile**: have the codegen invoke `lake build` or `vuma compile` on `hmac.vuma` at build time and link the resulting symbols. Most faithful to the "VUMA can compile itself" vision, but requires the codegen to depend on a working VUMA toolchain at build time — chicken-and-egg.

None of these is 3 weeks of work. Option 1 is ~1 week; option 2 is ~2 weeks; option 3 is multi-month. The catalog's 3-week code estimate is reasonable for option 2.

---

## Newly surfaced bugs

### V-A3-1 — IVE `verify_layout_consistency` is structurally blind to V-03

**Severity**: P0 (soundness gap — masks V-03).
**File**: `src/ive/src/verification.rs:268–327` (`rederive_layout` + `type_align_size`) and `:336–365` (`verify_layout_consistency`).
**Discovered by**: this audit.

The IVE's `verify_layout_consistency` re-derives layout offsets/sizes from the field list using `type_align_size`, which has the SAME `_ => 8` catch-all as the buggy `bridge_type_size` (line 325: `_ => (8, 8)`). The IVE docstring at lines 264–267 admits this: *"matches the pipeline's `_ => 8` catch-all — known small-layout bug; this verifier faithfully reproduces it so that consistency checks pass on pipeline-provided layouts."*

So the consistency check is structurally a no-op for any nested-layout program: the pipeline produces wrong offsets via `bridge_type_size`, the IVE reproduces the same wrong offsets via `type_align_size`, the consistency check passes, and the actual `verify_state_reads`/`verify_state_writes` field-bounds check uses the wrong offsets to "prove" safety.

**Fix**: `rederive_layout` should accept an optional `layout_sizes: &HashMap<String, u64>` table and consult it for user-defined type names, mirroring `bridge_type_size_with_layouts`. This is the same fix as V-03 but applied to the IVE side. ~1 day of work.

**Effort**: 1 day after V-03 lands.

### V-A3-2 — `delegate_capability` uses a hardcoded signing key and hardcoded PIDs

**Severity**: P0 (security — capability forgery).
**File**: `src/codegen/src/capability.rs:117–137`.
**Discovered by**: this audit.

```rust
let mut signing_key: Vec<u8> = b"vuma_dev_signing_key".to_vec();
signing_key.extend_from_slice(&parent_token_id.to_le_bytes());
...
let token = crate::ipc::capability::grant_capability(
    child_id_u128,
    1, // source_pid (delegator)         ← HARDCODED
    2, // target_pid (delegatee)         ← HARDCODED
    resource,
    perms,
    1, // delegation_depth
    0, // created_at
    3600, // ttl_seconds (1 hour)
    &signing_key,
);
```

The signing key is the literal ASCII string `"vuma_dev_signing_key"` — publicly visible in the source tree. Any reader of the repo can forge a valid delegated capability token for any parent token id by computing `FNV-1a × 4 (salt || key || parent_id || ...)` themselves.

The `source_pid: 1` and `target_pid: 2` literals mean every delegated token claims to be from PID 1 to PID 2 regardless of the actual delegator/delegatee — so the `verify_delegation_chain` PID-check (if it were ever called, which per V-16 amplification 2 it is not) would always see a fixed PID pair.

**Fix**: thread the actual signing key (a per-process secret) and the actual source/target PIDs through `delegate_capability`'s signature. The caller (codegen) must source the secret from a runtime-provided value, not a compile-time literal. ~2 days of work plus a key-management design decision.

**Effort**: 2 days + key-management design.

### V-A3-3 — `discharge_rate` computation excludes `failed` from the denominator

**Severity**: P1 (metric integrity — overstates discharge success).
**File**: `src/bin/compile_dump.rs:228–236`.
**Discovered by**: this audit.

```rust
let summary = format!(
    "passed={} failed={} unverified={} total={} discharge_rate={}%",
    result.summary.passed,
    result.summary.failed,
    result.summary.unverified,
    result.summary.total_checked,
    (100 * result.summary.passed)
        .checked_div(result.summary.passed + result.summary.unverified)
        .unwrap_or(100)
);
```

The denominator is `passed + unverified`, NOT `passed + failed + unverified` (which would equal `total_checked`). Consequences:

1. **Failed invariants are excluded from the denominator.** If a program has 5 passed, 3 failed, 2 unverified, the displayed `discharge_rate` is `500 / (5 + 2) = 71%`, not the correct `500 / 10 = 50%`. The metric overstates success in proportion to the failure count.

2. **`unwrap_or(100)` returns 100% when all invariants fail.** If a program has 0 passed, 5 failed, 0 unverified, the denominator is `0 + 0 = 0`, `checked_div(0)` returns `None`, and `unwrap_or(100)` returns `100` — so a fully-failed program reports `discharge_rate=100%`. This is mathematically nonsensical and actively misleading.

3. **The architecture doc describes the metric incorrectly.** `docs/architecture.md:76` says: *"The `discharge_rate` is the fraction of proof obligations that the IVE discharged (via Z3 or trivial-true elision) over the **total obligations** collected from the program."* The implementation does not match the spec.

4. **The wave3 audit's "100% discharge rate" claim is unfalsifiable.** `scripts/archive/audit/wave3_ive_discharge.md:32` reports "Corpus-wide weighted discharge rate (passed / (passed+failed+unverified)): **100.00%**" — but that's the audit's own computation, not the compiler's. The compiler's `compile_dump` output for those same tests would have shown `discharge_rate=100%` regardless because the test suite has zero failures. The bug doesn't trigger on the gold-standard suite, but it would trigger on any real program with violated invariants.

**Fix**: replace line 234 with `(100 * result.summary.passed).checked_div(result.summary.total_checked).unwrap_or(0)`. Use `total_checked` as the denominator (matches the spec and the displayed `total=N` field). Change `unwrap_or(100)` to `unwrap_or(0)` so an all-failed program reports `0%`, not `100%`. 1-line fix.

**Effort**: 1 day (1-line fix + test that asserts the metric matches the spec on a program with mixed pass/fail/unverified).

### V-A3-4 — `lean-rust-parity.yml` CI workflow tests a non-existent FFI bridge

**Severity**: P1 (CI integrity — false green).
**File**: `.github/workflows/lean-rust-parity.yml` (entire file, 323 lines) + `build.rs:6–51` + `tests/pmt_runtime_ffi_smoke.rs:81–121` + `proof/README.md:138–156`.
**Discovered by**: this audit.

The `lean-rust-parity.yml` workflow's `cargo_check` job (line 135) runs `cargo check --features pmt-runtime-check` and the workflow docstring (lines 23–28) claims this *"activates `build.rs`, which — when `lake` is on PATH and `LEAN_HOME` is set — attempts the real `lake build` → `lean --emit-c` → `cc::Build` FFI pipeline."*

**This is false.** The actual `build.rs` (read in full) says at lines 6–9: *"Lean FFI bridge removed."* And at lines 35–40: *"The `pmt-runtime-check` Cargo feature is RETAINED as a no-op."* The `lean_ffi_linked` cfg is **never emitted** (line 50).

Furthermore, `proof/README.md:138–156` describes an `extracted/` directory containing `lean_stub.c` and `pmt_check.rs`:
> *"This directory previously held the Rust/C side of the Lean↔Rust FFI bridge for the PMT checkers proven in `PMT/Extraction.lean`."*

**The `proof/extracted/` directory does not exist** in the current tree (verified via `ls proof/extracted/` → "No such file or directory"). The README and `build.rs:42` both reference files (`proof/extracted/lean_stub.c`, `proof/extracted/pmt_check.rs`) that are not present.

The `tests/pmt_runtime_ffi_smoke.rs` test (read in full) references `#[link(name = "lean_extraction", kind = "static")]` (line 100) and extern declarations for `lean_verified_capacity_check_prim` / `lean_verify_state_reads_prim` (lines 106–119). These symbols were supposed to be provided by `liblean_extraction.a`, which was supposed to be built by `build.rs` from either the real Lean C extraction or the stub. Neither exists. The test is gated `#[cfg(feature = "pmt-runtime-check")]` and `#[cfg_attr(not(lean_ffi_linked), ignore)]` (line 52), so it is silently ignored at runtime — but the `#[link]` directive would still be processed at link time. Whether the workflow actually passes depends on whether the test crate compiles without `liblean_extraction.a` available (it may be that the `#[link]` is lazy and the test's `#[ignore]` skips the actual symbol resolution).

Either way: the `lean-rust-parity.yml` CI workflow's claim to "BIND the Lean↔Rust bridge" is false advertising. The workflow runs Lean build, then Rust build, then a smoke test that's `#[ignore]`'d, then a parity test that compares hand-translated Rust checkers against Lean definitions. There is no actual FFI linkage being tested.

**Fix**: either (a) delete `lean-rust-parity.yml` and rely on `proof-verify.yml` for the Lean-side gate (the Lean proofs are spec-only per `proof/README.md`), or (b) update the workflow docstring to honestly describe what it tests (hand-translated Rust parity, not FFI linkage), or (c) delete the stale references to `proof/extracted/` in `build.rs` and `proof/README.md`. ~1 day to decide and clean up.

**Effort**: 1 day.

### V-A3-5 — Lean `SessionType` model is behind the Rust IVE by 4 variants

**Severity**: P2 (faithfulness gap — Lean model cannot reason about `Choice`/`Offer`/`Rec`/`Var`).
**File**: `proof/PMT/IVE/Soundness/SessionType.lean:27–31` vs `src/ive/src/session_type.rs:38–56`.
**Discovered by**: this audit.

The Lean `SessionType` inductive has 3 variants (`end`, `send`, `recv`). The Rust IVE `SessionType` has 7 variants (`End`, `Send`, `Recv`, `Choice`, `Offer`, `Rec`, `Var`). The Lean model is missing 4 variants that exist in the Rust implementation. Since the AST/IR `SessionType` (per V-11) also lacks these variants, the IVE's `Choice`/`Offer`/`Rec`/`Var` arms are dead code in practice — but if V-11 is fixed (AST/IR extended), the Lean model would still need to catch up before any soundness proof could cover branching protocols.

**Fix**: extend the Lean `SessionType` inductive with `choice`, `offer`, `rec`, `var` constructors mirroring the Rust IVE; extend `process_session_event` to handle them (the IVE's `verify_session_types` doesn't actually have event kinds for Choice/Offer — those are encoded as Send/Recv with a tag — so the Lean model only needs the type definition, not new event kinds). ~3 days.

**Effort**: 3 days.

### V-A3-6 — `l1l3_collapse` has dead "unknown capability operation" branch

**Severity**: P3 (dead code / misleading failure paths).
**File**: `src/ive/src/verification.rs:2379–2383`.
**Discovered by**: this audit.

```rust
let is_intrinsic = matches!(c.kind, ComputationKind::Intrinsic(_));
if !is_intrinsic {
    continue;
}
let known = true; // All Intrinsic variants are known capability ops
if !known {
    failures.push(format!("unknown capability operation: {}", c.kind.label()));
    continue;
}
l2_checks_folded += 1;
```

`let known = true;` is a constant. The `if !known { ... }` branch is unreachable dead code — the "unknown capability operation" failure can never fire. This means the L2 capability check is effectively "if it's tagged `Intrinsic`, fold 1 L2 check" — there's no actual verification of the capability operation beyond the parse-time `Intrinsic` tag.

The docstring at lines 2249–2256 claims the verifier *"verifies the label is one of the known capability operations (`capability_grant`, `capability_delegate`, `stark_prove`)"* — but the code does not check the label at all; it trusts the `ComputationKind::Intrinsic` tag set by the parser.

**Fix**: either delete the dead `if !known` branch (and update the docstring), or actually check `c.kind.label()` against the known set. ~2 hours.

**Effort**: 2 hours.

### V-A3-7 — `Effect::ExternCall` enum is dead code (IVE does not consume it)

**Severity**: P2 (architectural — effect analysis not used for verification).
**File**: `src/codegen/src/effects.rs` (entire file) + `src/pipeline.rs:4431–4432` + `src/ive/src/` (no references).
**Discovered by**: this audit.

The catalog's task description says "Effect enum (consumed by IVE)". **This is false.** Searched:
```
grep -rn "use.*effects|Effect::|EffectSet|analyze_program_effects|is_pure|infer_effects" src/ive/
```
→ zero matches. The IVE crate never imports or references `vuma_codegen::effects::Effect`, `EffectSet`, `infer_effects`, or `analyze_program_effects`.

The only consumer is `src/pipeline.rs:4431`:
```rust
let effects_map = effects::analyze_program_effects(&program.functions);
summary.pure_functions = effects_map.values().filter(|e| e.is_pure()).count();
```
The `effects_map` is computed, used to count pure functions for a summary, and then **discarded** — never threaded into any optimization pass or verifier. The actual optimizer passes (`DCE`, `CSE`, etc. at `pipeline.rs:4451–4467`) use `has_side_effects(instr)` (defined locally in `opt.rs`), not the `Effect` enum.

So `Effect::ExternCall` (line 40 of `effects.rs`) — which the catalog's task description specifically asks about ("verify it's marked impure") — is correctly defined as impure (`is_pure()` returns false if any effect is in the set, line 54–56), but the impurity information is never consulted by the IVE or any optimization pass.

**Fix**: either (a) delete `effects.rs` and the `run_escape_and_effects_passes` summary (the pure-function count is informational only), or (b) actually wire `effects_map` into the optimizer so that `Effect::ExternCall` functions are excluded from CSE/memoization. Option (b) is the right fix but requires touching `opt.rs`'s `has_side_effects` to consult the interprocedural effect map. ~1 week.

**Effort**: 1 day (option a) or 1 week (option b).

### V-A3-8 — `verify_information_flow_from_ir` does not propagate labels through arithmetic or branches

**Severity**: P1 (soundness — information-flow check misses indirect leaks).
**File**: `src/ive/src/information_flow.rs:532–623`.
**Discovered by**: this audit.

The IR-level wrapper `verify_information_flow_from_ir` (the one actually called by the pipeline at `src/pipeline.rs:1355`) emits `FlowEvent`s only for three IR instruction kinds:
- `IRInstr::ChannelSend` (line 552) — `FlowKind::ChannelSend`
- `IRInstr::Call` to `channel_send`/`channel_send_cap` (line 569) — `FlowKind::ChannelSend`
- `IRInstr::Store` (line 597) — `FlowKind::Assign`

It does **NOT** emit events for:
- `IRInstr::BinOp` / `Add` / `Sub` / `Mul` / `Div` — so `let k = secret_key + 1;` does not produce a `FlowKind::BinOp` event. The result vreg `k` is not tainted.
- `IRInstr::CondBranch` / `Branch` — so `if secret { public = 1 }` does not produce a `FlowKind::Branch` event. Implicit flows are not tracked.

The `label_of_vreg` function (line 497) resolves labels **by name only**: a vreg is `Secret` iff its declared name is in `secret_vars`, otherwise `Public`. There is no transitive taint propagation — `let k = secret_key + 1` produces a vreg named `k` (not `secret_key`), so `label_of_vreg(k)` returns `Public`.

Concrete attack: `#[secret] let key = 0xdeadbeef; let leaked = key + 1; channel_send(ch, leaked);` — the `ChannelSend` of `leaked` is labeled `Public → Public` because `leaked`'s name is not in `secret_vars`. The leak is invisible to the checker.

The lattice is defined correctly (`Public ⊑ Internal ⊑ Secret ⊑ TopSecret` at lines 44–53, `can_flow_to`/`join` correct), but the IR wrapper collapses it to a 2-level `{Public, Secret}` system by name lookup and never emits `BinOp`/`Branch` events. The Lean proof `verify_information_flow_sound` (`proof/PMT/IVE/Soundness/InformationFlow.lean:118–134`) is genuine but vacuously satisfies itself because no `BinOp`/`Branch` events are ever constructed by the IR wrapper.

**Fix**: emit `FlowKind::BinOp` events for every `IRInstr::BinOp`/`Add`/`Sub`/`Mul`/`Div` (computing `result_label = join(lhs_label, rhs_label)` and tracking per-vreg labels in a `HashMap<u32, SecurityLabel>` instead of resolving by name). Emit `FlowKind::Branch` events for every `IRInstr::CondBranch` (collecting branch-assigned vregs from the target block's `Store`s). ~1 week.

**Effort**: 1 week.

---

## Test coverage gaps

1. **No test exercises the `discharge_rate` computation on a program with `failed > 0`.** Every test in the gold-standard suite has `failed = 0`, so the V-A3-3 bug (`unwrap_or(100)` when `passed + unverified == 0`) never triggers. Add a test that constructs a `VerificationSummary { passed: 0, failed: 5, unverified: 0, total_checked: 5 }` and asserts `discharge_rate = 0%`, not `100%`.

2. **No test exercises `delegate_capability` with a non-default signing key.** The hardcoded `b"vuma_dev_signing_key"` literal at `capability.rs:117` is never varied in tests — the existing test at `capability.rs:233–243` only checks that the child id is non-zero and has the high bit set. Add a test that forges a delegated token using the public source-code key and asserts `verify_capability` accepts it (demonstrating the vulnerability).

3. **No test covers `verify_information_flow_from_ir` on a program with indirect flows.** The existing tests at `information_flow.rs:657–682` only test direct `Store` of a secret-named value. Add a test: `#[secret] let k = ...; let leaked = k + 1; Store(leaked, public_addr)` — assert this is flagged (it currently is not, per V-A3-8).

4. **No test covers the IVE `verify_layout_consistency` on a nested-layout program.** Per V-A3-1, the consistency check is structurally blind to the V-03 bug. Add a test that constructs a `PmtLayoutSpec` for a layout nesting another layout (e.g. `Pipe { waitq: WaitQueue, ... }`) where `WaitQueue`'s real size is 24 bytes but `bridge_type_size` returns 8 — assert the consistency check flags the mismatch (it currently does not).

5. **No test covers `l1l3_collapse` on an unknown capability operation.** Per V-A3-6, the `if !known` branch is dead. Add a test that injects a `Computation` node with `ComputationKind::Intrinsic(unknown_variant)` and asserts `l1l3_collapse` reports a failure (it currently does not — the branch is unreachable).

6. **The `lean-rust-parity.yml` workflow's `smoke_test` job is `#[ignore]`'d.** Per `tests/pmt_runtime_ffi_smoke.rs:52`, both tests are tagged `#[cfg_attr(not(lean_ffi_linked), ignore)]` and `lean_ffi_linked` is never set, so the tests are always skipped. The CI workflow runs them with `--ignored` (line 180), which forces them to run — but they would link-fail because `liblean_extraction.a` doesn't exist. Verify whether this workflow actually passes on `main` (if it does, the link is being satisfied some other way; if it doesn't, CI is red and nobody noticed).

7. **`pmt_soundness_correct` has no test that exercises its non-tautological content.** The theorem proves `final_used ≤ initial_used + Σ layout_sizes`, but no test in `proof/PMT/Test/` constructs a program where this inequality is tight (i.e., where `final_used` is strictly less than the upper bound, e.g. via `field_access` steps that don't bump `used`). The existing tests (`ValidProgram`, `UafProgram`, `OverflowProgram`, `MultiStepProgram`) all have `final_used = initial_used + Σ` exactly.

8. **No Lean test covers the `Pmt` (lowercase) model's `Arena` vs the `PMT` (uppercase) model's `Arena`.** The two Lean models disagree on `Arena` fields (3 vs 4 — see "Lean Arena/Field divergence" above). Add a theorem or test that constructs an `Arena` in both models and asserts they agree on `capacity`/`used` (they do, but only because the lowercase model's `alloc_id` is unrelated to the uppercase model's missing field).

---

## Summary of verdicts

| Claim | Catalog status | A-3 verdict | Evidence |
|---|---|---|---|
| V-14 (f32 PMT Lean proof is greenfield) | Open | **VERIFIED** | `Basic.lean:38–42, 134–143`; `PmtInstr.lean:177–194, 810–821`; `Soundness.lean:245–525` |
| V-16 (FNV-1a × 4 capability signatures) | Open | **VERIFIED** + 2 amplifications | `ipc.rs:996–1007`; `capability.rs:31–54, 49–54`; `womb/crypto/mac_kdf/hmac.vuma:75–116` |
| V-11 (Session types lack Choice/Offer) | Open | **PARTIALLY VERIFIED** (IVE has them; AST/IR/Lean don't) | `ast.rs:1633–1647`; `ir.rs:167–176`; `ive/session_type.rs:38–56`; `SessionType.lean:27–31` |
| V-03 (legacy `bridge_type_size` in `build_pmt_layout_specs`) | Open | **VERIFIED** + IVE consistency check is blind to it | `pipeline.rs:6532, 6724`; `verification.rs:268–327, 336–365` |

## Summary of newly surfaced bugs

| ID | Severity | Title | Effort |
|---|---|---|---|
| V-A3-1 | P0 | IVE `verify_layout_consistency` structurally blind to V-03 | 1 day |
| V-A3-2 | P0 | `delegate_capability` hardcodes signing key + PIDs | 2 days + design |
| V-A3-3 | P1 | `discharge_rate` excludes `failed` from denominator; `unwrap_or(100)` | 1 day |
| V-A3-4 | P1 | `lean-rust-parity.yml` CI tests non-existent FFI bridge | 1 day |
| V-A3-5 | P2 | Lean `SessionType` model behind Rust IVE by 4 variants | 3 days |
| V-A3-6 | P3 | `l1l3_collapse` has dead "unknown capability operation" branch | 2 hours |
| V-A3-7 | P2 | `Effect::ExternCall` enum is dead code (IVE doesn't consume it) | 1 day–1 week |
| V-A3-8 | P1 | `verify_information_flow_from_ir` misses indirect flows (no BinOp/Branch events) | 1 week |
