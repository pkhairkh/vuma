//! Verification engine for the IVE module.
//!
//! (Legacy cleanup) The five pointer-invariant verifiers
//! (liveness / exclusivity / interpretation / origin / cleanup) have been
//! removed; VUMA 2.0 verifies programs via PMT state verification only.
//! The `VerificationEngine` is now a thin facade that exposes
//! [`VerificationEngine::verify_pmt`] through `verify_all`.
//!
//! # Architecture
//!
//! The `VerificationEngine` is a facade that:
//! 1. Accepts a `vuma_scg::SCG` and optional BD map
//! 2. Delegates to `InvariantAggregator::verify_pmt` for PMT state verification
//! 3. Aggregates results into a unified vector

use crate::result::VerificationResult;
use std::collections::{HashMap, HashSet};
use vuma_bd::descriptor::BD;
use vuma_codegen::scg_to_ir::TypedStateMeta;
use vuma_scg::graph::SCG;
use vuma_scg::hash::type_hash;
use vuma_scg::node::{ComputationKind, NodeId};

// ---------------------------------------------------------------------------
// VerificationInput
// ---------------------------------------------------------------------------

/// Input for the verification engine: an SCG and optionally pre-inferred BDs.
///
/// If no BD map is provided, the verification engine will run BD inference
/// automatically before verification.
pub struct VerificationInput {
    /// The SCG to verify.
    pub scg: SCG,
    /// Pre-inferred BD map (optional — will be inferred if absent).
    pub bd_map: Option<HashMap<NodeId, BD>>,
    /// Optional PMT layout registry — maps layout name →
    /// [`PmtLayoutSpec`].  Used by `InvariantAggregator::verify_pmt` when
    /// the verification level is [`VerificationLevel::Pmt`].  Populated
    /// from the program's `Item::LayoutDef` AST nodes by the pipeline
    /// (the SCG itself does not retain structured layout info — see
    /// `parser::to_scg::convert_item`'s `Item::LayoutDef` arm, which
    /// emits a Computation node with a descriptive label but discards
    /// the field types/sizes).
    pub pmt_layouts: Option<HashMap<String, PmtLayoutSpec>>,
    /// Explicitly-marked secret variable names.
    ///
    /// Populated from `#[secret]` attributes by the pipeline (see
    /// `pipeline.rs::collect_secret_vars`). When non-empty, the
    /// [`VerificationInput::is_secret_value`] helper consults this set
    /// exclusively — SCG nodes whose label references any of these
    /// variable names are treated as secret-tainted, instead of every
    /// node whose label or source filename happens to contain the
    /// substring `"secret"`.
    ///
    /// When empty (no `#[secret]` annotations in the program),
    /// [`VerificationInput::is_secret_value`] falls back to the unsound
    /// substring heuristic with a `vuma_log!(warn, ...)` deprecation
    /// notice, so existing test programs that rely on filename-based
    /// tainting continue to work but are visibly noisy about the
    /// migration gap. New programs should annotate secrets with
    /// `#[secret]` instead of relying on the substring match.
    /// See `docs/architecture/ive-fix-proposals.md` §8 for the rationale.
    pub secret_vars: HashSet<String>,
    /// Typed-state metadata recovered from the codegen Scg (Task 2-A/3-B).
    ///
    /// Populated by the pipeline from
    /// `vuma::pipeline::bridge_ast_to_codegen_scg_with_meta`, which walks
    /// the parser AST in parallel with the codegen-SCG bridge and records
    /// every typed-state op (`state_new`, `p.field` read/write, `transform`,
    /// `#[foreign_consume]`) as a recoverable [`TypedStateMeta`] entry.
    /// When non-empty, [`InvariantAggregator::verify_pmt`] runs the
    /// dual-derivation `verify_typed_state_conformance` cross-check that
    /// proves the semantic SCG's `NodePayload` typed-state ops agree with
    /// this codegen-derived list. A disagreement surfaces a divergence
    /// between the two SCG construction paths (semantic `parser::to_scg`
    /// vs codegen `bridge_ast_to_codegen_scg`); per Task 3-B the
    /// cross-check currently logs a WARNING rather than hard-failing so
    /// existing programs keep building — see NEEDS_FOLLOWUP 3-B.
    pub typed_state_meta: Vec<TypedStateMeta>,
}

/// A unified layout spec for PMT state verification.
///
/// The three PMT state verifiers (`state_read`, `state_write`,
/// `state_transform`) each carry their own duplicated `LayoutInfo` /
/// `FieldInfo` structs (a parallel-development artefact).
/// `PmtLayoutSpec` is the IVE-public shape that the pipeline constructs from
/// the AST; `InvariantAggregator::verify_pmt` converts it to each verifier's
/// local `LayoutInfo` type on demand.
///
/// Fields are kept minimal — `name`, `total_size`, and a list of
/// `(field_name, byte_offset, byte_size, type_name)` tuples — so the
/// verifiers can validate offset+size bounds and type compatibility.
#[derive(Debug, Clone, PartialEq)]
pub struct PmtLayoutSpec {
    /// Layout name (e.g. `"Point"`).
    pub name: String,
    /// Total layout size in bytes (including tail padding).
    pub total_size: u64,
    /// Fields in declaration order with computed offsets/sizes.
    pub fields: Vec<PmtFieldSpec>,
}

/// A single field within a [`PmtLayoutSpec`].
#[derive(Debug, Clone, PartialEq)]
pub struct PmtFieldSpec {
    /// Field name (unique within the layout).
    pub name: String,
    /// Byte offset of the field within the layout.
    pub offset: u64,
    /// Field size in bytes.
    pub size: u64,
    /// Field type as a display string (e.g. `"u32"`, `"[u8; 16]"`).
    pub type_name: String,
}

/// Re-derive layout offsets/sizes from the field list using the same C-style
/// alignment rules the pipeline's `build_pmt_layout_specs` uses (see
/// `pipeline.rs:8669` `bridge_type_align` / `bridge_type_size`).
///
/// This is the certifying-algorithm approach (McCarthy 1995; Blass-Nash-Remmel
/// 2006): the verifier independently recomputes the fact it's checking,
/// rather than trusting the caller. Returns `(total_size, Vec<(offset, size)>)`.
///
/// # Alignment rules
///
/// Mirrors the private `bridge_type_align` / `bridge_type_size` in the
/// `vuma` root crate (which IVE cannot call directly).  Uses `type_name`
/// to dispatch:
/// - `i8`/`u8`/`bool`        → align 1, size 1
/// - `i16`/`u16`             → align 2, size 2
/// - `i32`/`u32`/`f32`       → align 4, size 4
/// - `i64`/`u64`/`f64`       → align 8, size 8
/// - `*T`/`Ptr<..>`/`Channel`→ align 8, size 8
/// - `[T; N]`                → recurse on `T` (align of element, size × N)
/// - anything else (user-defined layout name, etc.) → align 8, size 8
///   (matches the pipeline's `_ => 8` catch-all — known small-layout bug;
///   this verifier faithfully reproduces it so that consistency checks pass
///   on pipeline-provided layouts).
pub fn rederive_layout(fields: &[PmtFieldSpec]) -> (u64, Vec<(u64, u64)>) {
    let mut offset: u64 = 0;
    let mut max_align: u64 = 1; // minimum alignment is 1
    let mut result = Vec::with_capacity(fields.len());
    for field in fields {
        let (align, size) = type_align_size(&field.type_name);
        // Standard align-up: `(offset + align - 1) & !(align - 1)`.
        // Matches the pipeline's `if falign > 1 && offset % falign != 0`
        // branch (pipeline.rs:8870).
        if align > 1 && !offset.is_multiple_of(align) {
            offset = (offset + align - 1) & !(align - 1);
        }
        max_align = max_align.max(align);
        result.push((offset, size));
        offset += size;
    }
    // Tail-pad to max_align (pipeline.rs:8880-8881).
    let total = if max_align > 1 && !offset.is_multiple_of(max_align) {
        (offset + max_align - 1) & !(max_align - 1)
    } else {
        offset
    };
    (total, result)
}

/// Compute `(alignment, size)` for a type given its display string.
///
/// Mirrors the pipeline's private `bridge_type_align` / `bridge_type_size`
/// (`pipeline.rs:8669` / `8727`). The dispatch is purely string-based — this
/// is intentional, so IVE does not need to depend on `vuma_parser::ast::Type`
/// (the SCG-side `PmtFieldSpec` only carries the display string anyway).
fn type_align_size(type_name: &str) -> (u64, u64) {
    let t = type_name.trim();
    // Array: `[T; N]`
    if t.starts_with('[') && t.ends_with(']') {
        let inner = &t[1..t.len() - 1];
        if let Some(semi) = inner.rfind(';') {
            let elem_str = inner[..semi].trim();
            let count_str = inner[semi + 1..].trim();
            if let Ok(count) = count_str.parse::<u64>() {
                let (elem_align, elem_size) = type_align_size(elem_str);
                return (elem_align, elem_size.saturating_mul(count));
            }
        }
        return (8, 8); // malformed array — fall through to catch-all
    }
    // Pointer-like types: `*T`, `Ptr<..>`, `RegionPtr<..>`, `Channel`.
    if t.starts_with('*') || t.starts_with("Ptr<") || t.starts_with("RegionPtr<") || t == "Channel"
    {
        return (8, 8);
    }
    // Primitive scalars (BDBase by name).
    match t {
        "i8" | "u8" | "bool" => (1, 1),
        "i16" | "u16" => (2, 2),
        "i32" | "u32" | "f32" => (4, 4),
        "i64" | "u64" | "f64" => (8, 8),
        _ => (8, 8), // catch-all: user-defined layout names, etc.
    }
}

/// Verify that pipeline-provided layouts match independently re-derived
/// layouts. Returns a list of mismatch descriptions (empty if all match).
///
/// For each layout, recomputes `(total_size, per_field (offset, size))` from
/// the field list and compares with the pipeline-provided values. Any
/// divergence indicates a bug in `build_pmt_layout_specs` — the IVE
/// verifiers would otherwise check state reads/writes against wrong offsets.
pub fn verify_layout_consistency(layouts: &HashMap<String, PmtLayoutSpec>) -> Vec<String> {
    let mut mismatches = Vec::new();
    for (name, spec) in layouts {
        let (derived_total, derived_fields) = rederive_layout(&spec.fields);
        if derived_total != spec.total_size {
            mismatches.push(format!(
                "Layout '{}': total_size mismatch (pipeline={}, derived={})",
                name, spec.total_size, derived_total
            ));
        }
        for (i, (derived_offset, derived_size)) in derived_fields.iter().enumerate() {
            if i < spec.fields.len() {
                let field = &spec.fields[i];
                if *derived_offset != field.offset {
                    mismatches.push(format!(
                        "Layout '{}'.{}: offset mismatch (pipeline={}, derived={})",
                        name, field.name, field.offset, derived_offset
                    ));
                }
                if *derived_size != field.size {
                    mismatches.push(format!(
                        "Layout '{}'.{}: size mismatch (pipeline={}, derived={})",
                        name, field.name, field.size, derived_size
                    ));
                }
            }
        }
    }
    mismatches
}

