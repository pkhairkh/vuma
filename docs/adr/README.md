# ADR Index

Architecture Decision Records for the VUMA-side audit. Each ADR is a
standalone Markdown file in this directory, in MADR format (Context /
Decision / Consequences / Alternatives Considered / References).

ADRs are numbered sequentially. Once an ADR's status becomes
`Accepted`, it is a binding decision; supersession requires a new ADR
that explicitly references the superseded one in its `Supersedes:`
field.

## ADRs

| Number | Title | Status | Date | Closes |
|--------|-------|--------|------|--------|
| [ADR-0001](ADR-0001.md) | Fix `bridge_type_to_ir_type` to map f32/f64 | Accepted (severity: P0→P1 by ADR-0011, then **REVERTED to P0** by Wave S-Z empirical test — causes memory corruption + IVE unsoundness) | 2026-08-01 | V-34 |
| [ADR-0002](ADR-0002.md) | Fix `type_size_from_name` + `type_alignment` for layout names | Accepted (severity revised by ADR-0011: P0→P2) | 2026-08-01 | V-35, V-42, V-44 |
| [ADR-0003](ADR-0003.md) | Thread IRType through StateRead/StateWrite + fix `Alloc { size: 0 }` | Accepted (severity revised by ADR-0011: P0→P2) | 2026-08-01 | V-36, V-A2-1 |
| [ADR-0004](ADR-0004.md) | Migrate `build_pmt_layout_specs` + IVE `rederive_layout` to `_with_layouts` | Accepted (framing revised by ADR-0011: P0→P1, IVE-soundness not codegen-correctness) | 2026-08-01 | V-03, V-NEW-2 |
| [ADR-0005](ADR-0005.md) | Delete unused build-deps + legacy `bridge_type_size` | Accepted | 2026-08-01 | V-40, deps cleanup |
| [ADR-0006](ADR-0006.md) | Defer f32 PMT Lean proof to v2; use runtime `__float_overflow_trap` only | Accepted (effort revised by ADR-0011: 3-6mo → 2-4wk bit-pattern / 2-3mo IEEE-754) | 2026-08-01 | V-14 |
| [ADR-0007](ADR-0007.md) | Wire `verify_capability` + migrate to HMAC-SHA256 | **Accepted** (promoted by ADR-0024; framing revised by ADR-0011: wire IVE verifier not emitted binaries; severity P1→P0) | 2026-08-01 | V-16, V-A3-2 |
| [ADR-0008](ADR-0008.md) | Fix `discharge_rate` denominator to include `failed` | Accepted (confirmed by ADR-0011) | 2026-08-01 | V-A3-3 |
| [ADR-0009](ADR-0009.md) | Re-run full test suite on `main` HEAD before treating V-39 as ground truth | Accepted (confirmed by ADR-0011; QEMU 10.0+ mandated) | 2026-08-01 | V-39 (stale baseline) |
| [ADR-0010](ADR-0010.md) | Adopt "5 external crates maximum" dependency policy | Accepted (confirmed by ADR-0011) | 2026-08-01 | deps policy |
| [ADR-0011](ADR-0011.md) | Re-audit corrections to ADR-0001 through ADR-0010 | Accepted | 2026-08-01 | (meta-ADR; corrects severities and framing) |
| [ADR-0012](ADR-0012.md) | Adopt "VEEE" as the name for the UX language layer | Accepted | 2026-08-01 | (renames VELL → VEEE) |
| [ADR-0013](ADR-0013.md) | Adopt the three-layer architecture (VUMA / WOMB / VEEE) | Accepted | 2026-08-01 | (formalizes layer boundaries) |
| [ADR-0014](ADR-0014.md) | VEEE compiles to VUMA AST, not to VUMA IR | Accepted | 2026-08-01 | (VEEE inherits PMT verification) |
| [ADR-0015](ADR-0015.md) | ~~VEEE backend strategy — Cranelift (dev) + VUMA codegen (prod) + MLIR→SPIR-V (GPU)~~ | **SUPERSEDED by ADR-0022 + ADR-0023** | 2026-08-01 | (violates hand-write philosophy) |
| [ADR-0016](ADR-0016.md) | VEEE's incremental computation engine lives in VEEE, not VUMA | Accepted | 2026-08-01 | (VUMA stays minimal) |
| [ADR-0017](ADR-0017.md) | VEEE's monotonicity types are a VEEE-layer type-system feature | Accepted | 2026-08-01 | (not a VUMA IR feature) |
| [ADR-0018](ADR-0018.md) | ~~GPU path for VEEE goes through MLIR→SPIR-V~~ | **SUPERSEDED by ADR-0022** (hand-written SPIR-V via glslangValidator) | 2026-08-01 | (violates hand-write philosophy) |
| [ADR-0019](ADR-0019.md) | WOMB UI modules live in `womb/ui/`; IrqRing generalizes to `womb/sync/` | Accepted | 2026-08-01 | (WOMB layer structure) |
| [ADR-0020](ADR-0020.md) | Fix broken `womb/net/*.vuma` imports (V-WOMB-1) | Accepted | 2026-08-01 | V-WOMB-1 |
| [ADR-0021](ADR-0021.md) | Delete the `Effect` enum (it is dead code) | Accepted | 2026-08-01 | V-A3-7 |
| [ADR-0022](ADR-0022.md) | Hand-written SPIR-V backend (supersedes ADR-0018's MLIR approach) | Accepted | 2026-08-01 | V-GPU, V-26 |
| [ADR-0023](ADR-0023.md) | VEEE dev builds use VUMA's codegen with `--dev` flags, not Cranelift | Accepted | 2026-08-01 | (supersedes ADR-0015's dev track) |
| [ADR-0024](ADR-0024.md) | Promote ADR-0007 from Proposed to Accepted (IVE l1l3_collapse wiring verified) | Accepted | 2026-08-01 | (promotes ADR-0007) |
| [ADR-0025](ADR-0025.md) | Extend SIMD coverage incrementally — add ops as text-shaper benchmarks demand them | Accepted | 2026-08-01 | V-13, V-A2-3 |