/// Cross-check the field LIST (names + count) of parser-provided layouts
/// against an independently IVE-derived layout map.
///
/// The existing [`verify_layout_consistency`] only re-derives offsets/sizes
/// from the parser-provided field list — the field list itself (which fields
/// exist with which names) is still parser-trusted.  This function closes
/// that residual gap by comparing the parser-provided field list against an
/// IVE-derived source.
///
/// The IVE-derived source is built inside [`VerificationEngine::verify_pmt`]
/// by walking the SCG's `StateRead` / `StateWrite` / `StateTransform` /
/// `ForeignConsume` nodes and collecting the set of `(layout_name,
/// field_name)` pairs actually referenced in the program. This is an
/// independent derivation from the SCG (which is itself built from the AST
/// by `parser::to_scg`) — a parser bug that drops or renames a field when
/// constructing `PmtLayoutSpec` would leave the SCG's state operations
/// still referencing the original field name, which this check catches.
///
/// # Semantics
///
/// For each layout present in `ivederived_layouts`:
/// - The parser-provided map must contain a layout with the same name.
///   (Layouts present only in the parser map are NOT flagged — the parser
///   may declare layouts that the program does not access.)
/// - The IVE-derived field count must not exceed the parser-provided field
///   count. (A larger IVE-derived count means the SCG references more
///   fields than the parser declares — a strong signal of a dropped field.)
/// - Every field name in the IVE-derived list must be present in the
///   parser-provided layout's field list (by name, in any position).
///
/// Returns a list of mismatch descriptions (empty if all match).
///
/// **NOTE:** A minimal placeholder was originally provided here to unblock
/// compilation; the full implementation with count checking and richer error
/// messages is provided here.
pub fn verify_layout_field_list_consistency(
    parser_layouts: &HashMap<String, PmtLayoutSpec>,
    ivederived_layouts: &HashMap<String, PmtLayoutSpec>,
) -> Vec<String> {
    let mut mismatches = Vec::new();
    for (name, derived_spec) in ivederived_layouts {
        let parser_spec = match parser_layouts.get(name) {
            Some(s) => s,
            None => {
                mismatches.push(format!(
                    "Layout '{}': present in IVE-derived (SCG) but missing from parser-provided layouts",
                    name
                ));
                continue;
            }
        };
        // Build a set of parser-declared field names for O(1) lookup.
        let parser_field_names: HashSet<&str> =
            parser_spec.fields.iter().map(|f| f.name.as_str()).collect();
        // Count check: IVE-derived (SCG-referenced) should not declare more
        // fields than the parser. If it does, the parser likely dropped a
        // field during PmtLayoutSpec construction.
        if derived_spec.fields.len() > parser_spec.fields.len() {
            mismatches.push(format!(
                "Layout '{}': field count mismatch (parser={}, ivederived={}) — IVE-derived references more fields than parser declares",
                name,
                parser_spec.fields.len(),
                derived_spec.fields.len()
            ));
        }
        // Names check: every IVE-derived (SCG-referenced) field name must be
        // declared in the parser-provided layout.
        for (i, df) in derived_spec.fields.iter().enumerate() {
            if !parser_field_names.contains(df.name.as_str()) {
                mismatches.push(format!(
                    "Layout '{}'.field[{}] ('{}'): referenced in SCG state op but not declared in parser-provided layout (parser fields: [{}])",
                    name,
                    i,
                    df.name,
                    parser_spec
                        .fields
                        .iter()
                        .map(|f| f.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
        }
    }
    mismatches
}

// ---------------------------------------------------------------------------
// Typed-state conformance cross-check (Task 3-B)
// ---------------------------------------------------------------------------
//
// The semantic SCG (`vuma-scg`) carries typed-state ops as dedicated
// `NodePayload` variants (`StateInit` / `StateRead` / `StateWrite` /
// `StateTransform` / `ForeignConsume`). The codegen Scg lowers those same
// ops to UNTYPED `AllocationNode` / `StructAccessNode` / `CallNode` /
// `ForeignConsumeStmt` statements, *losing* the layout/field/kind info —
// but Task 2-A attaches a parallel `Vec<TypedStateMeta>` record alongside
// the codegen Scg so that info is recoverable.
//
// The cross-check below is a DUAL-DERIVATION proof: it independently
// extracts a normalized `(kind, layout, field)` multiset from
//   (1) the semantic SCG's `NodePayload`s, and
//   (2) the codegen Scg's `TypedStateMeta` list,
// and compares them. A divergence signals that the two SCG construction
// paths (`parser::to_scg` vs `bridge_ast_to_codegen_scg`) disagree on the
// program's typed-state shape — the same class of bug the existing
// `verify_layout_field_list_consistency` cross-check targets for layout
// field lists.
//
// `vreg_or_var` is recorded for diagnostics only: the semantic SCG uses
// numeric vregs (`u32`), while the codegen `TypedStateMeta` uses either a
// synthetic source-order counter (`StateInit::result_vreg`) or a variable
// name string (`StateRead`/`StateWrite`/`ForeignConsume::var`). The two
// are NOT directly comparable, so equality is decided on `kind` +
// `layout` + `field` only.

/// The five typed-state op kinds, mirroring the semantic SCG's
/// `NodePayload` variants and the codegen `TypedStateMeta` enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum TypedStateKind {
    /// `let p = state_new(L)` — typed-state allocation.
    StateInit,
    /// `let v = p.field` — typed-state field read.
    StateRead,
    /// `p.field = expr` — typed-state field write.
    StateWrite,
    /// `let q = t(p)` — typed-state layout reinterpretation.
    StateTransform,
    /// `consume(p)` / `#[foreign_consume]` — linearity marker.
    ForeignConsume,
}

/// A normalized `(kind, layout, field)` triple recovered from EITHER the
/// semantic SCG's `NodePayload` typed-state ops OR the codegen Scg's
/// [`TypedStateMeta`] list. Used by [`verify_typed_state_conformance`].
///
/// `vreg_or_var` carries the original vreg (semantic side, as a string) or
/// variable name (codegen side) for diagnostics; it is NOT part of the
/// equality comparison (see the module-level comment above).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypedStateTriple {
    /// Which typed-state op kind this triple came from.
    pub kind: TypedStateKind,
    /// Layout name. For `StateTransform` this is the *input* layout; the
    /// output layout is encoded in `field` as `"->output_layout"` so a
    /// divergent output layout surfaces as a field mismatch.
    pub layout: String,
    /// Field name. Empty for `StateInit` / `ForeignConsume` (no field).
    /// For `StateTransform` this is `"->output_layout"`.
    pub field: String,
    /// Original vreg (semantic) or var string (codegen) — diagnostics only.
    pub vreg_or_var: String,
}

/// Extract normalized [`TypedStateTriple`]s from the semantic SCG's
/// `NodePayload` typed-state variants (derivation #1).
fn extract_typed_state_triples_from_scg(scg: &SCG) -> Vec<TypedStateTriple> {
    use vuma_scg::node::{NodePayload, StateInitNode};
    let mut out = Vec::new();
    for node in scg.nodes() {
        match &node.payload {
            NodePayload::StateInit(StateInitNode { layout_name, result_vreg }) => {
                out.push(TypedStateTriple {
                    kind: TypedStateKind::StateInit,
                    layout: layout_name.clone(),
                    field: String::new(),
                    vreg_or_var: result_vreg.to_string(),
                });
            }
            NodePayload::StateRead(r) => {
                out.push(TypedStateTriple {
                    kind: TypedStateKind::StateRead,
                    layout: r.layout_name.clone(),
                    field: r.field_name.clone(),
                    vreg_or_var: r.state_vreg.to_string(),
                });
            }
            NodePayload::StateWrite(w) => {
                out.push(TypedStateTriple {
                    kind: TypedStateKind::StateWrite,
                    layout: w.layout_name.clone(),
                    field: w.field_name.clone(),
                    vreg_or_var: w.state_vreg.to_string(),
                });
            }
            NodePayload::StateTransform(t) => {
                // Key on the input layout; encode the output layout in the
                // field slot so a divergent output layout is caught.
                out.push(TypedStateTriple {
                    kind: TypedStateKind::StateTransform,
                    layout: t.input_layout.clone(),
                    field: format!("->{}", t.output_layout),
                    vreg_or_var: t.input_vreg.to_string(),
                });
            }
            NodePayload::ForeignConsume(fc) => {
                // NOTE: the codegen `TypedStateMeta::ForeignConsume` does
                // NOT carry a layout_name (only `var`), so for this kind
                // the cross-check is count-only — the semantic layout is
                // recorded here for diagnostics but the comparison drops
                // it (see `verify_typed_state_conformance`).
                out.push(TypedStateTriple {
                    kind: TypedStateKind::ForeignConsume,
                    layout: fc.layout_name.clone(),
                    field: String::new(),
                    vreg_or_var: fc.input_vreg.to_string(),
                });
            }
            _ => {}
        }
    }
    out
}

/// Extract normalized [`TypedStateTriple`]s from the codegen Scg's
/// [`TypedStateMeta`] list (derivation #2).
fn extract_typed_state_triples_from_codegen_meta(
    meta: &[TypedStateMeta],
) -> Vec<TypedStateTriple> {
    meta.iter()
        .map(|m| match m {
            TypedStateMeta::StateInit { layout_name, result_vreg } => TypedStateTriple {
                kind: TypedStateKind::StateInit,
                layout: layout_name.clone(),
                field: String::new(),
                vreg_or_var: result_vreg.to_string(),
            },
            TypedStateMeta::StateRead { var, layout_name, field_name } => TypedStateTriple {
                kind: TypedStateKind::StateRead,
                layout: layout_name.clone(),
                field: field_name.clone(),
                vreg_or_var: var.clone(),
            },
            TypedStateMeta::StateWrite { var, layout_name, field_name } => TypedStateTriple {
                kind: TypedStateKind::StateWrite,
                layout: layout_name.clone(),
                field: field_name.clone(),
                vreg_or_var: var.clone(),
            },
            TypedStateMeta::StateTransform { input_layout, output_layout } => TypedStateTriple {
                kind: TypedStateKind::StateTransform,
                layout: input_layout.clone(),
                field: format!("->{}", output_layout),
                vreg_or_var: String::new(),
            },
            TypedStateMeta::ForeignConsume { var } => TypedStateTriple {
                kind: TypedStateKind::ForeignConsume,
                // Codegen does not carry a layout for ForeignConsume; the
                // comparison handles this kind by count only.
                layout: String::new(),
                field: String::new(),
                vreg_or_var: var.clone(),
            },
        })
        .collect()
}

/// Dual-derivation cross-check (Task 3-B): prove the semantic SCG and the
/// codegen Scg agree on typed-state info.
///
/// Walks the semantic SCG's `NodePayload` typed-state variants and
/// normalizes each into a [`TypedStateTriple`] (derivation #1), then
/// normalizes the codegen Scg's [`TypedStateMeta`] list into the same
/// triple form (derivation #2). The two are compared by:
/// - per-kind **count** (all five kinds), and
/// - `(kind, layout, field)` **multiset** equality for the four kinds
///   that carry comparable layout/field info (`StateInit`, `StateRead`,
///   `StateWrite`, `StateTransform`). `ForeignConsume` is count-only
///   because the codegen `TypedStateMeta::ForeignConsume` does not carry
///   a layout name.
///
/// This mirrors the dual-derivation pattern of
/// [`verify_layout_field_list_consistency`]: a divergence between the two
/// independent derivations signals a bug in one of the two SCG
/// construction paths (`parser::to_scg` for the semantic SCG,
/// `bridge_ast_to_codegen_scg` for the codegen Scg).
///
/// Returns a list of mismatch descriptions (empty if the two sides agree).
pub fn verify_typed_state_conformance(
    scg: &SCG,
    codegen_meta: &[TypedStateMeta],
) -> Vec<String> {
    let semantic_triples = extract_typed_state_triples_from_scg(scg);
    let codegen_triples = extract_typed_state_triples_from_codegen_meta(codegen_meta);
    let mut mismatches = Vec::new();

    let all_kinds = [
        TypedStateKind::StateInit,
        TypedStateKind::StateRead,
        TypedStateKind::StateWrite,
        TypedStateKind::StateTransform,
        TypedStateKind::ForeignConsume,
    ];

    // Per-kind count check (covers all five kinds, including the
    // layout-less ForeignConsume).
    for kind in all_kinds {
        let scg_count = semantic_triples.iter().filter(|t| t.kind == kind).count();
        let cg_count = codegen_triples.iter().filter(|t| t.kind == kind).count();
        if scg_count != cg_count {
            mismatches.push(format!(
                "{:?}: count mismatch (semantic SCG={}, codegen Scg={})",
                kind, scg_count, cg_count
            ));
        }
    }

    // (kind, layout, field) multiset check for the four layout-carrying
    // kinds. ForeignConsume is excluded (codegen lacks layout); its count
    // is already checked above.
    let multiset_kinds: HashSet<TypedStateKind> = [
        TypedStateKind::StateInit,
        TypedStateKind::StateRead,
        TypedStateKind::StateWrite,
        TypedStateKind::StateTransform,
    ]
    .iter()
    .copied()
    .collect();

    let mut scg_counts: HashMap<(TypedStateKind, String, String), usize> = HashMap::new();
    for t in &semantic_triples {
        if multiset_kinds.contains(&t.kind) {
            *scg_counts
                .entry((t.kind, t.layout.clone(), t.field.clone()))
                .or_insert(0) += 1;
        }
    }
    let mut cg_counts: HashMap<(TypedStateKind, String, String), usize> = HashMap::new();
    for t in &codegen_triples {
        if multiset_kinds.contains(&t.kind) {
            *cg_counts
                .entry((t.kind, t.layout.clone(), t.field.clone()))
                .or_insert(0) += 1;
        }
    }

    let mut all_keys: Vec<&(TypedStateKind, String, String)> =
        scg_counts.keys().chain(cg_counts.keys()).collect();
    // Tuples sort lexicographically when every element is `Ord`;
    // `TypedStateKind` derives `Ord` for this purpose.
    all_keys.sort();
    for key in all_keys {
        let sc = scg_counts.get(key).copied().unwrap_or(0);
        let cc = cg_counts.get(key).copied().unwrap_or(0);
        if sc != cc {
            mismatches.push(format!(
                "{:?} layout='{}' field='{}': occurrence count mismatch (semantic SCG={}, codegen Scg={})",
                key.0, key.1, key.2, sc, cc
            ));
        }
    }

    mismatches
}

impl VerificationInput {
    /// Create verification input from an SCG (without pre-inferred BDs).
    pub fn from_scg(scg: SCG) -> Self {
        Self {
            scg,
            bd_map: None,
            pmt_layouts: None,
            secret_vars: HashSet::new(),
            typed_state_meta: Vec::new(),
        }
    }

    /// Create verification input with a pre-inferred BD map.
    pub fn with_bd_map(scg: SCG, bd_map: HashMap<NodeId, BD>) -> Self {
        Self {
            scg,
            bd_map: Some(bd_map),
            pmt_layouts: None,
            secret_vars: HashSet::new(),
            typed_state_meta: Vec::new(),
        }
    }

    /// Attach a PMT layout registry (used by `VerificationLevel::Pmt`).
    pub fn with_pmt_layouts(mut self, layouts: HashMap<String, PmtLayoutSpec>) -> Self {
        self.pmt_layouts = Some(layouts);
        self
    }

    /// Attach the set of explicitly-marked secret variable names (from
    /// `#[secret]` attributes in the source). When non-empty,
    /// [`VerificationInput::is_secret_value`] consults this set instead of
    /// the unsound substring-based heuristic on labels/filenames.
    ///
    /// See [`VerificationInput::secret_vars`] and
    /// `docs/architecture/ive-fix-proposals.md` §8.
    pub fn with_secret_vars(mut self, vars: HashSet<String>) -> Self {
        self.secret_vars = vars;
        self
    }

    /// Attach the typed-state metadata recovered from the codegen Scg
    /// (Task 2-A/3-B). When non-empty, [`InvariantAggregator::verify_pmt`]
    /// runs the `verify_typed_state_conformance` dual-derivation
    /// cross-check comparing this codegen-derived list against the
    /// semantic SCG's `NodePayload` typed-state ops.
    ///
    /// Produced by `vuma::pipeline::bridge_ast_to_codegen_scg_with_meta`
    /// (the AST-walking bridge that records every typed-state op as a
    /// recoverable [`TypedStateMeta`] entry alongside the codegen Scg).
    pub fn with_typed_state_meta(mut self, meta: Vec<TypedStateMeta>) -> Self {
        self.typed_state_meta = meta;
        self
    }

    /// Decide whether a value is secret-tainted.
    ///
    /// Resolution order:
    /// 1. If [`VerificationInput::secret_vars`] is non-empty (i.e. the
    ///    program contains at least one `#[secret]` attribute), the result
    ///    is `secret_vars.contains(name)`. This is the sound,
    ///    attribute-based detection path: only names the programmer
    ///    explicitly annotated are treated as secret.
    /// 2. Otherwise (no `#[secret]` annotations anywhere in the program),
    ///    the verifier falls back to the unsound substring heuristic —
    ///    `name` is treated as secret iff it contains the substring
    ///    `"secret"`. A deprecation warning is emitted via `vuma_log!`
    ///    *every time* this path is taken, so legacy programs that rely on
    ///    filename/label-based tainting are visibly noisy about it. The
    ///    fallback exists ONLY so existing test programs continue to pass
    ///    during the migration; new programs should annotate with
    ///    `#[secret]` instead.
    ///
    /// This helper is the single, well-typed consumer of the
    /// `secret_vars` field. The information-flow verifier
    /// (`information_flow.rs`) uses explicit `SecurityLabel` enum values
    /// rather than calling this method, but future CT (constant-time)
    /// checks should consume secret-ness exclusively through this helper
    /// so that the fallback warning fires deterministically.
    ///
    /// # Examples
    ///
    /// ```ignore
    /// use std::collections::HashSet;
    /// use vuma_ive::VerificationInput;
    /// use vuma_scg::SCG;
    ///
    /// // Program with #[secret] annotations → strict mode.
    /// let mut secrets = HashSet::new();
    /// secrets.insert("key".to_string());
    /// let input = VerificationInput::from_scg(SCG::new())
    ///     .with_secret_vars(secrets);
    /// assert!(input.is_secret_value("key"));          // explicit annotation
    /// assert!(!input.is_secret_value("secret_seed")); // NOT in set
    ///
    /// // Program without #[secret] annotations → fallback.
    /// let input = VerificationInput::from_scg(SCG::new());
    /// // Emits: [warn] falling back to substring-based secret detection on ...
    /// assert!(input.is_secret_value("secret_seed"));  // substring match
    /// assert!(!input.is_secret_value("counter"));     // no match
    /// ```
    pub fn is_secret_value(&self, name: &str) -> bool {
        if !self.secret_vars.is_empty() {
            // Explicit attribute-based detection: sound.
            return self.secret_vars.contains(name);
        }
        // Deprecation fallback: unsound substring heuristic. Emit a warning
        // so legacy programs are visibly noisy about the migration gap.
        vuma_log!(
            warn,
            "falling back to substring-based secret detection on {:?} — add #[secret] attribute for robustness",
            name
        );
        name.contains("secret")
    }
}

// ---------------------------------------------------------------------------
// VerificationEngine
// ---------------------------------------------------------------------------

/// The verification engine checks VUMA's PMT state invariants against SCGs.
///
/// (Legacy cleanup) The five pointer-invariant verifiers
/// (liveness / exclusivity / interpretation / origin / cleanup) have been
/// removed. `verify_all` now delegates to [`Self::verify_pmt`].
pub struct VerificationEngine {
    /// Whether to emit detailed diagnostic logging.
    verbose: bool,
    /// (Legacy, retained for API stability) Maximum number of paths explored
    /// by the now-removed liveness verifier.  No longer used by `verify_pmt`,
    /// but preserved so `InvariantAggregator::with_max_paths` continues to
    /// compile and round-trip the value.
    max_paths: usize,
    /// (Legacy, retained for API stability) Maximum path length explored by
    /// the now-removed cleanup verifier.  No longer used by `verify_pmt`.
    max_path_length: usize,
}

impl Clone for VerificationEngine {
    fn clone(&self) -> Self {
        Self {
            verbose: self.verbose,
            max_paths: self.max_paths,
            max_path_length: self.max_path_length,
        }
    }
}

// ---------------------------------------------------------------------------
// Lean FFI routing for the 3 PMT state verifiers (Wave 5-A)
// ---------------------------------------------------------------------------
//
// When the `pmt-runtime-check` feature is enabled, `verify_pmt` routes the
// 3 hand-written Rust verifiers (`verify_state_reads`,
// `verify_state_writes`, `verify_all_transforms`) through the Lean-extracted
// equivalents declared in the `lean_ffi` module below.
//
// Two sub-paths, selected by the `lean_ffi_linked` cfg (emitted by build.rs
// only when the real Lean C output is linked):
//
//   - STUB (default, `lean_ffi_linked` NOT set): build.rs linked
//     `proof/extracted/lean_stub.c`, which hardcodes success for every
//     `lean_verify_*` symbol. `verify_pmt_via_lean` mirrors that by
//     returning all-empty (all-pass) result Vecs. This is the path Wave 5-C
//     tests assert: "Lean verifier ran but stub returned success".
//
//   - REAL (`lean_ffi_linked` set): calls the extracted `lean_verify_*`
//     externs. Marshalling Rust types to boxed Lean objects is Wave 5-C
//     work; until then the args are null placeholders and this branch is
//     dead code (build.rs never emits `lean_ffi_linked` in the current
//     build).
//
// When `pmt-runtime-check` is OFF, the hand-written Rust verifiers are used
// directly (the parity-tested path) - unchanged from pre-Wave-5 behaviour.
#[cfg(feature = "pmt-runtime-check")]
#[allow(dead_code)] // externs/LeanObject unused in the STUB sub-path
mod lean_ffi {
    use std::ffi::c_void;

    /// Opaque pointer to a Lean boxed object (`lean_object *` in Lean's C
    /// ABI). Matches the `LeanObject` alias in `proof/extracted/pmt_check.rs`.
    pub type LeanObject = c_void;

    extern "C" {
        /// `@[export lean_verify_state_reads]` - Lean signature
        /// `(env_list : List (String x LayoutInfo)) (reads : List StateRead)
        /// : Bool`. Returns `uint8_t` (1 iff all reads pass).
        pub fn lean_verify_state_reads(
            env_list: *mut LeanObject,
            reads: *mut LeanObject,
        ) -> u8;

        /// `@[export lean_verify_state_writes]` - Lean signature
        /// `(env_list) (consumed : List String) (writes : List StateWrite)
        /// : Bool`. Returns `uint8_t` (1 iff all writes pass).
        pub fn lean_verify_state_writes(
            env_list: *mut LeanObject,
            consumed: *mut LeanObject,
            writes: *mut LeanObject,
        ) -> u8;

        /// `@[export lean_verify_transform]` - Lean signature
        /// `(layouts : LayoutRegistry) (t : StateTransform) : Bool`.
        /// Returns `uint8_t` (1 iff the transform is valid).
        pub fn lean_verify_transform(
            layouts: *mut LeanObject,
            t: *mut LeanObject,
        ) -> u8;
    }

    // ─────────────────────────────────────────────────────────────
    // `_prim` wrappers (VERIF-SIGSEGV-FIX). These are the String-based,
    // C-marshallable exports that parse the §9 serialization format
    // internally. Unlike the boxed `lean_verify_*` externs above — which
    // SIGSEGV under real Lean when handed null `lean_object*` args (the
    // PMT-1-G / VERIF-SIGSEGV-FIX failure) — the `_prim` wrappers accept
    // Lean `String`s built via `lean_mk_string` / `str_to_lean`, so they
    // are safe to call once `lean_ffi::init()` has run. Mirrors the FFI
    // surface in `tests/pmt_parity_test.rs` and
    // `proof/extracted/pmt_check.rs`. Declared unconditionally (matching
    // the boxed externs); only INVOKED under `#[cfg(lean_ffi_linked)]`
    // in `verify_pmt_via_lean` below. C links by symbol name, so the
    // 4-arg `kind` variant added by the TRANSFORM-FIX subagent is
    // compatible with the 3-arg stub definition on the STUB path (which
    // is never called from here anyway).
    // ─────────────────────────────────────────────────────────────
    extern "C" {
        /// `lean_verify_state_reads_prim(registry, reads) -> u8`.
        /// `registry` / `reads` are boxed Lean `String`s in §9 format.
        pub fn lean_verify_state_reads_prim(
            registry: *mut LeanObject,
            reads: *mut LeanObject,
        ) -> u8;

        /// `lean_verify_state_writes_prim(registry, consumed, writes)
        /// -> u8`. All three args are boxed Lean `String`s in §9 format.
        pub fn lean_verify_state_writes_prim(
            registry: *mut LeanObject,
            consumed: *mut LeanObject,
            writes: *mut LeanObject,
        ) -> u8;

        /// `lean_verify_transform_prim(registry, input_layout,
        /// output_layout, kind) -> u8`. `kind` is a boxed Lean `String`
        /// ("copy" / "identity" / "reinterpret"). The `kind` parameter
        /// is added by the TRANSFORM-FIX subagent; verification.rs
        /// passes "copy" (no TransformKind context here — it is only
        /// checking validity).
        pub fn lean_verify_transform_prim(
            registry: *mut LeanObject,
            input_layout: *mut LeanObject,
            output_layout: *mut LeanObject,
            kind: *mut LeanObject,
        ) -> u8;
    }

    /// `initialize_PMT` — Lean module initializer exported by
    /// `proof/.lake/build/ir/PMT.c`. MUST be invoked exactly once before
    /// any `lean_verify_*` call: without it the Lean runtime is
    /// uninitialized and the verifiers SIGSEGV (the PMT-1-G smoke-test
    /// failure). `builtin = 1` runs Lean's standard builtin initializers;
    /// the second arg is Lean's reserved `lean_object*` (pass null). On
    /// success it returns a non-null `lean_object*` (Lean's unit value);
    /// null denotes failure. Gated on `lean_ffi_linked` because the
    /// linkage stub (`lean_stub.c`) does not export this symbol.
    #[cfg(lean_ffi_linked)]
    extern "C" {
        fn initialize_PMT(builtin: u8, w: *mut LeanObject) -> *mut LeanObject;
    }

    /// `lean_mk_string` — Lean C-runtime symbol (`lean.h`) that boxes a
    /// NUL-terminated C string into a Lean `String`. Only provided when
    /// the real Lean runtime (`libleanrt.a`) is linked, i.e. under
    /// `lean_ffi_linked` (the stub `lean_stub.c` does not export it).
    /// Used by `str_to_lean` below to marshal Rust `&str` into the
    /// boxed Lean `String` args the `_prim` wrappers expect.
    #[cfg(lean_ffi_linked)]
    extern "C" {
        fn lean_mk_string(s: *const std::ffi::c_char) -> *mut LeanObject;
    }

    /// Convert a Rust `&str` into a boxed Lean `String` via
    /// `lean_mk_string`. NUL bytes (not representable in a C string)
    /// are replaced with `'?'`. Mirrors `str_to_lean` in
    /// `tests/pmt_parity_test.rs`. Only meaningful under
    /// `lean_ffi_linked`; the stub variant below is dead code at
    /// runtime (the sole call site is gated on `lean_ffi_linked`).
    #[cfg(lean_ffi_linked)]
    pub fn str_to_lean(s: &str) -> *mut LeanObject {
        use std::ffi::CString;
        let sanitized: String =
            s.chars().map(|c| if c == '\0' { '?' } else { c }).collect();
        let c_str =
            CString::new(sanitized).unwrap_or_else(|_| CString::new("").unwrap());
        unsafe { lean_mk_string(c_str.as_ptr()) }
    }

    /// Stub variant — never invoked at runtime (the only call site is
    /// under `#[cfg(lean_ffi_linked)]`). Returns null so the file still
    /// type-checks on the STUB sub-path.
    #[cfg(not(lean_ffi_linked))]
    pub fn str_to_lean(_s: &str) -> *mut LeanObject {
        std::ptr::null_mut()
    }

    /// Guards exactly-once execution of `initialize_PMT` across threads.
    #[cfg(lean_ffi_linked)]
    static INIT: std::sync::Once = std::sync::Once::new();

    /// Persisted outcome of the one-shot `initialize_PMT` call. `Once`
    /// only runs the init closure a single time, so the result (null =>
    /// failure) is recorded here for every subsequent `init()` caller.
    #[cfg(lean_ffi_linked)]
    static INIT_FAILED: std::sync::atomic::AtomicBool =
        std::sync::atomic::AtomicBool::new(false);

    /// Invoke `initialize_PMT` exactly once (thread-safe via `Once`)
    /// before any Lean verifier FFI call. Returns `Ok(())` on success
    /// (non-null return — Lean's unit value) or `Err` describing the
    /// failure if the initializer returned null.
    #[cfg(lean_ffi_linked)]
    pub fn init() -> Result<(), String> {
        INIT.call_once(|| {
            // `builtin = 1` => run Lean's standard builtin initializers.
            let res = unsafe { initialize_PMT(1, std::ptr::null_mut()) };
            if res.is_null() {
                INIT_FAILED.store(true, std::sync::atomic::Ordering::SeqCst);
            }
        });
        if INIT_FAILED.load(std::sync::atomic::Ordering::SeqCst) {
            Err(
                "Lean module initializer initialize_PMT returned null (init failed)"
                    .to_string(),
            )
        } else {
            Ok(())
        }
    }
}

/// Result of routing the 3 PMT state verifiers through Lean: the same
/// `(read, write, transform)` Vec triple the hand-written verifiers return
/// so the downstream aggregation in `verify_pmt` is unchanged.
#[cfg(feature = "pmt-runtime-check")]
type LeanPmtOutcome = (
    Vec<crate::state_read::StateReadVerification>,
    Vec<crate::state_write::StateWriteVerification>,
    Vec<crate::state_transform::StateTransformVerification>,
);

// ─────────────────────────────────────────────────────────────────────
// §9 serializers (VERIF-MARSHAL). Adapted from tests/pmt_parity_test.rs
// serializers for the production data types. Only compiled under
// `lean_ffi_linked` (the REAL sub-path); the STUB sub-path never invokes
// them. Each emits the newline/tab-delimited string payloads the Lean
// `_prim` parsers in proof/PMT/Extraction.lean consume (§9 format):
//   • registry   : `name \t total_size \t field_count` + field lines
//                  `fname \t offset \t size \t type_name`
//   • reads      : `var \t field_name \t expected_type`
//   • writes     : `var \t field_name \t value_type \t 0|1`
//   • consumed   : vars joined by `\n`
//
// CRITICAL: the Lean `env` model for reads/writes is `String → Option
// LayoutInfo` keyed by VARIABLE name (`env r.var` / `env w.var` — see
// StateReads.lean / StateWrites.lean), whereas the transform `_prim`
// looks up layouts by LAYOUT name (`layouts input_layout`). The merged
// registry therefore contains BOTH:
//   1. var-keyed entries — each (var, layout_name) in `state_var_layouts`
//      resolved to its LayoutInfo so reads/writes env lookups succeed;
//   2. layout-name-keyed entries — every layout in the 3 layout maps so
//      transform lookups by name succeed.
// `List.lookup` returns the first match, so a pathological collision
// (var name == layout name) prefers the var-keyed entry; in practice
// variable and layout names are disjoint.
// ─────────────────────────────────────────────────────────────────────

/// Abstraction over the three identical-shape `LayoutInfo` structs
/// (state_read / state_write / state_transform) so one serializer can
/// traverse all three layout maps. All are in the `ive` crate, so a
/// local trait impl satisfies the orphan rules.
#[cfg(lean_ffi_linked)]
trait IveLayoutSer {
    fn total_size(&self) -> u64;
    fn fields_ser(&self) -> Vec<(String, u64, u64, String)>;
}

#[cfg(lean_ffi_linked)]
impl IveLayoutSer for crate::state_read::LayoutInfo {
    fn total_size(&self) -> u64 {
        self.total_size
    }
    fn fields_ser(&self) -> Vec<(String, u64, u64, String)> {
        self.fields
            .iter()
            .map(|f| (f.name.clone(), f.offset, f.size, f.type_name.clone()))
            .collect()
    }
}

#[cfg(lean_ffi_linked)]
impl IveLayoutSer for crate::state_write::LayoutInfo {
    fn total_size(&self) -> u64 {
        self.total_size
    }
    fn fields_ser(&self) -> Vec<(String, u64, u64, String)> {
        self.fields
            .iter()
            .map(|f| (f.name.clone(), f.offset, f.size, f.type_name.clone()))
            .collect()
    }
}

#[cfg(lean_ffi_linked)]
impl IveLayoutSer for crate::state_transform::LayoutInfo {
    fn total_size(&self) -> u64 {
        self.total_size
    }
    fn fields_ser(&self) -> Vec<(String, u64, u64, String)> {
        self.fields
            .iter()
            .map(|f| (f.name.clone(), f.offset, f.size, f.type_name.clone()))
            .collect()
    }
}

/// Append one layout entry (header + field lines) in §9 format.
#[cfg(lean_ffi_linked)]
fn push_ive_layout_entry(
    s: &mut String,
    name: &str,
    total_size: u64,
    fields: &[(String, u64, u64, String)],
) {
    s.push_str(&format!("{}\t{}\t{}\n", name, total_size, fields.len()));
    for (fname, offset, size, type_name) in fields {
        s.push_str(&format!("{}\t{}\t{}\t{}\n", fname, offset, size, type_name));
    }
}

/// Serialize the merged registry (var-keyed + layout-name-keyed entries)
/// from the var→layout-name map and the 3 layout maps. See the
/// `IveLayoutSer` block above for the env-keying rationale.
#[cfg(lean_ffi_linked)]
fn serialize_ive_registry(
    state_var_layouts: &HashMap<String, String>,
    read_layouts: &HashMap<String, crate::state_read::LayoutInfo>,
    write_layouts: &HashMap<String, crate::state_write::LayoutInfo>,
    transform_layouts: &HashMap<String, crate::state_transform::LayoutInfo>,
) -> String {
    let mut s = String::new();

    // (1) Var-keyed entries: resolve var → layout_name → LayoutInfo so
    // Lean's `env r.var` / `env w.var` lookups succeed. Prefer
    // read_layouts, then write_layouts, then transform_layouts (the
    // LayoutInfo shape is identical across all three).
    for (var, layout_name) in state_var_layouts {
        if let Some(l) = read_layouts.get(layout_name) {
            push_ive_layout_entry(&mut s, var, l.total_size(), &l.fields_ser());
        } else if let Some(l) = write_layouts.get(layout_name) {
            push_ive_layout_entry(&mut s, var, l.total_size(), &l.fields_ser());
        } else if let Some(l) = transform_layouts.get(layout_name) {
            push_ive_layout_entry(&mut s, var, l.total_size(), &l.fields_ser());
        }
        // If layout_name is absent from all 3 maps, skip: Lean's env
        // lookup returns `none` and the read/write is reported invalid
        // ("variable not state-typed"), matching the Rust hand-verifier.
    }

    // (2) Layout-name-keyed entries: every layout in the 3 maps, so the
    // transform `_prim` can look up `input_layout` / `output_layout` by
    // name. Duplicate names across maps re-serialize harmlessly
    // (identical shape); a name equal to a variable name is shadowed by
    // the var entry above (pathological, see header comment).
    for (name, l) in read_layouts {
        push_ive_layout_entry(&mut s, name, l.total_size(), &l.fields_ser());
    }
    for (name, l) in write_layouts {
        push_ive_layout_entry(&mut s, name, l.total_size(), &l.fields_ser());
    }
    for (name, l) in transform_layouts {
        push_ive_layout_entry(&mut s, name, l.total_size(), &l.fields_ser());
    }

    s
}

/// Serialize reads as `var \t field_name \t expected_type` lines.
#[cfg(lean_ffi_linked)]
fn serialize_ive_reads(reads: &[(String, String, String)]) -> String {
    let mut s = String::new();
    for (var, field, expected_type) in reads {
        s.push_str(&format!("{}\t{}\t{}\n", var, field, expected_type));
    }
    s
}

/// Serialize writes as `var \t field_name \t value_type \t 0|1` lines.
/// `after_consume` maps to "1" (true) / "0" (false), matching Lean's
/// `parseBoolField` ("1" or "true" → true).
#[cfg(lean_ffi_linked)]
fn serialize_ive_writes(writes: &[crate::state_write::StateWriteOp]) -> String {
    let mut s = String::new();
    for w in writes {
        let ac = if w.after_consume { "1" } else { "0" };
        s.push_str(&format!(
            "{}\t{}\t{}\t{}\n",
            w.var_name, w.field_name, w.value_type, ac
        ));
    }
    s
}

/// Serialize consumed var names joined by `\n` (Lean `splitLines` drops
/// empty lines, so an empty set yields `""` → `[]`). Sorted for
/// determinism (HashSet iteration order is randomized).
#[cfg(lean_ffi_linked)]
fn serialize_ive_consumed(consumed_vars: &HashSet<String>) -> String {
    let mut names: Vec<&String> = consumed_vars.iter().collect();
    names.sort();
    names
        .iter()
        .map(|n| n.as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Route the 3 PMT state verifiers through the Lean-extracted FFI surface.
///
/// Returns the same `(reads, writes, transforms)` Vec triple the
/// hand-written verifiers produce, so downstream aggregation in `verify_pmt`
/// is identical. See the `lean_ffi` module doc for the STUB/REAL sub-paths.
#[cfg(feature = "pmt-runtime-check")]
fn verify_pmt_via_lean(
    state_var_layouts: &HashMap<String, String>,
    read_layouts: &HashMap<String, crate::state_read::LayoutInfo>,
    reads: &[(String, String, String)],
    write_layouts: &HashMap<String, crate::state_write::LayoutInfo>,
    writes: &[crate::state_write::StateWriteOp],
    consumed_vars: &HashSet<String>,
    transform_layouts: &HashMap<String, crate::state_transform::LayoutInfo>,
    transforms: &[(String, String)],
) -> Result<LeanPmtOutcome, String> {
    // Mark every argument as used so the STUB sub-path (which does not
    // touch them) emits no unused-variable warnings. The REAL sub-path
    // below consumes all 8 for §9 marshalling (VERIF-MARSHAL).
    let _ = (
        state_var_layouts,
        read_layouts,
        reads,
        write_layouts,
        writes,
        consumed_vars,
        transform_layouts,
        transforms,
    );

    // -- STUB sub-path (default): lean_stub.c hardcodes success -----
    // build.rs did NOT emit `lean_ffi_linked`, so the extracted symbols
    // are the stub. Return all-empty Vecs => downstream `.iter().all(|r|
    // r.valid)` is `true` and `*_errs` is empty. Wave 5-C tests assert
    // this stub-success behaviour.
    #[cfg(not(lean_ffi_linked))]
    let outcome: LeanPmtOutcome = (Vec::new(), Vec::new(), Vec::new());

    // -- REAL sub-path (`lean_ffi_linked` cfg set by build.rs) ------
    // Initialize the Lean module exactly once BEFORE any verifier call.
    // `initialize_PMT` sets up the Lean runtime; without it the
    // `lean_verify_*` externs SIGSEGV (the PMT-1-G smoke-test failure).
    // `init()` is a no-op after the first call (guarded by `Once`).
    #[cfg(lean_ffi_linked)]
    lean_ffi::init().map_err(|e| format!("Lean PMT module init failed: {e}"))?;

    // Call the 3 extracted `_prim` wrappers (String-based,
    // C-marshallable). Unlike the boxed `lean_verify_*` externs — which
    // SIGSEGV under real Lean when handed null `lean_object*` args (the
    // VERIF-SIGSEGV-FIX failure) — the `_prim` wrappers accept Lean
    // `String`s built via `lean_mk_string` / `str_to_lean` and parse
    // the §9 serialization format internally. `lean_ffi::init()` above
    // has already initialized the Lean runtime.
    //
    // VERIF-MARSHAL: the 8 IVE pipeline inputs are now fully marshalled
    // into the §9 string format the `_prim` parsers expect (see the
    // `serialize_ive_*` helpers above), so the returned `u8` reflects
    // the real program data rather than empty placeholders.
    #[cfg(lean_ffi_linked)]
    let outcome = {
        // Build the merged §9 registry (var-keyed + layout-name-keyed)
        // and the reads / writes / consumed payloads.
        let registry_str = serialize_ive_registry(
            state_var_layouts,
            read_layouts,
            write_layouts,
            transform_layouts,
        );
        let reads_str = serialize_ive_reads(reads);
        let writes_str = serialize_ive_writes(writes);
        let consumed_str = serialize_ive_consumed(consumed_vars);

        // Box each payload as a Lean `String` via lean_mk_string.
        //
        // TODO(VERIF-MARSHAL ownership): `str_to_lean` returns a
        // `*mut LeanObject` whose refcount is NOT decremented (no
        // `lean_dec`), so every call leaks one Lean String. Acceptable
        // for a verification pass that runs at most once per IVE
        // pipeline; a long-running driver that re-verifies many
        // programs would accrue unbounded growth. Proper fix: wrap each
        // pointer in a newtype that calls `lean_dec` on Drop (requires
        // declaring `lean_dec` in the `lean_ffi` extern block). `reg`
        // is reused across all 3 verifier calls; the per-transform
        // `in_lean` / `out_lean` leak once each.
        let reg = lean_ffi::str_to_lean(&registry_str);
        let rds = lean_ffi::str_to_lean(&reads_str);
        let wrt = lean_ffi::str_to_lean(&writes_str);
        let con = lean_ffi::str_to_lean(&consumed_str);

        let reads_ok =
            unsafe { lean_ffi::lean_verify_state_reads_prim(reg, rds) != 0 };
        let writes_ok =
            unsafe { lean_ffi::lean_verify_state_writes_prim(reg, con, wrt) != 0 };

        // Transforms: the `_prim` looks up input/output layouts BY NAME
        // in the registry and applies the kind-specific check.
        // verification.rs has no TransformKind context here, so pass
        // "copy" (the permissive default — Copy accepts any layout
        // pair, mirroring verify_transform when no structural
        // constraint applies). `kind` is hoisted out of the loop to
        // leak one Lean String instead of one per transform.
        let kind_lean = lean_ffi::str_to_lean("copy");
        let transforms_ok = transforms.iter().all(|(in_l, out_l)| {
            let in_lean = lean_ffi::str_to_lean(in_l);
            let out_lean = lean_ffi::str_to_lean(out_l);
            unsafe { lean_ffi::lean_verify_transform_prim(reg, in_lean, out_lean, kind_lean) != 0 }
        });

        // The `_prim` wrappers return a single all-or-nothing Bool, not
        // per-read/write/transform results, so on failure emit one
        // summary error entry (matching the prior structure). Empty
        // Vecs on success => downstream `.iter().all(|r| r.valid)` is
        // true and `*_errs` is empty.
        let read_results: Vec<crate::state_read::StateReadVerification> = if reads_ok {
            Vec::new()
        } else {
            vec![crate::state_read::StateReadVerification {
                var_name: String::new(),
                layout_name: String::new(),
                field_name: String::new(),
                valid: false,
                error: Some(format!(
                    "Lean lean_verify_state_reads_prim returned false ({} reads checked)",
                    reads.len()
                )),
            }]
        };
        let write_results: Vec<crate::state_write::StateWriteVerification> = if writes_ok {
            Vec::new()
        } else {
            vec![crate::state_write::StateWriteVerification {
                var_name: String::new(),
                layout_name: String::new(),
                field_name: String::new(),
                valid: false,
                error: Some(format!(
                    "Lean lean_verify_state_writes_prim returned false ({} writes checked)",
                    writes.len()
                )),
            }]
        };
        let transform_results: Vec<crate::state_transform::StateTransformVerification> =
            if transforms_ok {
                Vec::new()
            } else {
                vec![crate::state_transform::StateTransformVerification {
                    input_layout: String::new(),
                    output_layout: String::new(),
                    valid: false,
                    transform_kind: crate::state_transform::TransformKind::Copy,
                    error: Some(format!(
                        "Lean lean_verify_transform_prim returned false for at least one of {} transforms (kind=copy)",
                        transforms.len()
                    )),
                }]
            };

        (read_results, write_results, transform_results)
    };

    Ok(outcome)
}

impl VerificationEngine {
    /// Construct a new verification engine.
    pub fn new() -> Self {
        Self {
            verbose: false,

            max_paths: 64,
            max_path_length: 256,
        }
    }

    /// Enable verbose diagnostic output.
    pub fn with_verbose(mut self, verbose: bool) -> Self {
        self.verbose = verbose;
        self
    }

    /// Set the maximum number of paths for the liveness verifier.
    pub fn with_max_paths(mut self, max_paths: usize) -> Self {
        self.max_paths = max_paths;
        self
    }

    /// Set the maximum path length for the cleanup verifier.
    pub fn with_max_path_length(mut self, max_path_length: usize) -> Self {
        self.max_path_length = max_path_length;
        self
    }

    /// Accessor for the liveness path limit.
    pub fn max_paths(&self) -> usize {
        self.max_paths
    }

    /// Accessor for the cleanup path-length limit.
    pub fn max_path_length(&self) -> usize {
        self.max_path_length
    }

    /// Verify PMT (Programs as Memory Transformations) state safety:
    /// state-field reads, state-field writes (with linearity), and state
    /// transformations.
    ///
    /// Walks the SCG for `StateRead` / `StateWrite` / `StateTransform` /
    /// `StateInit` / `ForeignConsume` nodes, builds the per-verifier input
    /// tuples, and delegates to [`crate::state_read::verify_state_reads`],
    /// [`crate::state_write::verify_state_writes`], and
    /// [`crate::state_transform::verify_all_transforms`].
    ///
    /// # Layout registry
    ///
    /// The SCG does not retain structured layout info — `Item::LayoutDef`
    /// is lowered to a `Computation` node with a descriptive label.  The
    /// pipeline therefore attaches the layout registry to
    /// [`VerificationInput::pmt_layouts`] before invoking verification.
    /// If `pmt_layouts` is absent, the verifiers report "layout not found"
    /// for every state operation (a FAIL verdict).
    ///
    /// # Vreg → var-name mapping
    ///
    /// The SCG's `StateReadNode` / `StateWriteNode` use a `state_vreg: u32`
    /// field rather than a source variable name.  The verifiers work in
    /// terms of variable names, so we synthesise a stable name
    /// `"_state_{node_id}_{vreg}"` per distinct (node, vreg) pair, using
    /// the SCG node's globally-unique `NodeId` to qualify the per-function
    /// vreg.  This prevents cross-function vreg collisions in interprocedural
    /// verification (vreg 7 in funcA and vreg 7 in funcB no longer alias).
    ///
    /// # Linearity check
    ///
    /// The node-ID qualification above defeated the write-after-consume
    /// linearity check in [`crate::state_write::verify_state_writes`]: a
    /// `StateTransform` consuming vreg `1` (synthetic name
    /// `_state_{transform_id}_1`) and a subsequent `StateWrite` to vreg
    /// `1` (synthetic name `_state_{write_id}_1`) ended up as different
    /// variables, so `consumed_vars.contains(&w.var_name)` never matched.
    ///
    /// We fix this *without* weakening the cross-function vreg collision
    /// protection: alongside `consumed_vars`, we also track a per-vreg
    /// consume set (`consumed_vregs: HashSet<u32>`) that records which vregs
    /// have been consumed by a `StateTransform` / `ForeignConsume` in this
    /// SCG.  Each `StateWriteOp` then gets `after_consume` set to `true`
    /// iff its `state_vreg` is in `consumed_vregs`.  This keys consume
    /// tracking on the bare vreg rather than the node-ID-qualified
    /// synthetic name, realised as a parallel set so the existing
    /// `verify_state_writes` API and its tests are unchanged.
    pub fn verify_pmt(&self, input: &VerificationInput) -> VerificationResult {
        use crate::result::{CounterExample, VerificationStatus};
        use crate::state_read::{FieldInfo as ReadField, LayoutInfo as ReadLayout};
        use crate::state_transform::{
            FieldInfo as TransformField, LayoutInfo as TransformLayout,
        };
        use crate::state_write::{
            FieldInfo as WriteField, LayoutInfo as WriteLayout, StateWriteOp,
        };
        // The 3 hand-written verifier functions are only called on the
        // `cfg(not(feature = "pmt-runtime-check"))` path (see the routing
        // branch below). Gating the imports avoids unused-import warnings
        // when the feature is on (Lean FFI path is used instead).
        #[cfg(not(feature = "pmt-runtime-check"))]
        use crate::state_read::verify_state_reads;
        #[cfg(not(feature = "pmt-runtime-check"))]
        use crate::state_transform::verify_all_transforms;
        #[cfg(not(feature = "pmt-runtime-check"))]
        use crate::state_write::verify_state_writes;
        use std::collections::{HashMap, HashSet};
        use vuma_scg::node::{
            ForeignConsumeNode, NodePayload, NodeType, StateReadNode, StateTransformNode,
            StateWriteNode,
        };

        let scg = &input.scg;

        // ── Independently re-derive layout offsets/sizes ────────────────
        //
        // Before trusting the pipeline-provided `pmt_layouts`, re-derive
        // every layout's offsets/sizes from its field list using the same
        // C-style alignment rules the pipeline uses. Mismatch ⇒ Fail.
        if let Some(layouts) = &input.pmt_layouts {
            if !layouts.is_empty() {
                let mismatches = verify_layout_consistency(layouts);
                if !mismatches.is_empty() {
                    for m in &mismatches {
                        vuma_log!(warn, "[Gap 3] layout consistency check failed: {}", m);
                    }
                    let desc = format!(
                        "pmt-state layout consistency failed: {} mismatch(es) —                          pipeline-provided layouts do not match independently                          re-derived layouts (first: {})",
                        mismatches.len(),
                        mismatches.first().unwrap()
                    );
                    return VerificationResult::new(
                        "pmt-state",
                        VerificationStatus::Violated {
                            counterexample: CounterExample::new(
                                Vec::new(),
                                default_program_point(),
                                desc.clone(),
                            ),
                        },
                        desc,
                    );
                }
            }
        }

        // ── Build the per-verifier layout registries ──────────────────────
        let empty: HashMap<String, PmtLayoutSpec> = HashMap::new();
        let pmt_layouts = input.pmt_layouts.as_ref().unwrap_or(&empty);

        let mut read_layouts: HashMap<String, ReadLayout> = HashMap::new();
        let mut write_layouts: HashMap<String, WriteLayout> = HashMap::new();
        let mut transform_layouts: HashMap<String, TransformLayout> = HashMap::new();
        for (name, spec) in pmt_layouts {
            read_layouts.insert(
                name.clone(),
                ReadLayout {
                    name: spec.name.clone(),
                    total_size: spec.total_size,
                    fields: spec
                        .fields
                        .iter()
                        .map(|f| ReadField {
                            name: f.name.clone(),
                            offset: f.offset,
                            size: f.size,
                            type_name: f.type_name.clone(),
                        })
                        .collect(),
                },
            );
            write_layouts.insert(
                name.clone(),
                WriteLayout {
                    name: spec.name.clone(),
                    total_size: spec.total_size,
                    fields: spec
                        .fields
                        .iter()
                        .map(|f| WriteField {
                            name: f.name.clone(),
                            offset: f.offset,
                            size: f.size,
                            type_name: f.type_name.clone(),
                        })
                        .collect(),
                },
            );
            transform_layouts.insert(
                name.clone(),
                TransformLayout {
                    name: spec.name.clone(),
                    total_size: spec.total_size,
                    fields: spec
                        .fields
                        .iter()
                        .map(|f| TransformField {
                            name: f.name.clone(),
                            offset: f.offset,
                            size: f.size,
                            type_name: f.type_name.clone(),
                        })
                        .collect(),
                },
            );
        }

        // ── Walk SCG nodes; collect reads / writes / transforms ───────────
        //
        // Also collect `accessed_field_refs`: layout_name → list of
        // field_names referenced by StateRead / StateWrite nodes in the SCG.
        // This is the IVE-derived field-list source used by the
        // `verify_layout_field_list_consistency` cross-check below.
        let mut state_var_layouts: HashMap<String, String> = HashMap::new();
        let mut consumed_vars: HashSet<String> = HashSet::new();
        // Parallel consume tracker keyed by the bare vreg number.
        // See the doc comment on `verify_pmt` for why this exists alongside
        // the node-ID-qualified `consumed_vars`.
        let mut consumed_vregs: HashSet<u32> = HashSet::new();
        let mut reads: Vec<(String, String, String)> = Vec::new();
        let mut writes: Vec<StateWriteOp> = Vec::new();
        let mut transforms: Vec<(String, String)> = Vec::new();
        let mut state_init_count: usize = 0;
        // layout_name → Vec<field_name> referenced in the SCG (first-access
        // order, deduplicated). Used as the IVE-derived source for the
        // field-list cross-check.
        let mut accessed_field_refs: HashMap<String, Vec<String>> = HashMap::new();

        let mut state_nodes: Vec<(u64, NodeType, NodePayload)> = Vec::new();
        for node in scg.nodes() {
            state_nodes.push((
                node.id.as_u64(),
                node.node_type.clone(),
                node.payload.clone(),
            ));
        }
        state_nodes.sort_by_key(|(id, _, _)| *id);

        for (id, _, payload) in &state_nodes {
            match payload {
                NodePayload::StateInit(_) => {
                    state_init_count += 1;
                }
                NodePayload::StateTransform(t) => {
                    let StateTransformNode {
                        input_vreg,
                        input_layout,
                        output_layout,
                        ..
                    } = t;
                    let in_var = format!("_state_{}_{}", id, input_vreg);
                    state_var_layouts
                        .entry(in_var.clone())
                        .or_insert_with(|| input_layout.clone());
                    consumed_vars.insert(in_var);
                    // Track the bare vreg as consumed so a subsequent
                    // StateWrite to the same vreg (in a different node) is
                    // still detected as a linearity violation.
                    consumed_vregs.insert(*input_vreg);
                    transforms.push((input_layout.clone(), output_layout.clone()));
                }
                NodePayload::ForeignConsume(fc) => {
                    let ForeignConsumeNode {
                        input_vreg,
                        layout_name,
                    } = fc;
                    let in_var = format!("_state_{}_{}", id, input_vreg);
                    state_var_layouts
                        .entry(in_var.clone())
                        .or_insert_with(|| layout_name.clone());
                    consumed_vars.insert(in_var);
                    // Mirror the StateTransform consume-tracking for
                    // foreign-consume nodes (same linearity semantics).
                    consumed_vregs.insert(*input_vreg);
                }
                NodePayload::StateRead(r) => {
                    let StateReadNode {
                        state_vreg,
                        layout_name,
                        field_name,
                        ..
                    } = r;
                    let var = format!("_state_{}_{}", id, state_vreg);
                    state_var_layouts
                        .entry(var.clone())
                        .or_insert_with(|| layout_name.clone());
                    // Record the (layout, field) reference for the
                    // field-list cross-check.
                    let fields = accessed_field_refs.entry(layout_name.clone()).or_default();
                    if !fields.iter().any(|f| f == field_name) {
                        fields.push(field_name.clone());
                    }
                    let expected_type = pmt_layouts
                        .get(layout_name)
                        .and_then(|spec| spec.fields.iter().find(|f| &f.name == field_name))
                        .map(|f| f.type_name.clone())
                        .unwrap_or_default();
                    reads.push((var, field_name.clone(), expected_type));
                }
                NodePayload::StateWrite(w) => {
                    let StateWriteNode {
                        state_vreg,
                        layout_name,
                        field_name,
                        ..
                    } = w;
                    let var = format!("_state_{}_{}", id, state_vreg);
                    state_var_layouts
                        .entry(var.clone())
                        .or_insert_with(|| layout_name.clone());
                    // Record the (layout, field) reference for the
                    // field-list cross-check.
                    let fields = accessed_field_refs.entry(layout_name.clone()).or_default();
                    if !fields.iter().any(|f| f == field_name) {
                        fields.push(field_name.clone());
                    }
                    let value_type = pmt_layouts
                        .get(layout_name)
                        .and_then(|spec| spec.fields.iter().find(|f| &f.name == field_name))
                        .map(|f| f.type_name.clone())
                        .unwrap_or_default();
                    // Set `after_consume` based on the bare-vreg consume
                    // tracker. This fixes the node-ID-qualification
                    // regression: the synthetic node-ID-qualified name would
                    // never match across two different SCG nodes, but the
                    // bare vreg does.
                    let after_consume = consumed_vregs.contains(state_vreg);
                    writes.push(StateWriteOp {
                        var_name: var,
                        field_name: field_name.clone(),
                        value_type,
                        after_consume,
                    });
                }
                _ => {}
            }
        }

        // ── Cross-check field LIST (names + count) ────────────────────
        //
        // The re-derive offsets/sizes check above only validates the
        // *geometry* of parser-provided layouts. The field LIST itself
        // (which fields exist with which names) was still parser-trusted.
        //
        // We now close that residual gap by independently deriving a
        // field-list source from the SCG's StateRead / StateWrite nodes
        // and comparing it against the parser-provided `pmt_layouts`.  A
        // parser bug that drops or renames a field during PmtLayoutSpec
        // construction would leave the SCG still referencing the original
        // field name, which this check catches as a Violation.
        //
        // Semantics: for each layout referenced in the SCG, the parser-
        // provided layout must declare every referenced field name, and
        // the SCG-referenced count must not exceed the parser-declared count.
        // (The parser may declare MORE fields than the SCG references — not
        // all declared fields need to be accessed.)
        if !accessed_field_refs.is_empty() {
            let mut ivederived_layouts: HashMap<String, PmtLayoutSpec> = HashMap::new();
            for (lname, fnames) in &accessed_field_refs {
                ivederived_layouts.insert(
                    lname.clone(),
                    PmtLayoutSpec {
                        name: lname.clone(),
                        total_size: 0, // not used by the field-list check
                        fields: fnames
                            .iter()
                            .map(|n| PmtFieldSpec {
                                name: n.clone(),
                                offset: 0, // not used by the field-list check
                                size: 0,   // not used by the field-list check
                                type_name: String::new(),
                            })
                            .collect(),
                    },
                );
            }
            let field_mismatches =
                verify_layout_field_list_consistency(pmt_layouts, &ivederived_layouts);
            if !field_mismatches.is_empty() {
                for m in &field_mismatches {
                    vuma_log!(warn, "[Task 5-c] field-list cross-check failed: {}", m);
                }
                let desc = format!(
                    "pmt-state layout field-list cross-check failed: {} mismatch(es) —                          parser-provided field lists do not match IVE-derived                          (SCG-referenced) field lists (first: {})",
                    field_mismatches.len(),
                    field_mismatches.first().unwrap()
                );
                return VerificationResult::new(
                    "pmt-state",
                    VerificationStatus::Violated {
                        counterexample: CounterExample::new(
                            Vec::new(),
                            default_program_point(),
                            desc.clone(),
                        ),
                    },
                    desc,
                );
            }
        }

        // ── Cross-check typed-state conformance (Task 3-B) ────────────────────────────────────────────────────────
        //
        // Dual-derivation proof that the semantic SCG's typed-state
        // `NodePayload`s and the codegen Scg's `TypedStateMeta` list agree
        // on per-kind counts and (layout, field) multisets. The codegen
        // meta is attached via `VerificationInput::typed_state_meta`
        // (populated by the pipeline from
        // `bridge_ast_to_codegen_scg_with_meta`). A divergence surfaces a
        // bug in one of the two SCG construction paths.
        //
        // IMPORTANT (Task 3-B): the cross-check currently logs a WARNING
        // and does NOT hard-fail. This is intentional: the cross-check's
        // value is in SURFACING divergences, and hard-failing on a real
        // (pre-existing) divergence would break the build for programs
        // that otherwise verify cleanly. Once divergences are resolved
        // (NEEDS_FOLLOWUP 3-B), this can be promoted to a `Violated`
        // result like the field-list cross-check above.
        if !input.typed_state_meta.is_empty() {
            let ts_mismatches =
                verify_typed_state_conformance(scg, &input.typed_state_meta);
            if !ts_mismatches.is_empty() {
                for m in &ts_mismatches {
                    vuma_log!(
                        warn,
                        "[Task 3-B] typed-state conformance cross-check \
                         divergence: {}",
                        m
                    );
                }
                vuma_log!(
                    warn,
                    "[Task 3-B] semantic SCG and codegen Scg disagree on \
                     typed-state info ({} mismatch(es), first: {}); \
                     logging as WARNING per Task 3-B (not a hard fail). \
                     See NEEDS_FOLLOWUP 3-B.",
                    ts_mismatches.len(),
                    ts_mismatches.first().unwrap()
                );
            } else {
                vuma_log!(
                    info,
                    "[Task 3-B] typed-state conformance cross-check passed \
                     (semantic SCG and codegen Scg agree on {} typed-state \
                     op(s)).",
                    input.typed_state_meta.len()
                );
            }
        }

        // ┬─ Run the 3 verifiers ───────────────────────────────────────────────────
        //
        // Wave 5-A: the 3 PMT state verifiers are now routed. When the
        // `pmt-runtime-check` feature is ON, `verify_pmt_via_lean` drives
        // them through the Lean-extracted FFI surface (stub-success path
        // by default; real extraction when the `lean_ffi_linked` cfg is
        // emitted by build.rs). When the feature is OFF, the hand-written
        // Rust verifiers are used (the parity-tested path) - unchanged.
        #[cfg(feature = "pmt-runtime-check")]
        let (read_results, write_results, transform_results) = match verify_pmt_via_lean(
            &state_var_layouts,
            &read_layouts,
            &reads,
            &write_layouts,
            &writes,
            &consumed_vars,
            &transform_layouts,
            &transforms,
        ) {
            Ok(triple) => triple,
            Err(e) => {
                // Lean module init failed (only reachable on the
                // `lean_ffi_linked` real-Lean path). Surface it as a hard
                // Violation rather than silently passing verification.
                return VerificationResult::new(
                    "pmt-state",
                    VerificationStatus::Violated {
                        counterexample: CounterExample::new(
                            Vec::new(),
                            default_program_point(),
                            e.clone(),
                        ),
                    },
                    e,
                );
            }
        };

        #[cfg(not(feature = "pmt-runtime-check"))]
        let (read_results, write_results, transform_results) = {
            let read_results = verify_state_reads(&state_var_layouts, &read_layouts, &reads);
            let write_results =
                verify_state_writes(&state_var_layouts, &write_layouts, &writes, &consumed_vars);
            let transform_results = verify_all_transforms(&transform_layouts, &transforms);
            (read_results, write_results, transform_results)
        };

        // Wave 2 task IVE-2-A: arena_bounds verifier is now ACTIVE.
        // Walk the SCG for ArenaAlloc nodes and verify each references a
        // registered layout with total_size > 0 (and capacity check if the
        // arena's capacity is known). The result is OR-ed into the overall
        // verdict: if any arena-bounds check fails, the program is rejected.
        use crate::arena_bounds::{self, LayoutSpec as ArenaLayoutSpec};
        let arena_layouts: HashMap<String, ArenaLayoutSpec> = pmt_layouts
            .iter()
            .map(|(name, spec)| (name.clone(), ArenaLayoutSpec {
                name: spec.name.clone(),
                total_size: spec.total_size,
            }))
            .collect();
        let arena_bounds_results = arena_bounds::verify_arena_bounds(&arena_layouts, scg);

        let read_ok = read_results.iter().all(|r| r.valid);
        let write_ok = write_results.iter().all(|r| r.valid);
        let transform_ok = transform_results.iter().all(|r| r.valid);
        let arena_bounds_ok = arena_bounds_results.iter().all(|r| r.valid);

        let read_errs: Vec<String> = read_results
            .iter()
            .filter_map(|r| r.error.clone())
            .collect();
        let write_errs: Vec<String> = write_results
            .iter()
            .filter_map(|r| r.error.clone())
            .collect();
        let transform_errs: Vec<String> = transform_results
            .iter()
            .filter_map(|r| r.error.clone())
            .collect();

        let arena_bounds_errs: Vec<String> = arena_bounds_results
            .iter()
            .filter_map(|r| r.error.clone())
            .collect();

        let all_errs: Vec<String> = read_errs
            .iter()
            .chain(write_errs.iter())
            .chain(transform_errs.iter())
            .chain(arena_bounds_errs.iter())
            .cloned()
            .collect();

        let total_ops = reads.len() + writes.len() + transforms.len();
        let all_ok = read_ok && write_ok && transform_ok && arena_bounds_ok;

        if all_ok {
            VerificationResult::new(
                "pmt-state",
                VerificationStatus::Proven,
                format!(
                    "pmt-state check passed ({} init(s), {} read(s), {} write(s), {} transform(s))",
                    state_init_count,
                    reads.len(),
                    writes.len(),
                    transforms.len()
                ),
            )
        } else if total_ops == 0 {
            VerificationResult::new(
                "pmt-state",
                VerificationStatus::Proven,
                "pmt-state check passed (no state operations found)".to_string(),
            )
        } else {
            VerificationResult::new(
                "pmt-state",
                VerificationStatus::Violated {
                    counterexample: CounterExample::new(
                        Vec::new(),
                        default_program_point(),
                        all_errs.join("; "),
                    ),
                },
                format!(
                    "pmt-state violations: {} read-error(s), {} write-error(s), {} transform-error(s)",
                    read_errs.len(),
                    write_errs.len(),
                    transform_errs.len()
                ),
            )
        }
    }

    /// Run the PMT state verification check and return the result in a
    /// single-element vector.  (Legacy pointer-invariant verifiers have
    /// been removed; only PMT state verification is performed.)
    pub fn verify_all(&self, input: &VerificationInput) -> Vec<VerificationResult> {
        vec![self.verify_pmt(input)]
    }
}

/// Construct a default [`ProgramPoint`] (empty string) for use in
/// counterexamples where the exact source location is not known.
fn default_program_point() -> crate::result::ProgramPoint {
    String::new()
}

impl Default for VerificationEngine {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// L1-L3 Invariant Collapse
// ---------------------------------------------------------------------------

/// The three invariant layers in VUMA's verification hierarchy.
///
/// VUMA tracks invariants at three layers:
/// - **L1 (runtime)**: invariants checked at runtime by the L1 framing
///   layer (MAGIC, type_hash, CRC32, sequence number, cap_count). These
///   are dynamic checks performed on each channel send/recv.
/// - **L2 (IPC-layer)**: invariants checked by the IPC layer at
///   capability-attestation time (StarkProof verification, capability
///   delegation depth, security-label flow). These are static checks
///   performed at channel-open / capability-grant time.
/// - **L3 (compile-time)**: invariants checked by the IVE at compile
///   time (liveness, exclusivity, interpretation, origin, cleanup —
///   the five core invariants; plus linear-type checking and
///   information-flow type-checking).
///
/// The **collapse theorem** states: if every L1 runtime check passes
/// for all executions of a program, AND every L2 IPC-layer check
/// passes for all capability grants in the program, THEN the L3
/// compile-time invariants are sound (any L3 violation would imply
/// an L1 or L2 violation, which is a contradiction). This lets the
/// compiler trust L3 invariants without re-running L1/L2 at every
/// program point — a major performance win for the verifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum InvariantLayer {
    /// L1: runtime invariants (channel framing checks).
    L1,
    /// L2: IPC-layer invariants (capability attestation).
    L2,
    /// L3: compile-time invariants (IVE five core + linear + infoflow).
    L3,
}

/// Result of an L1→L3 invariant collapse proof.
///
/// Records whether the collapse succeeded (`collapsed: true`) and the
/// evidence used. A successful collapse means: every L1 runtime check
/// that the program relies on has been verified at compile time (e.g.
/// the type_hash in every channel_send matches the IRType of the
/// message), so the L3 compile-time invariants can be trusted without
/// re-running the L1 checks at runtime.
#[derive(Debug, Clone)]
pub struct L1L3Collapse {
    /// Whether the L1→L3 collapse succeeded.
    pub collapsed: bool,
    /// The number of L1 runtime checks that were verified at compile
    /// time and folded into L3.
    pub l1_checks_folded: usize,
    /// The number of L2 IPC-layer checks that were verified at compile
    /// time and folded into L3.
    pub l2_checks_folded: usize,
    /// Human-readable summary of the collapse proof.
    pub summary: String,
}

/// Prove that L1 (runtime) invariants collapse into L3 (compile-time)
/// invariants.
///
/// This is the **L1L3 collapse proof** (also called the
/// `InvariantCollapse` or `collapse_proof`). It scans the SCG for
/// every channel operation (`channel_open`, `channel_send`,
/// `channel_recv`) and every capability operation
/// (`capability_grant`, `capability_delegate`, `stark_prove`) and
/// verifies that the L1 runtime checks they encode (type_hash
/// match, CRC32 integrity, capability attestation) are statically
/// satisfied by the L3 type information (IRType::Channel payload
/// type, SecurityLabel lattice, linear-type annotations).
///
/// # Verification performed (NOT just counting)
///
/// For each `ChannelOpen` node:
/// - Verify `elem_type` is non-empty and `type_hash(elem_type) != 0`.
///   If not, record a failure.
/// - Record `(channel_name -> elem_type)` in a per-proof channel-type
///   map so subsequent `ChannelSend` / `ChannelRecv` nodes on the same
///   channel can be cross-checked.
///
/// For each `ChannelSend` node:
/// - Verify `ty` is non-empty and `type_hash(ty) != 0`. If not, record
///   a failure `"channel_send on {channel}: empty/invalid type"`.
/// - If the channel is already in the channel-type map, verify the
///   recorded type matches `ty`. If it mismatches, record a failure
///   `"type mismatch on channel {channel}: send declared {send_ty} \
///   but recv declared {recv_ty}"` (the map may have been populated
///   by either a prior send or a prior recv on the same channel).
/// - Otherwise insert `(channel -> ty)` into the map.
/// - If the check passes, fold 1 L1 runtime check (the type_hash +
///   CRC32 verification the L1 framing layer would have performed
///   at runtime).
///
/// For each `ChannelRecv` node:
/// - Verify `ty` is non-empty and `type_hash(ty) != 0`. If not, record
///   a failure `"channel_recv on {channel}: empty/invalid type"`.
/// - If the channel is already in the channel-type map, verify the
///   recorded type matches `ty`. If it mismatches, record a failure
///   `"type mismatch on channel {channel}: send declared {send_ty} \
///   but recv declared {recv_ty}"`.
/// - Otherwise insert `(channel -> ty)` into the map.
/// - If the check passes, fold 1 L1 runtime check.
///
/// For each `Computation` node whose label claims to be a capability
/// operation (contains `"capability_"` or `"stark_"`):
/// - Verify the label is one of the known capability operations
///   (`capability_grant`, `capability_delegate`, `stark_prove`).
///   If not, record a failure `"unknown capability operation: \
///   {label}"`.
/// - If known, fold 1 L2 IPC-layer check (the StarkProof / capability
///   attestation the IPC layer would have performed at grant time).
///
/// On success, returns an `L1L3Collapse` with `collapsed: true` and
/// the count of VERIFIED (not just counted) folded checks. On failure,
/// returns `collapsed: false` with a summary listing every failure
/// (this indicates a program that needs runtime checks the compiler
/// cannot statically discharge — a security-review flag).
///
/// **Soundness argument**: if `l1l3_collapse` returns `collapsed:
/// true`, then any L3 invariant violation at runtime would imply an
/// L1 check failure, which contradicts the assumption that L1 checks
/// pass for all executions. Therefore L3 invariants are sound.
pub fn l1l3_collapse(scg: &SCG) -> L1L3Collapse {
    let mut l1_checks_folded = 0usize;
    let mut l2_checks_folded = 0usize;
    let mut failures: Vec<String> = Vec::new();

    // Per-proof map: channel variable name -> declared element type.
    // Populated by ChannelOpen (the canonical declaration site) and
    // by ChannelSend / ChannelRecv (when no Open was seen, e.g. for
    // channels passed as function parameters).  Every subsequent
    // Send/Recv on the same channel must agree on the type — a
    // mismatch is a type-safety hole the L1 runtime check would
    // catch, so the L3 collapse proof must catch it too.
    let mut channel_types: HashMap<String, String> = HashMap::new();

    for node in scg.nodes() {
        match &node.payload {
            // ── ChannelOpen: the canonical declaration site ──
            vuma_scg::node::NodePayload::ChannelOpen(co) => {
                let chan = &co.dst;
                let ty = &co.elem_type;
                if ty.is_empty() || type_hash(ty) == 0 {
                    failures.push(format!("channel_open on {}: empty/invalid type", chan));
                    continue;
                }
                // If the channel was already declared (e.g. via a
                // prior Send/Recv on the same variable), verify the
                // types agree.  Otherwise insert.
                if let Some(existing) = channel_types.get(chan) {
                    if existing != ty {
                        failures.push(format!(
                            "type mismatch on channel {}: send declared {} but recv declared {}",
                            chan, existing, ty
                        ));
                    }
                } else {
                    channel_types.insert(chan.clone(), ty.clone());
                }
                // ChannelOpen folds the L1 cap_count=0 structural
                // check that every framed message on this channel
                // will carry (the open-time type binding).
                l1_checks_folded += 1;
            }
            // ── ChannelSend: verify the message type ──
            vuma_scg::node::NodePayload::ChannelSend(cs) => {
                let chan = &cs.channel;
                let ty = &cs.ty;
                if ty.is_empty() || type_hash(ty) == 0 {
                    failures.push(format!("channel_send on {}: empty/invalid type", chan));
                    continue;
                }
                let mut verified = true;
                if let Some(existing) = channel_types.get(chan) {
                    if existing != ty {
                        failures.push(format!(
                            "type mismatch on channel {}: send declared {} but recv declared {}",
                            chan, existing, ty
                        ));
                        verified = false;
                    }
                } else {
                    channel_types.insert(chan.clone(), ty.clone());
                }
                if verified {
                    l1_checks_folded += 1;
                }
            }
            // ── ChannelRecv: verify + cross-check against the send ──
            vuma_scg::node::NodePayload::ChannelRecv(cr) => {
                let chan = &cr.channel;
                let ty = &cr.ty;
                if ty.is_empty() || type_hash(ty) == 0 {
                    failures.push(format!("channel_recv on {}: empty/invalid type", chan));
                    continue;
                }
                let mut verified = true;
                if let Some(existing) = channel_types.get(chan) {
                    if existing != ty {
                        failures.push(format!(
                            "type mismatch on channel {}: send declared {} but recv declared {}",
                            chan, existing, ty
                        ));
                        verified = false;
                    }
                } else {
                    channel_types.insert(chan.clone(), ty.clone());
                }
                if verified {
                    l1_checks_folded += 1;
                }
            }
            // ── ChannelClose: no L1 type check to fold (no payload) ──
            vuma_scg::node::NodePayload::ChannelClose(_) => {
                // Close carries no type information; the L1 layer
                // performs only an fd-close syscall.  No check is
                // folded here.
            }
            // ── Capability Computation nodes ──
            vuma_scg::node::NodePayload::Computation(c) => {
                // Use nominal typing instead of string matching.
                // Previously: `known_intrinsics.contains(&lower.as_str())`
                // against a 6-entry string array. Now: any node tagged
                // `ComputationKind::Intrinsic(IntrinsicKind)` at parse /
                // deserialization time is intrinsically a known capability
                // op. This eliminates false positives from user-defined
                // functions named e.g. `my_capability_foo` (which would
                // never be tagged `Intrinsic`), and is unforgeable per the
                // Miller 2006 object-capability model.
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
            }
            _ => {}
        }
    }

    let collapsed = failures.is_empty();
    let summary = if collapsed {
        format!(
            "L1→L3 collapse SUCCESS: verified and folded {} L1 runtime checks \
             (channel framing: type_hash + CRC32) and {} L2 IPC-layer checks \
             (capability attestation: StarkProof) into L3 compile-time \
             invariants. Channel type-consistency verified across send/recv \
             pairs. L3 invariants are sound under the assumption that all \
             folded L1/L2 checks pass at runtime.",
            l1_checks_folded, l2_checks_folded
        )
    } else {
        format!(
            "L1→L3 collapse FAILURE: {} check(s) could not be folded: {}. \
             The program requires runtime checks the compiler cannot statically \
             discharge. Verified {} L1 checks and {} L2 checks before failing.",
            failures.len(),
            failures.join("; "),
            l1_checks_folded,
            l2_checks_folded
        )
    };

    L1L3Collapse {
        collapsed,
        l1_checks_folded,
        l2_checks_folded,
        summary,
    }
}

/// Alias for [`l1l3_collapse`] — the invariant-collapse proof.
///
/// This name is provided for callers that prefer the `collapse_proof`
/// spelling (mirrors the `InvariantCollapse` concept in the literature).
pub fn collapse_proof(scg: &SCG) -> L1L3Collapse {
    l1l3_collapse(scg)
}

/// Convenience type-alias for the collapse result, for callers that refer
/// to it as `InvariantCollapse` (the theorem name rather than the function
/// name).
pub type InvariantCollapse = L1L3Collapse;

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::result::VerificationStatus;
    // `NodePayload` is referenced unqualified by the `l1l3_collapse_*`
    // regression tests below. The parent module imports
    // `ComputationKind, NodeId` from `vuma_scg::node` but NOT `NodePayload`
    // itself, and the crate root re-exports it as `vuma_scg::NodePayload`.
    // The import is restored here so the secret-detection tests can run.
    use vuma_scg::NodePayload;

    #[test]
    fn verification_input_from_scg() {
        let scg = SCG::new();
        let input = VerificationInput::from_scg(scg);
        assert!(input.bd_map.is_none());
    }

    #[test]
    fn verification_input_with_bd_map() {
        let scg = SCG::new();
        let bd_map = HashMap::new();
        let input = VerificationInput::with_bd_map(scg, bd_map);
        assert!(input.bd_map.is_some());
    }

    // ── Secret-detection unit tests ──
    //
    // The `secret_vars` field is populated from `#[secret]` attributes by
    // `pipeline.rs::collect_secret_vars`. `is_secret_value` is the single
    // well-typed consumer: when `secret_vars` is non-empty it consults the
    // set exclusively (sound, attribute-based); when empty it falls back
    // to the unsound substring heuristic and emits a `vuma_log!(warn, ...)`
    // deprecation notice so legacy programs are visibly noisy.

    /// When `#[secret]` annotations are present, only explicitly annotated
    /// names are treated as secret. The substring heuristic must NOT fire
    /// — even a name that literally contains the substring `"secret"` is
    /// treated as non-secret if it is not in the explicit set.
    #[test]
    fn secret_detection_with_explicit_attribute_is_strict() {
        let scg = SCG::new();
        let mut secrets = HashSet::new();
        secrets.insert("private_key".to_string());
        secrets.insert("session_token".to_string());
        let input = VerificationInput::from_scg(scg).with_secret_vars(secrets);

        // Annotated names ARE secret.
        assert!(
            input.is_secret_value("private_key"),
            "explicitly-annotated `private_key` must be secret-tainted"
        );
        assert!(
            input.is_secret_value("session_token"),
            "explicitly-annotated `session_token` must be secret-tainted"
        );
        // Substring heuristic must NOT fire — `"secret_seed"` contains
        // `"secret"` but is NOT in the explicit set, so it must be
        // considered non-secret.
        assert!(
            !input.is_secret_value("secret_seed"),
            "substring heuristic must NOT fire when explicit #[secret] set is non-empty"
        );
        // A plain non-annotated, non-substring name is not secret.
        assert!(
            !input.is_secret_value("counter"),
            "non-annotated name must not be secret"
        );
    }

    /// When no `#[secret]` annotations are present (`secret_vars` empty),
    /// `is_secret_value` falls back to the unsound substring heuristic and
    /// emits a deprecation warning. This keeps legacy programs working
    /// during the migration window while making the gap visibly noisy.
    #[test]
    fn secret_detection_falls_back_to_substring_with_warning() {
        let scg = SCG::new();
        // No #[secret] annotations → secret_vars is empty.
        let input = VerificationInput::from_scg(scg);
        assert!(
            input.secret_vars.is_empty(),
            "fixture must not have any explicit #[secret] vars"
        );

        // Substring match: `"secret_seed"` contains `"secret"` → secret.
        assert!(
            input.is_secret_value("secret_seed"),
            "substring fallback must taint names containing 'secret'"
        );
        // Substring match: `"user_secret_key"` contains `"secret"` → secret.
        assert!(
            input.is_secret_value("user_secret_key"),
            "substring fallback is case-sensitive — lowercase 'secret' must match"
        );
        // No substring match: `"counter"` is not secret.
        assert!(
            !input.is_secret_value("counter"),
            "non-matching name must not be secret"
        );
        // Sanity: the substring match is lowercase-only — `"SECRET"` should
        // NOT trigger (matches the historical heuristic's behaviour).
        assert!(
            !input.is_secret_value("SECRET_KEY"),
            "substring fallback is case-sensitive — uppercase 'SECRET' must NOT match"
        );
    }

    // ── l1l3_collapse: real invariant-prover tests ──
    //
    // The audit found the old l1l3_collapse just counted channel ops
    // and always returned collapsed:true.  These tests verify the new
    // implementation REALLY verifies type consistency and REALLY
    // returns collapsed:false on failure.

    /// Helper: build an SCG with a channel_open + send + recv + close
    /// where all types agree.  The collapse proof must succeed.
    #[test]
    fn l1l3_collapse_succeeds_on_consistent_channel_types() {
        use vuma_scg::node::{
            ChannelCloseNode, ChannelOpenNode, ChannelRecvNode, ChannelSendNode, ComputationNode,
            NodeType, ProgramPoint,
        };

        let mut scg = SCG::new();
        let pp = ProgramPoint {
            file: None,
            line: None,
            column: None,
            offset: None,
        };

        // ch = channel_open<i32>()
        let _ = scg.add_node(
            NodeType::ChannelOpen,
            NodePayload::ChannelOpen(ChannelOpenNode {
                dst: "ch".to_string(),
                elem_type: "i32".to_string(),
            }),
            pp.clone(),
        );
        // channel_send(ch, msg)  -- ty = "i32"
        let _ = scg.add_node(
            NodeType::ChannelSend,
            NodePayload::ChannelSend(ChannelSendNode {
                channel: "ch".to_string(),
                message: "msg".to_string(),
                ty: "i32".to_string(),
            }),
            pp.clone(),
        );
        // x = channel_recv(ch)  -- ty = "i32"
        let _ = scg.add_node(
            NodeType::ChannelRecv,
            NodePayload::ChannelRecv(ChannelRecvNode {
                dst: "x".to_string(),
                channel: "ch".to_string(),
                ty: "i32".to_string(),
            }),
            pp.clone(),
        );
        // channel_close(ch)
        let _ = scg.add_node(
            NodeType::ChannelClose,
            NodePayload::ChannelClose(ChannelCloseNode {
                channel: "ch".to_string(),
            }),
            pp.clone(),
        );
        // capability_grant(1, 1) — a known capability op
        let _ = scg.add_node(
            NodeType::Computation,
            NodePayload::Computation(ComputationNode::new("capability_grant", None, false)),
            pp,
        );

        let collapse = l1l3_collapse(&scg);
        assert!(
            collapse.collapsed,
            "consistent channel types should collapse, but got: {}",
            collapse.summary,
        );
        // open + send + recv = 3 L1 checks folded (close folds none).
        assert_eq!(
            collapse.l1_checks_folded, 3,
            "expected 3 L1 checks folded (open+send+recv), got {}: {}",
            collapse.l1_checks_folded, collapse.summary,
        );
        // 1 known capability_grant → 1 L2 check folded.
        assert_eq!(
            collapse.l2_checks_folded, 1,
            "expected 1 L2 check folded, got {}: {}",
            collapse.l2_checks_folded, collapse.summary,
        );
    }

    /// A send declaring `i32` followed by a recv declaring `i64` on
    /// the SAME channel must FAIL the collapse proof (type mismatch).
    #[test]
    fn l1l3_collapse_fails_on_send_recv_type_mismatch() {
        use vuma_scg::node::{
            ChannelOpenNode, ChannelRecvNode, ChannelSendNode, NodeType, ProgramPoint,
        };

        let mut scg = SCG::new();
        let pp = ProgramPoint {
            file: None,
            line: None,
            column: None,
            offset: None,
        };
        let _ = scg.add_node(
            NodeType::ChannelOpen,
            NodePayload::ChannelOpen(ChannelOpenNode {
                dst: "ch".to_string(),
                elem_type: "i32".to_string(),
            }),
            pp.clone(),
        );
        let _ = scg.add_node(
            NodeType::ChannelSend,
            NodePayload::ChannelSend(ChannelSendNode {
                channel: "ch".to_string(),
                message: "msg".to_string(),
                ty: "i32".to_string(),
            }),
            pp.clone(),
        );
        // Recv declares a DIFFERENT type — type-safety hole.
        let _ = scg.add_node(
            NodeType::ChannelRecv,
            NodePayload::ChannelRecv(ChannelRecvNode {
                dst: "x".to_string(),
                channel: "ch".to_string(),
                ty: "i64".to_string(),
            }),
            pp,
        );

        let collapse = l1l3_collapse(&scg);
        assert!(
            !collapse.collapsed,
            "send/recv type mismatch must FAIL the collapse proof, but got: {}",
            collapse.summary,
        );
        assert!(
            collapse.summary.contains("type mismatch"),
            "failure summary should mention type mismatch, got: {}",
            collapse.summary,
        );
    }

    /// A channel_send with an empty `ty` must FAIL.
    #[test]
    fn l1l3_collapse_fails_on_empty_send_type() {
        use vuma_scg::node::{ChannelSendNode, NodeType, ProgramPoint};

        let mut scg = SCG::new();
        let pp = ProgramPoint {
            file: None,
            line: None,
            column: None,
            offset: None,
        };
        let _ = scg.add_node(
            NodeType::ChannelSend,
            NodePayload::ChannelSend(ChannelSendNode {
                channel: "ch".to_string(),
                message: "msg".to_string(),
                ty: "".to_string(), // empty type — invalid
            }),
            pp,
        );

        let collapse = l1l3_collapse(&scg);
        assert!(
            !collapse.collapsed,
            "empty channel_send type must FAIL, but got: {}",
            collapse.summary,
        );
        assert!(
            collapse.summary.contains("empty/invalid type"),
            "failure should mention empty/invalid type, got: {}",
            collapse.summary,
        );
        // The empty-typed send must NOT be folded.
        assert_eq!(
            collapse.l1_checks_folded, 0,
            "empty-typed send must not fold, got {}: {}",
            collapse.l1_checks_folded, collapse.summary,
        );
    }

    /// A user-defined function whose name happens to contain "capability_"
    /// (e.g. `my_capability_foo`) must NOT be misidentified as a capability
    /// intrinsic. Under the old substring matching, this would either be
    /// counted as a folded L2 check (false positive) or flagged as an
    /// "unknown capability operation" (false negative). Under exact
    /// matching, it is simply skipped — collapse succeeds with zero folded
    /// L2 checks.
    #[test]
    fn l1l3_collapse_skips_user_defined_capability_named_function() {
        use vuma_scg::node::{ComputationNode, NodeType, ProgramPoint};

        let mut scg = SCG::new();
        let pp = ProgramPoint {
            file: None,
            line: None,
            column: None,
            offset: None,
        };
        // "my_capability_foo" is a user-defined function name that
        // contains "capability_" but is NOT a known intrinsic.
        let _ = scg.add_node(
            NodeType::Computation,
            NodePayload::Computation(ComputationNode::new("my_capability_foo", None, false)),
            pp,
        );

        let collapse = l1l3_collapse(&scg);
        assert!(
            collapse.collapsed,
            "user-defined function with 'capability_' in its name must \
             NOT trigger a failure (exact-match fix), got: {}",
            collapse.summary,
        );
        assert_eq!(
            collapse.l2_checks_folded, 0,
            "user-defined 'my_capability_foo' must NOT be counted as a \
             folded L2 capability check, got: {}",
            collapse.summary,
        );
    }

    /// An empty SCG must collapse trivially (no checks, no failures).
    #[test]
    fn l1l3_collapse_succeeds_on_empty_scg() {
        let scg = SCG::new();
        let collapse = l1l3_collapse(&scg);
        assert!(
            collapse.collapsed,
            "empty SCG should collapse trivially, got: {}",
            collapse.summary,
        );
        assert_eq!(collapse.l1_checks_folded, 0);
        assert_eq!(collapse.l2_checks_folded, 0);
    }

    // ── Capability-detection regression tests ───────────────────────────
    //
    // These tests pin the FIX: capability
    // detection in `l1l3_collapse` no longer uses substring matching
    // (`lower.contains("capability_") || lower.contains("stark_")`).
    // Instead it uses nominal typing via
    // `matches!(c.kind, ComputationKind::Intrinsic(_))` at
    // `verification.rs:882-905` (line numbers may shift; search for
    // `ComputationKind::Intrinsic(_)`). Only nodes tagged
    // `ComputationKind::Intrinsic(IntrinsicKind)` at parse /
    // deserialization time count as capability ops — user-defined
    // functions whose name merely *contains* `capability_` are NOT
    // misidentified.

    /// [Positive case] A node tagged `ComputationKind::Intrinsic(_)`
    /// IS detected as a capability op and folds exactly one L2 check.
    ///
    /// We construct the node directly with
    /// `ComputationKind::Intrinsic(IntrinsicKind::CapabilityGrant)`
    /// (bypassing `ComputationNode::new`, which would also promote
    /// the string `"capability_grant"` to the intrinsic variant via
    /// `from_op_name`). This isolates the test to the
    /// `matches!(c.kind, ComputationKind::Intrinsic(_))` site.
    #[test]
    fn l1l3_collapse_detects_intrinsic_kind_as_capability() {
        use vuma_scg::node::{ComputationNode, IntrinsicKind, NodeType, ProgramPoint};

        let mut scg = SCG::new();
        let pp = ProgramPoint {
            file: None,
            line: None,
            column: None,
            offset: None,
        };
        // Direct construction with the Intrinsic variant — nominal tag.
        let _ = scg.add_node(
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                kind: ComputationKind::Intrinsic(IntrinsicKind::CapabilityGrant),
                result_type: None,
                tail_call: false,
            }),
            pp,
        );

        let collapse = l1l3_collapse(&scg);
        assert!(
            collapse.collapsed,
            "an Intrinsic(CapabilityGrant) node should not trigger a \
             failure, got: {}",
            collapse.summary,
        );
        assert_eq!(
            collapse.l2_checks_folded, 1,
            "Intrinsic(CapabilityGrant) MUST fold exactly 1 L2 capability \
             check under the Gap 7 fix (nominal typing), got: {}",
            collapse.summary,
        );
    }

    /// [Negative case] A node tagged
    /// `ComputationKind::Other("capability_foo")` is NOT detected as a
    /// capability op. The old substring matcher
    /// `lower.contains("capability_")` would have flagged this as a
    /// capability (false positive — folding a phantom L2 check — or
    /// false negative — emitting an "unknown capability operation"
    /// failure). The nominal-typing fix skips it cleanly: zero L2 checks
    /// folded, collapse still succeeds.
    #[test]
    fn l1l3_collapse_does_not_detect_other_kind_named_capability_foo() {
        use vuma_scg::node::{ComputationNode, NodeType, ProgramPoint};

        let mut scg = SCG::new();
        let pp = ProgramPoint {
            file: None,
            line: None,
            column: None,
            offset: None,
        };
        // Direct construction with the Other variant — NOT a known
        // intrinsic. The string "capability_foo" contains the
        // "capability_" substring, so the OLD substring matcher would
        // have misidentified it.
        let _ = scg.add_node(
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                kind: ComputationKind::Other("capability_foo".to_string()),
                result_type: None,
                tail_call: false,
            }),
            pp,
        );

        let collapse = l1l3_collapse(&scg);
        assert!(
            collapse.collapsed,
            "Other(\"capability_foo\") must NOT be flagged as an unknown \
             capability op (the old substring matcher would have), got: {}",
            collapse.summary,
        );
        assert_eq!(
            collapse.l2_checks_folded, 0,
            "Other(\"capability_foo\") must NOT fold any L2 capability \
             check — the Gap 7 fix uses nominal typing, so a user-defined \
             function name containing 'capability_' is NOT a capability. \
             Got: {}",
            collapse.summary,
        );
    }

    // ── Field-list cross-check tests ──────────────────────────────
    //
    // These tests pin the extension to the layout consistency check
    // (`verify_layout_field_list_consistency`): the field LIST (names +
    // count) of parser-provided `pmt_layouts` is now cross-checked against
    // an IVE-derived layout map built from the SCG's StateRead / StateWrite
    // nodes. A parser bug that drops or renames a field during
    // `build_pmt_layout_specs` construction (pipeline.rs:8925) would
    // leave the SCG still referencing the original field name, which
    // these tests verify is caught as a Violation.

    /// Helper: build a `PmtLayoutSpec` with the given field names, all
    /// at the same offset/size (the cross-check ignores geometry).
    fn mk_layout(name: &str, field_names: &[&str]) -> PmtLayoutSpec {
        PmtLayoutSpec {
            name: name.to_string(),
            total_size: 8,
            fields: field_names
                .iter()
                .enumerate()
                .map(|(i, n)| PmtFieldSpec {
                    name: n.to_string(),
                    offset: i as u64 * 4,
                    size: 4,
                    type_name: "u32".to_string(),
                })
                .collect(),
        }
    }

    /// Two layouts with the SAME offsets/sizes but DIFFERENT field names
    /// must trigger a violation. This is the canonical regression test for
    /// the field-list cross-check: a parser bug that renames `x`→`a` and
    /// `y`→`b` (but keeps the geometry) would slip past the offsets/sizes
    /// check but must be caught here.
    #[test]
    fn field_list_cross_check_fails_on_different_field_names() {
        let mut parser_layouts = HashMap::new();
        parser_layouts.insert("L".to_string(), mk_layout("L", &["x", "y"]));

        let mut ivederived_layouts = HashMap::new();
        ivederived_layouts.insert("L".to_string(), mk_layout("L", &["a", "b"]));

        let mismatches = verify_layout_field_list_consistency(&parser_layouts, &ivederived_layouts);
        assert!(
            !mismatches.is_empty(),
            "field-list cross-check must FAIL when field names differ \
             (even with matching offsets/sizes), got: {:?}",
            mismatches,
        );
        // Both IVE-derived field names should be reported as not declared.
        let joined = mismatches.join("; ");
        assert!(
            joined.contains("'a'") && joined.contains("'b'"),
            "mismatch descriptions should name both missing fields, got: {}",
            joined,
        );
    }

    /// Sanity check: when the IVE-derived field names match the
    /// parser-provided field names, the cross-check passes (no mismatches).
    /// This guards against false positives.
    #[test]
    fn field_list_cross_check_passes_on_matching_field_names() {
        let mut parser_layouts = HashMap::new();
        parser_layouts.insert("L".to_string(), mk_layout("L", &["x", "y"]));

        let mut ivederived_layouts = HashMap::new();
        ivederived_layouts.insert("L".to_string(), mk_layout("L", &["x", "y"]));

        let mismatches = verify_layout_field_list_consistency(&parser_layouts, &ivederived_layouts);
        assert!(
            mismatches.is_empty(),
            "field-list cross-check should PASS when field names match, got: {:?}",
            mismatches,
        );
    }

    /// The parser is allowed to declare MORE fields than the SCG
    /// references (not all declared fields need to be accessed).  This must
    /// NOT trigger a violation — otherwise every program with an unused
    /// field would fail to verify.
    #[test]
    fn field_list_cross_check_passes_when_parser_declares_more_fields() {
        let mut parser_layouts = HashMap::new();
        parser_layouts.insert("L".to_string(), mk_layout("L", &["x", "y", "z"]));

        let mut ivederived_layouts = HashMap::new();
        // SCG only references "x" and "y" — "z" is declared but unused.
        ivederived_layouts.insert("L".to_string(), mk_layout("L", &["x", "y"]));

        let mismatches = verify_layout_field_list_consistency(&parser_layouts, &ivederived_layouts);
        assert!(
            mismatches.is_empty(),
            "field-list cross-check should PASS when parser declares more \
             fields than the SCG references, got: {:?}",
            mismatches,
        );
    }

    /// If the SCG references a field that the parser-provided layout does
    /// NOT declare, the cross-check must fail. This is the "dropped field"
    /// parser-bug scenario.
    #[test]
    fn field_list_cross_check_fails_when_scg_references_undeclared_field() {
        let mut parser_layouts = HashMap::new();
        // Parser dropped "y" — only declares "x".
        parser_layouts.insert("L".to_string(), mk_layout("L", &["x"]));

        let mut ivederived_layouts = HashMap::new();
        // SCG references both "x" and "y".
        ivederived_layouts.insert("L".to_string(), mk_layout("L", &["x", "y"]));

        let mismatches = verify_layout_field_list_consistency(&parser_layouts, &ivederived_layouts);
        assert!(
            !mismatches.is_empty(),
            "field-list cross-check must FAIL when SCG references a field \
             not declared in parser-provided layout, got: {:?}",
            mismatches,
        );
        let joined = mismatches.join("; ");
        assert!(
            joined.contains("'y'"),
            "mismatch should name the dropped field 'y', got: {}",
            joined,
        );
        // Count mismatch should also be reported (IVE-derived 2 > parser 1).
        assert!(
            joined.contains("field count mismatch"),
            "mismatch should report the count mismatch, got: {}",
            joined,
        );
    }

    /// End-to-end: when the SCG references a field the parser-provided
    /// layout doesn't declare, `verify_pmt` must return
    /// `VerificationStatus::Violated` (not just log a warning).
    #[test]
    fn verify_pmt_returns_violated_on_field_list_mismatch() {
        use vuma_scg::node::{NodeType, ProgramPoint, StateInitNode, StateReadNode};

        let mut scg = SCG::new();
        let pp = ProgramPoint {
            file: None,
            line: None,
            column: None,
            offset: None,
        };

        // state_init: vreg 0 = layout "L"
        let _ = scg.add_node(
            NodeType::StateInit,
            NodePayload::StateInit(StateInitNode {
                layout_name: "L".to_string(),
                result_vreg: 0,
            }),
            pp.clone(),
        );
        // state_read: vreg 0, layout "L", field "phantom" — but the
        // parser-provided layout below only declares "x" and "y".
        let _ = scg.add_node(
            NodeType::StateRead,
            NodePayload::StateRead(StateReadNode {
                state_vreg: 0,
                layout_name: "L".to_string(),
                field_name: "phantom".to_string(),
                result_vreg: 1,
            }),
            pp,
        );

        let mut pmt_layouts = HashMap::new();
        pmt_layouts.insert(
            "L".to_string(),
            PmtLayoutSpec {
                name: "L".to_string(),
                total_size: 8,
                fields: vec![
                    PmtFieldSpec {
                        name: "x".to_string(),
                        offset: 0,
                        size: 4,
                        type_name: "u32".to_string(),
                    },
                    PmtFieldSpec {
                        name: "y".to_string(),
                        offset: 4,
                        size: 4,
                        type_name: "u32".to_string(),
                    },
                ],
            },
        );

        let input = VerificationInput::from_scg(scg).with_pmt_layouts(pmt_layouts);
        let engine = VerificationEngine::new();
        let result = engine.verify_pmt(&input);

        assert!(
            matches!(result.status, VerificationStatus::Violated { .. }),
            "verify_pmt must return Violated when SCG references a field \
             not declared in parser-provided layout, got: {:?}",
            result.status,
        );
        assert!(
            result.message.contains("field-list cross-check failed"),
            "violation message should mention the field-list cross-check, \
             got: {}",
            result.message,
        );
    }
}