**Note**: ADR-0011 is a meta-ADR that documents the Wave F re-audit
corrections. The technical fixes in ADR-0001 through ADR-0010 remain
valid; what changes is severity, framing, and urgency. Read ADR-0011
alongside the original ADRs to get the current state.

**Note**: ADR-0012 through ADR-0018 are Wave L decisions covering the
VEEE rename (ADR-0012), the three-layer architecture (ADR-0013), and
VEEE's compilation strategy (ADR-0014, ADR-0016, ADR-0017). ADR-0015
and ADR-0018 are **SUPERSEDED** by ADR-0022 and ADR-0023 — they
violated VUMA's hand-write philosophy by proposing MLIR and Cranelift.
The hand-written alternatives (glslangValidator for SPIR-V, VUMA
codegen with `--dev` flags for dev builds) are in ADR-0022 and
ADR-0023.

**Note**: ADR-0019 through ADR-0021 are Wave L decisions covering the
WOMB layer (ADR-0019, ADR-0020) and the cleanup of the previously-
undecided `Effect` enum (ADR-0021, resolving V-A3-7).

**Note**: ADR-0022 through ADR-0025 are Wave O/P/Q decisions:
- ADR-0022 — hand-written SPIR-V (replaces MLIR)
- ADR-0023 — VUMA codegen with `--dev` flags (replaces Cranelift)
- ADR-0024 — promotes ADR-0007 to Accepted (IVE l1l3_collapse verified)
- ADR-0025 — V-13 SIMD incremental extension (uses existing latency tables as benchmark data)

## Dependency edges

```
ADR-0001 (V-34) ──┐
                  ├──> ADR-0004 (V-03 + V-NEW-2) ──> ADR-0003 (V-36 + V-A2-1)
ADR-0002 (V-35) ──┘                                  │
                                                     │
ADR-0002 ──> V-46 (deferred ADR)                      │
ADR-0002 ──> V-NEW-1 (deferred ADR)                   │
                                                     │
ADR-0004 ──> ADR-0005 (delete legacy fn + deps) ──> ADR-0010 (5-crate policy)
                                                     │
ADR-0009 (re-run tests) ──── no deps, parallel ──────┘
ADR-0008 (discharge_rate) ── no deps
ADR-0006 (Lean f32 defer) ── no deps
ADR-0007 (HMAC-SHA256) ──── no deps on bridge-fix, but lands after ADR-0005
```

## Statuses

- **Accepted** — decision is binding; implementation can proceed.
- **Proposed** — decision is sound but has an unresolved medium-confidence
  step that needs verification before implementation. (ADR-0007's
  per-backend call-ABI wiring step needs verification.)
- **Rejected** — not used in this set; no ADR was rejected.
- **Superseded** — not used in this set; no ADR supersedes another.
- **Deprecated** — not used in this set.

## ADRs deferred (not yet written)

The following catalog entries need ADRs but were deferred because the
research did not produce enough confidence to lock in a decision:

- **V-13** (SIMD coverage) — needs benchmarking data on which SIMD ops
  actually accelerate text shaping. ADR will be written after a
  benchmark harness exists.
- **V-11** (session types AST/IR plumbing) — IVE-side `Choice`/`Offer`
  already exists as dead code, so the IVE work is done, but the AST/IR
  enum extension + parser syntax + Lean proof update design needs more
  thought. ADR will be written after the syntax design is settled.
- **V-26** (const byte arrays) — needs syntax design (`[u8; 4]: 0x01,
  0x02, 0x03, 0x04`? `b"..."` literal? `Expr::ArrayLit` vs `Lit::Bytes`?).
  ADR will be written after the syntax is settled.
- **Unified `VumaType` refactor** — too large for an ADR; needs a
  separate RFC. Would eliminate V-34, V-35, V-42, V-44, V-46, V-03,
  V-NEW-2, V-NEW-1 in one stroke but is a 2–3 week refactor touching
  every layer.
- **Effect enum wire-vs-delete** (V-A3-7) — the `Effect` enum is
  currently dead code (IVE has zero references to it). Wiring it into
  IVE is 2–3 weeks of real work; deleting it is 1 day. Decision needs
  usage analysis (does V-02 / the GPU stack actually need it?).

## How to add a new ADR

1. Copy `ADR-0001.md` as a template.
2. Number it `ADR-NNNN` where NNNN is the next free number.
3. Fill in Context (cite file:line evidence), Decision (precise enough
   to implement), Consequences (Positive/Negative/Neutral),
   Alternatives Considered (at least 2, with rejection rationale),
   References (links to catalog entries, research reports, source).
4. Add a row to the table above.
5. If the ADR supersedes another, update the superseded ADR's
   `Superseded by:` field and change its status to `Superseded`.