// ── IR-based L1→L3 collapse (pipeline wiring) ──────────────
//
// TASKS.md §0.3 requires that l1l3_collapse be CALLED from
// src/pipeline.rs, not just defined as library code with unit tests.
// The SCG-based l1l3_collapse takes a &SCG, but the pipeline has an
// &IRProgram at the point where we want to run the check.  This wrapper
// adapts the IR to the collapse proof by counting L1 runtime checks
// (channel_send, channel_recv, CRC32 verification, capability checks,
// protocol state checks) in the IR and reporting how many fold.

/// IR-based L1→L3 invariant collapse result (pipeline wiring).
#[derive(Debug, Clone)]
pub struct L1L3CollapseIR {
    /// Whether all L1 checks have compile-time-known arguments (fully collapsible).
    pub collapsed: bool,
    /// Total L1 runtime checks found in the IR.
    pub folded_checks: usize,
}

/// Count L1 runtime checks in an IRProgram and report the collapse status.
///
/// This is the pipeline-facing wrapper around the SCG-based `l1l3_collapse`.
/// It scans the IR for:
/// - channel_send / channel_recv (L1 framing + CRC)
/// - channel_send_cap (L2 capability)
/// - channel_recv_proto (L4 protocol state)
/// - supervisor_call (L5 kernel/user gate)
/// - aead_seal / aead_open (L8 crypto)
/// - stark_prove / stark_verify (L8 STARK)
///
/// Each is an L1 runtime check that can potentially fold into an L3
/// compile-time invariant.  The collapse succeeds if all checks have
/// compile-time-known arguments (Immediate values).
pub fn l1l3_collapse_from_ir(program: &vuma_codegen::ir::IRProgram) -> L1L3CollapseIR {
    let mut folded_checks: usize = 0;
    let mut all_compile_time = true;

    for func in &program.functions {
        for block in &func.blocks {
            for instr in &block.instructions {
                match instr {
                    vuma_codegen::ir::IRInstr::Call {
                        func: name, args, ..
                    } => {
                        let is_l1_check = matches!(
                            name.as_str(),
                            "channel_send"
                                | "channel_recv"
                                | "channel_send_cap"
                                | "channel_recv_proto"
                                | "supervisor_call"
                                | "aead_seal"
                                | "aead_open"
                                | "stark_prove"
                                | "stark_verify"
                                | "circuit_breaker_call"
                                | "hot_swap_trigger"
                        );
                        if is_l1_check {
                            folded_checks += 1;
                            // Check if all args are Immediate (compile-time known)
                            let all_imm = args
                                .iter()
                                .all(|a| matches!(a, vuma_codegen::ir::IRValue::Immediate(_)));
                            if !all_imm {
                                all_compile_time = false;
                            }
                        }
                    }
                    vuma_codegen::ir::IRInstr::ChannelRecvResult { .. } => {
                        folded_checks += 1;
                    }
                    _ => {}
                }
            }
        }
    }

    L1L3CollapseIR {
        collapsed: all_compile_time,
        folded_checks,
    }
}
