//! Security Model — lattice-based information-flow control, taint tracking,
//! capability-to-hardware mapping, declassification, and boundary enforcement.
//!
//! This module implements the VUMA Security Model as specified in
//! `VUMA-SPEC-SEC-001` (security-model-spec.md). The central invariant is the
//! **no-downgrade rule**: data may flow from a lower security level to an equal
//! or higher level, but *never* downward without explicit declassification.
//!
//! # Architecture
//!
//! | Concept               | Type                     | Description                                    |
//! |-----------------------|--------------------------|------------------------------------------------|
//! | **Security Level**    | [`SecurityLevel`]        | 5-level lattice: Public…TopSecret              |
//! | **Flow Policy**       | [`FlowPolicy`]           | FreeFlow / NoDowngrade / NoFlow                |
//! | **Taint Label**       | [`TaintLabel`]           | Lightweight taint label (source set)           |
//! | **Taint**             | [`TaintStatus`]          | Source-tracking with sanitization              |
//! | **Taint Tracker**     | [`TaintTracker`]         | Propagate taint through derivation chains      |
//! | **Security Relation** | [`SecurityRel`]          | Per-value security metadata (part of RelD)     |
//! | **Boundary**          | [`SecurityBoundary`]     | Region-pair boundary with crossing rules       |
//! | **Declassification**  | [`DeclassificationRecord`]| Audit-trail for every downgrade event         |
//! | **HW Mapping**        | [`Arm64SecurityMapping`] | CapD → PAC/BTI/MTE for Pi 5                   |
//! | **Verifier**          | [`SecurityVerifier`]     | Whole-program invariant checker                |

use crate::derivation::{Derivation, DerivationId};
use crate::program_point::{NodeId, ProgramPoint};
use crate::region::RegionId;
use hashbrown::{HashMap, HashSet};
use serde::{Deserialize, Serialize};
use std::fmt;

// ---------------------------------------------------------------------------
// Security Level Lattice
// ---------------------------------------------------------------------------

/// The five security levels in VUMA's mandatory access control lattice.
///
/// The ordering is total: `Public < Internal < Confidential < Secret < TopSecret`.
///
/// The pair `(L, ≤)` forms a lattice with:
/// - **Join** (lub) = `max(l1, l2)` — the more restrictive level.
/// - **Meet** (glb) = `min(l1, l2)` — the less restrictive level.
/// - **Top** = `TopSecret`, **Bottom** = `Public`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, PartialOrd, Ord)]
pub enum SecurityLevel {
    /// Bottom element — unrestricted, openly observable.
    Public = 0,
    /// Internal to the organisation / process — not for external release.
    Internal = 1,
    /// Confidential — restricted to authorised personnel.
    Confidential = 2,
    /// Secret — highly restricted, significant damage if disclosed.
    Secret = 3,
    /// Top element — exceptionally grave damage if disclosed.
    TopSecret = 4,
}

impl SecurityLevel {
    /// All levels in ascending order.
    pub const ALL: [SecurityLevel; 5] = [
        SecurityLevel::Public,
        SecurityLevel::Internal,
        SecurityLevel::Confidential,
        SecurityLevel::Secret,
        SecurityLevel::TopSecret,
    ];

    /// The bottom element of the lattice.
    pub const BOTTOM: SecurityLevel = SecurityLevel::Public;

    /// The top element of the lattice.
    pub const TOP: SecurityLevel = SecurityLevel::TopSecret;

    /// **Join** (least upper bound) — returns the more restrictive of two levels.
    ///
    /// ```
    /// use vuma_core::security::SecurityLevel;
    /// assert_eq!(SecurityLevel::Public.join(SecurityLevel::Secret), SecurityLevel::Secret);
    /// ```
    pub fn join(self, other: SecurityLevel) -> SecurityLevel {
        self.max(other)
    }

    /// **Meet** (greatest lower bound) — returns the less restrictive of two levels.
    ///
    /// ```
    /// use vuma_core::security::SecurityLevel;
    /// assert_eq!(SecurityLevel::Confidential.meet(SecurityLevel::Secret), SecurityLevel::Confidential);
    /// ```
    pub fn meet(self, other: SecurityLevel) -> SecurityLevel {
        self.min(other)
    }

    /// Returns `true` if information may flow from `self` to `target`
    /// (i.e. `self ≤ target`).
    pub fn can_flow_to(self, target: SecurityLevel) -> bool {
        self <= target
    }

    /// Numeric rank (0..4) useful for array indexing.
    pub fn rank(self) -> u8 {
        self as u8
    }
}

impl fmt::Display for SecurityLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SecurityLevel::Public => write!(f, "Public"),
            SecurityLevel::Internal => write!(f, "Internal"),
            SecurityLevel::Confidential => write!(f, "Confidential"),
            SecurityLevel::Secret => write!(f, "Secret"),
            SecurityLevel::TopSecret => write!(f, "TopSecret"),
        }
    }
}

// ---------------------------------------------------------------------------
// Flow Policy
// ---------------------------------------------------------------------------

/// The permissible information-flow direction for a value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum FlowPolicy {
    /// Unrestricted movement in any direction.
    FreeFlow,
    /// Enforces the no-write-down rule (default for most data).
    NoDowngrade,
    /// Data is statically unmovable across certain boundaries
    /// (e.g. cryptographic key material).
    NoFlow,
}

impl fmt::Display for FlowPolicy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FlowPolicy::FreeFlow => write!(f, "FreeFlow"),
            FlowPolicy::NoDowngrade => write!(f, "NoDowngrade"),
            FlowPolicy::NoFlow => write!(f, "NoFlow"),
        }
    }
}

// ---------------------------------------------------------------------------
// Taint Tracking
// ---------------------------------------------------------------------------

/// A source of taint — untrusted origin that data may come from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TaintSource {
    /// User input (stdin, CLI args, env vars, GUI events).
    UserInput,
    /// Data received from any network socket.
    Network,
    /// Data read from filesystem paths writable by untrusted principals.
    UntrustedFile,
}

impl fmt::Display for TaintSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TaintSource::UserInput => write!(f, "UserInput"),
            TaintSource::Network => write!(f, "Network"),
            TaintSource::UntrustedFile => write!(f, "UntrustedFile"),
        }
    }
}

/// The taint status of a value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaintStatus {
    /// Value is not tainted.
    Clean,
    /// Value is tainted with the given set of sources.
    Tainted {
        /// The set of taint sources this value carries.
        sources: HashSet<TaintSource>,
        /// Whether the taint is sanitizable (can be removed via verified
        /// sanitization).
        sanitizable: bool,
    },
}

impl TaintStatus {
    /// Create a clean (non-tainted) status.
    pub fn clean() -> Self {
        TaintStatus::Clean
    }

    /// Create a tainted status from a single source.
    pub fn tainted(source: TaintSource, sanitizable: bool) -> Self {
        TaintStatus::Tainted {
            sources: [source].into_iter().collect(),
            sanitizable,
        }
    }

    /// Create a tainted status from multiple sources.
    pub fn tainted_multi(sources: HashSet<TaintSource>, sanitizable: bool) -> Self {
        TaintStatus::Tainted {
            sources,
            sanitizable,
        }
    }

    /// Returns `true` if the value is tainted.
    pub fn is_tainted(&self) -> bool {
        matches!(self, TaintStatus::Tainted { .. })
    }

    /// Returns the taint sources, or an empty set if clean.
    pub fn sources(&self) -> HashSet<TaintSource> {
        match self {
            TaintStatus::Clean => HashSet::new(),
            TaintStatus::Tainted { sources, .. } => sources.clone(),
        }
    }

    /// Propagate taint: union the sources of two taint statuses.
    ///
    /// If either is tainted, the result is tainted with the union of sources.
    /// The result is sanitizable only if both inputs are sanitizable.
    pub fn propagate(&self, other: &TaintStatus) -> TaintStatus {
        match (self, other) {
            (TaintStatus::Clean, TaintStatus::Clean) => TaintStatus::Clean,
            (TaintStatus::Clean, TaintStatus::Tainted { sources, sanitizable })
            | (TaintStatus::Tainted { sources, sanitizable }, TaintStatus::Clean) => {
                TaintStatus::Tainted {
                    sources: sources.clone(),
                    sanitizable: *sanitizable,
                }
            }
            (
                TaintStatus::Tainted {
                    sources: s1,
                    sanitizable: san1,
                },
                TaintStatus::Tainted {
                    sources: s2,
                    sanitizable: san2,
                },
            ) => TaintStatus::Tainted {
                sources: s1.union(s2).copied().collect(),
                sanitizable: *san1 && *san2,
            },
        }
    }

    /// Sanitize the value: remove taint if sanitizable.
    ///
    /// Returns `Ok(Clean)` if the taint was sanitizable, or `Err(self)` if not.
    pub fn sanitize(self) -> Result<TaintStatus, TaintStatus> {
        match self {
            TaintStatus::Clean => Ok(TaintStatus::Clean),
            TaintStatus::Tainted { sanitizable: true, .. } => Ok(TaintStatus::Clean),
            t @ TaintStatus::Tainted { sanitizable: false, .. } => Err(t),
        }
    }

    /// Effective security level adjustment for tainted data.
    /// Tainted data is treated at the join of its explicit level and `Internal`.
    pub fn effective_level(&self, explicit: SecurityLevel) -> SecurityLevel {
        match self {
            TaintStatus::Clean => explicit,
            TaintStatus::Tainted { .. } => explicit.join(SecurityLevel::Internal),
        }
    }
}

impl fmt::Display for TaintStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TaintStatus::Clean => write!(f, "Clean"),
            TaintStatus::Tainted { sources, sanitizable } => {
                let srcs: Vec<_> = sources.iter().map(|s| s.to_string()).collect();
                write!(
                    f,
                    "Tainted({}{})",
                    srcs.join(","),
                    if *sanitizable { "" } else { ";unsanitizable" }
                )
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Taint Label
// ---------------------------------------------------------------------------

/// A lightweight taint label — the set of taint sources attached to a value.
///
/// Unlike [`TaintStatus`], which also tracks sanitizability, `TaintLabel`
/// is the minimal information needed for taint propagation along data-flow
/// edges. It is the label that flows through the SCG's DataFlow edges
/// during the IVE's fixed-point taint computation.
///
/// A `TaintLabel` is essentially a set of [`TaintSource`] values. The empty
/// set represents "Clean" (no taint). Taint propagation is set union.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaintLabel {
    /// The set of taint sources. Empty means clean.
    sources: HashSet<TaintSource>,
}

impl TaintLabel {
    /// Create a clean (empty) label.
    pub fn clean() -> Self {
        TaintLabel {
            sources: HashSet::new(),
        }
    }

    /// Create a label from a single source.
    pub fn from_source(source: TaintSource) -> Self {
        TaintLabel {
            sources: [source].into_iter().collect(),
        }
    }

    /// Create a label from multiple sources.
    pub fn from_sources(sources: HashSet<TaintSource>) -> Self {
        TaintLabel { sources }
    }

    /// Returns `true` if the label is clean (no taint sources).
    pub fn is_clean(&self) -> bool {
        self.sources.is_empty()
    }

    /// Returns `true` if the label is tainted (has at least one source).
    pub fn is_tainted(&self) -> bool {
        !self.sources.is_empty()
    }

    /// Returns the set of taint sources.
    pub fn sources(&self) -> &HashSet<TaintSource> {
        &self.sources
    }

    /// Propagate (join) two labels: the result carries the union of sources.
    ///
    /// This is the lattice join operation for taint labels.
    pub fn join(&self, other: &TaintLabel) -> TaintLabel {
        TaintLabel {
            sources: self.sources.union(&other.sources).copied().collect(),
        }
    }

    /// Check if this label contains a specific source.
    pub fn contains(&self, source: &TaintSource) -> bool {
        self.sources.contains(source)
    }

    /// Convert to a `TaintStatus` with the given sanitizability.
    pub fn to_status(&self, sanitizable: bool) -> TaintStatus {
        if self.sources.is_empty() {
            TaintStatus::Clean
        } else {
            TaintStatus::Tainted {
                sources: self.sources.clone(),
                sanitizable,
            }
        }
    }
}

impl fmt::Display for TaintLabel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.sources.is_empty() {
            write!(f, "Clean")
        } else {
            let srcs: Vec<_> = self.sources.iter().map(|s| s.to_string()).collect();
            write!(f, "Tainted({})", srcs.join(","))
        }
    }
}

impl Default for TaintLabel {
    fn default() -> Self {
        Self::clean()
    }
}

// ---------------------------------------------------------------------------
// TaintTracker
// ---------------------------------------------------------------------------

/// A taint tracker that propagates taint labels through a graph of value
/// nodes connected by derivation chains.
///
/// The tracker maintains a map from [`NodeId`] to [`TaintLabel`] and
/// supports incremental propagation: when a new edge is added, only the
/// affected downstream nodes are updated.
///
/// # Fixed-Point Propagation
///
/// The `propagate` method performs a fixed-point computation: it iterates
/// over all edges, joining source labels into destination labels, until no
/// labels change. This is equivalent to the IVE's taint propagation over
/// the SCG's DataFlow edges.
#[derive(Debug, Clone, Default)]
pub struct TaintTracker {
    /// Taint labels indexed by node ID.
    labels: HashMap<NodeId, TaintLabel>,
    /// Data-flow edges: (source, destination).
    edges: Vec<(NodeId, NodeId)>,
}

impl TaintTracker {
    /// Create a new, empty taint tracker.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the taint label for a node.
    pub fn set_label(&mut self, node: NodeId, label: TaintLabel) {
        self.labels.insert(node, label);
    }

    /// Get the taint label for a node (Clean if not set).
    pub fn get_label(&self, node: NodeId) -> TaintLabel {
        self.labels.get(&node).cloned().unwrap_or_default()
    }

    /// Add a data-flow edge from `src` to `dst`.
    pub fn add_edge(&mut self, src: NodeId, dst: NodeId) {
        self.edges.push((src, dst));
    }

    /// Propagate taint labels along all edges until a fixed point is reached.
    ///
    /// Returns the number of iterations performed.
    pub fn propagate(&mut self) -> usize {
        let mut iterations = 0;
        loop {
            let mut changed = false;
            for &(src, dst) in &self.edges {
                let src_label = self.get_label(src);
                let dst_label = self.get_label(dst);
                let joined = src_label.join(&dst_label);
                if joined != dst_label {
                    self.labels.insert(dst, joined);
                    changed = true;
                }
            }
            iterations += 1;
            if !changed {
                break;
            }
        }
        iterations
    }

    /// Propagate taint through a derivation chain.
    ///
    /// Given a chain of derivations and a taint map, walks the chain from
    /// root to leaf, joining all taint labels.
    pub fn propagate_chain(
        derivation: &Derivation,
        taint_map: &HashMap<DerivationId, TaintLabel>,
        lookup: impl Fn(DerivationId) -> Option<Derivation>,
    ) -> TaintLabel {
        let chain = derivation.trace(lookup);
        let mut accumulated = TaintLabel::clean();
        for d in &chain {
            if let Some(label) = taint_map.get(&d.id) {
                accumulated = accumulated.join(label);
            }
        }
        accumulated
    }

    /// Returns the number of nodes with taint labels.
    pub fn node_count(&self) -> usize {
        self.labels.len()
    }

    /// Returns the number of edges.
    pub fn edge_count(&self) -> usize {
        self.edges.len()
    }

    /// Returns all tainted nodes (nodes with non-empty taint labels).
    pub fn tainted_nodes(&self) -> Vec<(NodeId, TaintLabel)> {
        self.labels
            .iter()
            .filter(|(_, label)| label.is_tainted())
            .map(|(&id, label)| (id, label.clone()))
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Capability (VUMA-level, mirrors bd::capd::Capability)
// --------------------------------------------------------------------------- 

/// VUMA capability bits used for the security model.
///
/// This is the subset of the full capability set that is relevant to
/// security-level and hardware-mapping decisions. The full set lives in
/// `bd::capd::Capability`; we duplicate the relevant variants here to keep
/// the security module self-contained and avoid a cross-crate dependency.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SecurityCapability {
    /// Permission to observe the value's content.
    Read,
    /// Permission to modify the value's content.
    Write,
    /// Permission to transmit the value over a network channel.
    Send,
    /// Permission to call the value as code.
    Execute,
    /// Permission to derive a pointer to the value.
    DerivePtr,
}

impl fmt::Display for SecurityCapability {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SecurityCapability::Read => write!(f, "Read"),
            SecurityCapability::Write => write!(f, "Write"),
            SecurityCapability::Send => write!(f, "Send"),
            SecurityCapability::Execute => write!(f, "Execute"),
            SecurityCapability::DerivePtr => write!(f, "DerivePtr"),
        }
    }
}

// ---------------------------------------------------------------------------
// Security Relation (part of RelD)
// ---------------------------------------------------------------------------

/// The security metadata carried by every value as part of its Relational
/// Descriptor (RelD).
///
/// This is the core data structure of the security model. It records the
/// security classification, flow policy, taint status, and (if applicable)
/// the provenance of any declassification that has been applied.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecurityRel {
    /// The security classification of this value.
    pub level: SecurityLevel,
    /// The flow policy governing information flow.
    pub flow: FlowPolicy,
    /// The taint status (taint sources, sanitizability).
    pub taint: TaintStatus,
    /// If this value has been declassified, records the audit trail.
    pub declassification: Option<DeclassificationRecord>,
}

impl SecurityRel {
    /// Create a default SecurityRel at the given level with NoDowngrade flow
    /// and Clean taint.
    pub fn at(level: SecurityLevel) -> Self {
        Self {
            level,
            flow: FlowPolicy::NoDowngrade,
            taint: TaintStatus::Clean,
            declassification: None,
        }
    }

    /// Create a SecurityRel for untrusted (tainted) input.
    ///
    /// Tainted data is created at the given explicit level with `NoDowngrade`
    /// flow. The effective level is the join of the explicit level and
    /// `Internal`.
    pub fn for_untrusted(level: SecurityLevel, source: TaintSource) -> Self {
        Self {
            level,
            flow: FlowPolicy::NoDowngrade,
            taint: TaintStatus::tainted(source, true),
            declassification: None,
        }
    }

    /// Create a SecurityRel for cryptographic key material (NoFlow).
    pub fn for_key_material(level: SecurityLevel) -> Self {
        Self {
            level,
            flow: FlowPolicy::NoFlow,
            taint: TaintStatus::Clean,
            declassification: None,
        }
    }

    /// The effective security level, accounting for taint.
    ///
    /// Tainted data is treated at the join of its explicit level and Internal.
    pub fn effective_level(&self) -> SecurityLevel {
        self.taint.effective_level(self.level)
    }

    /// Check whether information can flow from `self` to `target`.
    ///
    /// Returns `Ok(())` if the flow is permitted, or a `SecurityViolation`
    /// describing why it is not.
    pub fn check_flow_to(&self, target: &SecurityRel) -> Result<(), SecurityViolation> {
        // NoFlow values cannot flow at all.
        if self.flow == FlowPolicy::NoFlow {
            return Err(SecurityViolation::NoFlowViolation {
                src_level: self.level,
                dst_level: target.level,
            });
        }

        // Check the no-downgrade rule using effective levels.
        let src_eff = self.effective_level();
        let dst_eff = target.effective_level();

        if !src_eff.can_flow_to(dst_eff) {
            // Allow if the target has a declassification gate that covers
            // this exact downgrade — but that check is done by the
            // SecurityVerifier at the boundary level, not here.
            return Err(SecurityViolation::InformationLeak {
                src_level: src_eff,
                dst_level: dst_eff,
            });
        }

        // NoDowngrade policy on the source blocks any downward flow
        // even if the target level would otherwise allow it.
        if self.flow == FlowPolicy::NoDowngrade && self.level > target.level {
            return Err(SecurityViolation::DowngradeBlocked {
                src_level: self.level,
                dst_level: target.level,
            });
        }

        Ok(())
    }

    /// Join two SecurityRels — used when combining values (e.g. arithmetic).
    ///
    /// The result has the join of the levels, the most restrictive flow policy,
    /// and the union of taint sources.
    pub fn join(&self, other: &SecurityRel) -> SecurityRel {
        SecurityRel {
            level: self.level.join(other.level),
            flow: self.flow.more_restrictive(other.flow),
            taint: self.taint.propagate(&other.taint),
            declassification: None,
        }
    }
}

impl fmt::Display for SecurityRel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SecurityRel({} {} {}", self.level, self.flow, self.taint)?;
        if let Some(ref dec) = self.declassification {
            write!(f, " declassified_by={}", dec.gate_function)?;
        }
        write!(f, ")")
    }
}

// ---------------------------------------------------------------------------
// Flow Policy helpers
// ---------------------------------------------------------------------------

impl FlowPolicy {
    /// Returns the more restrictive of two flow policies.
    ///
    /// Ordering: `NoFlow > NoDowngrade > FreeFlow`.
    pub fn more_restrictive(self, other: FlowPolicy) -> FlowPolicy {
        match (self, other) {
            (FlowPolicy::NoFlow, _) | (_, FlowPolicy::NoFlow) => FlowPolicy::NoFlow,
            (FlowPolicy::NoDowngrade, _) | (_, FlowPolicy::NoDowngrade) => FlowPolicy::NoDowngrade,
            (FlowPolicy::FreeFlow, FlowPolicy::FreeFlow) => FlowPolicy::FreeFlow,
        }
    }
}

// ---------------------------------------------------------------------------
// Security Violations
// ---------------------------------------------------------------------------

/// A violation of the security model detected during verification.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SecurityViolation {
    /// Information would leak from a higher level to a lower level.
    InformationLeak {
        src_level: SecurityLevel,
        dst_level: SecurityLevel,
    },
    /// A NoDowngrade policy prevents the flow.
    DowngradeBlocked {
        src_level: SecurityLevel,
        dst_level: SecurityLevel,
    },
    /// A NoFlow value attempted to cross a boundary.
    NoFlowViolation {
        src_level: SecurityLevel,
        dst_level: SecurityLevel,
    },
    /// Tainted data reached a sink that requires clean data.
    TaintedDataAtSink {
        sources: HashSet<TaintSource>,
        sink: String,
    },
    /// A boundary crossing was attempted without the required capabilities.
    MissingCapabilityForCrossing {
        capability: SecurityCapability,
        boundary_id: BoundaryId,
    },
    /// A declassification was attempted without a valid proof.
    DeclassificationWithoutProof {
        from_level: SecurityLevel,
        to_level: SecurityLevel,
    },
    /// Capability monotonicity violated — a capability was added.
    CapabilityMonotonicityViolation {
        added: SecurityCapability,
    },
    /// An untrusted source value carries the Execute capability.
    ExecuteOnUntrusted {
        source: TaintSource,
    },
    /// Implicit flow across a boundary.
    ImplicitFlowAcrossBoundary {
        condition_level: SecurityLevel,
        target_level: SecurityLevel,
    },
}

impl fmt::Display for SecurityViolation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SecurityViolation::InformationLeak { src_level, dst_level } => {
                write!(f, "information leak: {} → {}", src_level, dst_level)
            }
            SecurityViolation::DowngradeBlocked { src_level, dst_level } => {
                write!(f, "downgrade blocked: {} → {}", src_level, dst_level)
            }
            SecurityViolation::NoFlowViolation { src_level, dst_level } => {
                write!(f, "NoFlow violation: {} → {}", src_level, dst_level)
            }
            SecurityViolation::TaintedDataAtSink { sources, sink } => {
                let srcs: Vec<_> = sources.iter().map(|s| s.to_string()).collect();
                write!(f, "tainted data ({}) at sink '{}'", srcs.join(","), sink)
            }
            SecurityViolation::MissingCapabilityForCrossing { capability, boundary_id } => {
                write!(f, "missing {} for crossing boundary {}", capability, boundary_id)
            }
            SecurityViolation::DeclassificationWithoutProof { from_level, to_level } => {
                write!(f, "declassification without proof: {} → {}", from_level, to_level)
            }
            SecurityViolation::CapabilityMonotonicityViolation { added } => {
                write!(f, "capability monotonicity violated: {} added", added)
            }
            SecurityViolation::ExecuteOnUntrusted { source } => {
                write!(f, "Execute capability on untrusted source: {}", source)
            }
            SecurityViolation::ImplicitFlowAcrossBoundary { condition_level, target_level } => {
                write!(
                    f,
                    "implicit flow across boundary: condition at {} influences {}",
                    condition_level, target_level
                )
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Declassification
// ---------------------------------------------------------------------------

/// Opaque identifier for a declassification gate function.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct GateFunctionId(pub u64);

impl fmt::Display for GateFunctionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "gate#{}", self.0)
    }
}

/// A proof that a declassification is safe.
///
/// A declassification is only valid if:
/// 1. It is performed through a verified gate function.
/// 2. The gate function has been verified to produce output safe at the
///    target level (output independence, no side channels, completeness).
/// 3. The gate is the designated gate for the boundary being crossed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeclassificationProof {
    /// The gate function that performs the declassification.
    pub gate: GateFunctionId,
    /// The source security level being declassified from.
    pub from_level: SecurityLevel,
    /// The target security level being declassified to.
    pub to_level: SecurityLevel,
    /// Whether the gate function has been verified for output independence.
    pub output_independence_verified: bool,
    /// Whether the gate function has been verified for no side channels.
    pub no_side_channels_verified: bool,
    /// Whether the gate function has been verified for completeness.
    pub completeness_verified: bool,
    /// The boundary this declassification applies to (if any).
    pub boundary_id: Option<BoundaryId>,
}

impl DeclassificationProof {
    /// Create a new declassification proof.
    pub fn new(gate: GateFunctionId, from: SecurityLevel, to: SecurityLevel) -> Self {
        Self {
            gate,
            from_level: from,
            to_level: to,
            output_independence_verified: false,
            no_side_channels_verified: false,
            completeness_verified: false,
            boundary_id: None,
        }
    }

    /// Returns `true` if all proof obligations are satisfied.
    pub fn is_valid(&self) -> bool {
        self.output_independence_verified
            && self.no_side_channels_verified
            && self.completeness_verified
            && self.from_level > self.to_level
    }

    /// Mark all proof obligations as verified.
    pub fn verify_all(&mut self) {
        self.output_independence_verified = true;
        self.no_side_channels_verified = true;
        self.completeness_verified = true;
    }
}

/// An audit-trail record for a declassification event.
///
/// Every declassification is logged at runtime, enabling post-incident
/// analysis of information leaks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeclassificationRecord {
    /// The gate function that performed the declassification.
    pub gate_function: GateFunctionId,
    /// The original security level.
    pub from_level: SecurityLevel,
    /// The new (lower) security level.
    pub to_level: SecurityLevel,
    /// Where in the source code the declassification occurred.
    pub source_location: ProgramPoint,
    /// The proof that was validated before declassification.
    pub proof: DeclassificationProof,
}

// ---------------------------------------------------------------------------
// Security Boundary
// ---------------------------------------------------------------------------

/// Opaque identifier for a security boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct BoundaryId(pub u64);

impl fmt::Display for BoundaryId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "boundary#{}", self.0)
    }
}

/// A security boundary between two adjacent SCG regions.
///
/// A boundary `B = (R_high, R_low)` enforces that data and control flow
/// crossing from the higher-level region to the lower-level region must
/// satisfy the information-flow rule or go through a declassification gate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecurityBoundary {
    /// Unique identifier.
    pub id: BoundaryId,
    /// The region with the higher security level.
    pub region_high: RegionId,
    /// The region with the lower security level.
    pub region_low: RegionId,
    /// The security level of the high region.
    pub level_high: SecurityLevel,
    /// The security level of the low region.
    pub level_low: SecurityLevel,
    /// Capabilities required for data to cross this boundary.
    pub cross_permissions: HashSet<SecurityCapability>,
    /// The declassification gate for this boundary, if any.
    pub declassification_gate: Option<GateFunctionId>,
}

impl SecurityBoundary {
    /// Create a new security boundary.
    ///
    /// # Panics
    ///
    /// In debug builds, panics if `level_high <= level_low`.
    pub fn new(
        id: BoundaryId,
        region_high: RegionId,
        region_low: RegionId,
        level_high: SecurityLevel,
        level_low: SecurityLevel,
    ) -> Self {
        debug_assert!(
            level_high > level_low,
            "SecurityBoundary requires level_high > level_low"
        );
        Self {
            id,
            region_high,
            region_low,
            level_high,
            level_low,
            cross_permissions: HashSet::new(),
            declassification_gate: None,
        }
    }

    /// Check whether a read from `src_region` to `dst_region` is permitted.
    ///
    /// - High→Low: level must be ≤ level_low, or declassification gate required.
    /// - Low→High: always permitted (information flows upward).
    pub fn check_read_across(
        &self,
        src_region: RegionId,
        value_level: SecurityLevel,
    ) -> Result<(), SecurityViolation> {
        if src_region == self.region_high {
            // High → Low read: information flows downward
            if value_level.can_flow_to(self.level_low) {
                Ok(())
            } else if self.declassification_gate.is_some() {
                Ok(()) // gate will be checked at declassification time
            } else {
                Err(SecurityViolation::InformationLeak {
                    src_level: value_level,
                    dst_level: self.level_low,
                })
            }
        } else {
            // Low → High: always permitted
            Ok(())
        }
    }

    /// Check whether a write from `src_region` to `dst_region` is permitted.
    ///
    /// - High→Low: integrity concern — data is being written into low region.
    /// - Low→High: integrity concern — untrusted data injected into high region.
    pub fn check_write_across(
        &self,
        src_region: RegionId,
        value_level: SecurityLevel,
    ) -> Result<(), SecurityViolation> {
        if src_region == self.region_high {
            // High → Low write
            if value_level.can_flow_to(self.level_low) {
                Ok(())
            } else {
                Err(SecurityViolation::InformationLeak {
                    src_level: value_level,
                    dst_level: self.level_low,
                })
            }
        } else {
            // Low → High write: potential integrity violation
            // In VUMA this is flagged unless the target CapD includes Write
            // and the source value has been validated. For the model-level
            // check we just verify the level is appropriate.
            Ok(())
        }
    }

    /// Check whether a control-flow crossing requires specific capabilities.
    pub fn check_control_flow_across(
        &self,
        caller_caps: &HashSet<SecurityCapability>,
    ) -> Result<(), SecurityViolation> {
        for cap in &self.cross_permissions {
            if !caller_caps.contains(cap) {
                return Err(SecurityViolation::MissingCapabilityForCrossing {
                    capability: *cap,
                    boundary_id: self.id,
                });
            }
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// ARM64 Security Mapping (Pi 5)
// ---------------------------------------------------------------------------

/// ARM64 PAC key identifiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PacKey {
    /// APIAKey — for instruction addresses (function pointers).
    ApiA,
    /// APDAKey — for data addresses.
    ApdA,
}

/// ARM64 MTE operating mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MteMode {
    /// Synchronous tag checks (precise faults, used in dev/test).
    Synchronous,
    /// Asynchronous tag checks (imprecise faults, used in production).
    Asynchronous,
}

/// ARM64 BTI landing pad type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum BtiType {
    /// `bti c` — permits indirect calls.
    Call,
    /// `bti j` — permits indirect jumps.
    Jump,
    /// `bti jc` — permits both calls and jumps.
    CallAndJump,
}

/// The mapping from VUMA CapD capabilities to ARM64 hardware security
/// features for the Raspberry Pi 5 (BCM2712, Cortex-A76, ARMv8.2-A).
///
/// | VUMA Concept              | ARM64 Feature | Mapping                                  |
/// |---------------------------|---------------|------------------------------------------|
/// | DerivePtr capability      | PAC           | Pointer creation → sign; deref → verify  |
/// | Execute capability        | BTI           | Function entries → BTI landing pads      |
/// | Bounds invariant          | MTE           | Allocation tags prevent spatial overflow  |
/// | Liveness invariant        | MTE           | Deallocation retagging prevents UAF      |
/// | Capability monotonicity   | PAC + BTI     | Can't forge pointers or redirect exec    |
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Arm64SecurityMapping {
    /// The PAC key used for instruction (code) pointer signing.
    pub pac_instruction_key: PacKey,
    /// The PAC key used for data pointer signing.
    pub pac_data_key: PacKey,
    /// The MTE mode (synchronous for dev, asynchronous for prod).
    pub mte_mode: MteMode,
    /// Whether PAC is enabled.
    pub pac_enabled: bool,
    /// Whether BTI is enabled.
    pub bti_enabled: bool,
    /// Whether MTE is enabled.
    pub mte_enabled: bool,
}

impl Arm64SecurityMapping {
    /// Default mapping for Pi 5 with all features enabled in development mode.
    pub fn pi5_development() -> Self {
        Self {
            pac_instruction_key: PacKey::ApiA,
            pac_data_key: PacKey::ApdA,
            mte_mode: MteMode::Synchronous,
            pac_enabled: true,
            bti_enabled: true,
            mte_enabled: true,
        }
    }

    /// Mapping for Pi 5 in production mode (async MTE for performance).
    pub fn pi5_production() -> Self {
        Self {
            pac_instruction_key: PacKey::ApiA,
            pac_data_key: PacKey::ApdA,
            mte_mode: MteMode::Asynchronous,
            pac_enabled: true,
            bti_enabled: true,
            mte_enabled: true,
        }
    }

    /// Mapping with all hardware features disabled (testing / emulation).
    pub fn disabled() -> Self {
        Self {
            pac_instruction_key: PacKey::ApiA,
            pac_data_key: PacKey::ApdA,
            mte_mode: MteMode::Synchronous,
            pac_enabled: false,
            bti_enabled: false,
            mte_enabled: false,
        }
    }

    /// Map a capability to the corresponding ARM64 feature.
    ///
    /// Returns the hardware feature(s) that enforce the given VUMA capability.
    pub fn capability_to_hw(&self, cap: SecurityCapability) -> Vec<Arm64Feature> {
        match cap {
            SecurityCapability::DerivePtr => {
                if self.pac_enabled {
                    vec![Arm64Feature::Pac]
                } else {
                    vec![]
                }
            }
            SecurityCapability::Execute => {
                if self.bti_enabled {
                    vec![Arm64Feature::Bti]
                } else {
                    vec![]
                }
            }
            SecurityCapability::Read | SecurityCapability::Write => {
                if self.mte_enabled {
                    vec![Arm64Feature::Mte]
                } else {
                    vec![]
                }
            }
            SecurityCapability::Send => {
                // Send is enforced at the software level; PAC+BTI provide
                // defense-in-depth against bypass.
                let mut features = vec![];
                if self.pac_enabled {
                    features.push(Arm64Feature::Pac);
                }
                if self.bti_enabled {
                    features.push(Arm64Feature::Bti);
                }
                features
            }
        }
    }

    /// Map a capability set to the full set of ARM64 features required.
    pub fn capabilities_to_hw(
        &self,
        caps: &HashSet<SecurityCapability>,
    ) -> HashSet<Arm64Feature> {
        caps.iter()
            .flat_map(|c| self.capability_to_hw(*c))
            .collect()
    }

    /// Generate the PAC signing pseudocode for pointer creation.
    pub fn emit_pac_sign(&self, has_derive_ptr: bool) -> &'static str {
        if self.pac_enabled && has_derive_ptr {
            "let signed_ptr = pac_sign(ptr, context=fp);"
        } else if !has_derive_ptr {
            "compile_error!(\"missing DerivePtr capability\");"
        } else {
            "// PAC disabled; no signing"
        }
    }

    /// Generate the PAC verification pseudocode for pointer dereference.
    pub fn emit_pac_verify(&self) -> &'static str {
        if self.pac_enabled {
            "let verified_ptr = pac_verify(ptr, context=fp);"
        } else {
            "// PAC disabled; no verification"
        }
    }

    /// Generate the BTI landing pad pseudocode for a function entry.
    pub fn emit_bti_landing_pad(&self, has_execute: bool) -> &'static str {
        if self.bti_enabled && has_execute {
            "bti c  // permit indirect calls"
        } else if self.bti_enabled {
            "bti j  // permit indirect jumps only"
        } else {
            "// BTI disabled; no landing pad"
        }
    }

    /// Generate the MTE allocation pseudocode.
    pub fn emit_mte_alloc(&self) -> &'static str {
        if self.mte_enabled {
            "let tag = random_4bit_tag(); let ptr = mte_alloc(size, tag);"
        } else {
            "// MTE disabled; plain allocation"
        }
    }

    /// Generate the MTE deallocation pseudocode.
    pub fn emit_mte_dealloc(&self) -> &'static str {
        if self.mte_enabled {
            "mte_retag(ptr, random_4bit_tag());"
        } else {
            "// MTE disabled; plain deallocation"
        }
    }
}

/// ARM64 hardware security features.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Arm64Feature {
    /// Pointer Authentication Codes.
    Pac,
    /// Branch Target Identification.
    Bti,
    /// Memory Tagging Extension.
    Mte,
}

impl fmt::Display for Arm64Feature {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Arm64Feature::Pac => write!(f, "PAC"),
            Arm64Feature::Bti => write!(f, "BTI"),
            Arm64Feature::Mte => write!(f, "MTE"),
        }
    }
}

// ---------------------------------------------------------------------------
// Taint Tracking Through Derivation Chains
// ---------------------------------------------------------------------------

/// Taint propagation along a derivation chain.
///
/// Given a derivation and a lookup function, this walks the chain from the
/// current derivation back to the root region, propagating taint forward.
/// If any derivation in the chain carries taint, the result is tainted with
/// the union of all sources.
pub fn propagate_taint_through_chain<F>(
    derivation: &Derivation,
    taint_map: &HashMap<DerivationId, TaintStatus>,
    lookup: F,
) -> TaintStatus
where
    F: Fn(DerivationId) -> Option<Derivation>,
{
    let chain = derivation.trace(lookup);
    let mut accumulated = TaintStatus::Clean;

    for d in &chain {
        if let Some(taint) = taint_map.get(&d.id) {
            accumulated = accumulated.propagate(taint);
        }
    }

    accumulated
}

// ---------------------------------------------------------------------------
// Security Verifier
// ---------------------------------------------------------------------------

/// Result of a security verification pass.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerificationResult {
    /// Total number of checks performed.
    pub total_checks: usize,
    /// Number of checks that passed.
    pub passed: usize,
    /// Violations detected.
    pub violations: Vec<SecurityViolation>,
}

impl VerificationResult {
    /// Create an empty result.
    pub fn new() -> Self {
        Self {
            total_checks: 0,
            passed: 0,
            violations: Vec::new(),
        }
    }

    /// Record a passing check.
    pub fn pass(&mut self) {
        self.total_checks += 1;
        self.passed += 1;
    }

    /// Record a failing check with the given violation.
    pub fn fail(&mut self, violation: SecurityViolation) {
        self.total_checks += 1;
        self.violations.push(violation);
    }

    /// Returns `true` if all checks passed (no violations).
    pub fn all_passed(&self) -> bool {
        self.violations.is_empty()
    }
}

impl fmt::Display for VerificationResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "VerificationResult({}/{} passed, {} violations)",
            self.passed,
            self.total_checks,
            self.violations.len()
        )
    }
}

/// A value node in the security graph, carrying its SecurityRel and
/// associated region.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecNode {
    /// Unique node identifier.
    pub id: NodeId,
    /// The security metadata for this value.
    pub security: SecurityRel,
    /// The region this value belongs to.
    pub region: Option<RegionId>,
    /// The capabilities held on this value.
    pub capabilities: HashSet<SecurityCapability>,
    /// The taint sources (mirrored from SecurityRel for quick lookup).
    pub taint_sources: HashSet<TaintSource>,
}

/// A data-flow edge in the security graph.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecEdge {
    /// Source node.
    pub from: NodeId,
    /// Destination node.
    pub to: NodeId,
    /// Whether this is an implicit (control-flow) edge.
    pub implicit: bool,
    /// The boundary this edge crosses, if any.
    pub boundary: Option<BoundaryId>,
}

/// The **Security Verifier** — checks all security invariants over a
/// collection of security-annotated nodes, edges, and boundaries.
///
/// The verifier enforces:
/// - The no-downgrade information-flow rule (level check).
/// - Taint-at-sink checks.
/// - Boundary crossing rules (B1–B3 from the spec).
/// - Capability monotonicity.
/// - Execute-on-untrusted prevention.
/// - Declassification proof requirements.
#[derive(Debug, Clone, Default)]
pub struct SecurityVerifier {
    /// Security-annotated nodes.
    pub nodes: HashMap<NodeId, SecNode>,
    /// Data-flow edges.
    pub edges: Vec<SecEdge>,
    /// Security boundaries.
    pub boundaries: HashMap<BoundaryId, SecurityBoundary>,
    /// Valid declassification proofs.
    pub declassification_proofs: HashMap<GateFunctionId, DeclassificationProof>,
}

impl SecurityVerifier {
    /// Create a new, empty verifier.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a node to the verifier.
    pub fn add_node(&mut self, node: SecNode) {
        self.nodes.insert(node.id, node);
    }

    /// Add a data-flow edge.
    pub fn add_edge(&mut self, edge: SecEdge) {
        self.edges.push(edge);
    }

    /// Add a security boundary.
    pub fn add_boundary(&mut self, boundary: SecurityBoundary) {
        self.boundaries.insert(boundary.id, boundary);
    }

    /// Register a valid declassification proof.
    pub fn register_declassification_proof(&mut self, proof: DeclassificationProof) {
        self.declassification_proofs.insert(proof.gate, proof);
    }

    /// Run all security checks and return the aggregated result.
    pub fn verify(&self) -> VerificationResult {
        let mut result = VerificationResult::new();

        // 1. Check all data-flow edges for information-flow violations.
        self.check_information_flow(&mut result);

        // 2. Check tainted data at sinks.
        self.check_taint_at_sinks(&mut result);

        // 3. Check boundary crossing rules.
        self.check_boundary_crossings(&mut result);

        // 4. Check capability monotonicity.
        self.check_capability_monotonicity(&mut result);

        // 5. Check Execute on untrusted sources.
        self.check_execute_on_untrusted(&mut result);

        // 6. Check declassification proofs for any declassified values.
        self.check_declassification_proofs(&mut result);

        result
    }

    /// Check the no-downgrade information-flow rule on all edges.
    fn check_information_flow(&self, result: &mut VerificationResult) {
        for edge in &self.edges {
            let from_node = match self.nodes.get(&edge.from) {
                Some(n) => n,
                None => continue,
            };
            let to_node = match self.nodes.get(&edge.to) {
                Some(n) => n,
                None => continue,
            };

            match from_node.security.check_flow_to(&to_node.security) {
                Ok(()) => result.pass(),
                Err(v) => result.fail(v),
            }
        }
    }

    /// Check that tainted data does not reach clean-only sinks.
    ///
    /// Sinks are nodes whose `SecurityRel` is Clean and whose capabilities
    /// do not include `Send` (indicating they are security-critical consumers
    /// like system calls, database writes, eval).
    fn check_taint_at_sinks(&self, result: &mut VerificationResult) {
        for edge in &self.edges {
            let from_node = match self.nodes.get(&edge.from) {
                Some(n) => n,
                None => continue,
            };
            let to_node = match self.nodes.get(&edge.to) {
                Some(n) => n,
                None => continue,
            };

            if from_node.security.taint.is_tainted() && !to_node.security.taint.is_tainted() {
                // Tainted data flowing to a clean node — this is a potential
                // taint-at-sink violation. We flag it as a taint propagation
                // warning if the destination is a sink (no Send capability).
                if !to_node.capabilities.contains(&SecurityCapability::Send) {
                    result.fail(SecurityViolation::TaintedDataAtSink {
                        sources: from_node.security.taint.sources(),
                        sink: format!("node {}", to_node.id),
                    });
                } else {
                    result.pass();
                }
            } else {
                result.pass();
            }
        }
    }

    /// Check boundary crossing rules (B1–B3).
    fn check_boundary_crossings(&self, result: &mut VerificationResult) {
        for edge in &self.edges {
            let Some(boundary_id) = edge.boundary else {
                continue;
            };
            let Some(boundary) = self.boundaries.get(&boundary_id) else {
                continue;
            };
            let from_node = match self.nodes.get(&edge.from) {
                Some(n) => n,
                None => continue,
            };
            let to_node = match self.nodes.get(&edge.to) {
                Some(n) => n,
                None => continue,
            };

            // Determine direction.
            let from_region = from_node.region;
            let to_region = to_node.region;

            // Only check if the edge actually crosses the boundary.
            let crosses_high_to_low = from_region == Some(boundary.region_high)
                && to_region == Some(boundary.region_low);
            let crosses_low_to_high = from_region == Some(boundary.region_low)
                && to_region == Some(boundary.region_high);

            if !crosses_high_to_low && !crosses_low_to_high {
                result.pass();
                continue;
            }

            if edge.implicit {
                // Implicit flow across boundary.
                if from_node.security.effective_level() > boundary.level_low {
                    result.fail(SecurityViolation::ImplicitFlowAcrossBoundary {
                        condition_level: from_node.security.effective_level(),
                        target_level: boundary.level_low,
                    });
                    continue;
                }
            }

            if crosses_high_to_low {
                // B1: Read-across (High → Low).
                match boundary.check_read_across(
                    boundary.region_high,
                    from_node.security.effective_level(),
                ) {
                    Ok(()) => result.pass(),
                    Err(v) => result.fail(v),
                }
            } else {
                // Low → High: check control-flow capabilities (B3).
                match boundary.check_control_flow_across(&from_node.capabilities) {
                    Ok(()) => result.pass(),
                    Err(v) => result.fail(v),
                }
            }
        }
    }

    /// Check that capabilities only decrease over time (monotonicity).
    ///
    /// This is a structural check: along each edge, the destination's
    /// capabilities must be a subset of the source's capabilities.
    fn check_capability_monotonicity(&self, result: &mut VerificationResult) {
        for edge in &self.edges {
            let from_node = match self.nodes.get(&edge.from) {
                Some(n) => n,
                None => continue,
            };
            let to_node = match self.nodes.get(&edge.to) {
                Some(n) => n,
                None => continue,
            };

            // Check that to_node's capabilities are a subset of from_node's.
            let added: Vec<_> = to_node
                .capabilities
                .difference(&from_node.capabilities)
                .copied()
                .collect();

            if added.is_empty() {
                result.pass();
            } else {
                for cap in added {
                    result.fail(SecurityViolation::CapabilityMonotonicityViolation {
                        added: cap,
                    });
                }
            }
        }
    }

    /// Check that no value from an untrusted source carries Execute.
    fn check_execute_on_untrusted(&self, result: &mut VerificationResult) {
        for node in self.nodes.values() {
            if node.capabilities.contains(&SecurityCapability::Execute)
                && node.security.taint.is_tainted()
            {
                for source in node.security.taint.sources() {
                    result.fail(SecurityViolation::ExecuteOnUntrusted { source });
                }
            } else {
                result.pass();
            }
        }
    }

    /// Check that declassified values have valid proofs.
    fn check_declassification_proofs(&self, result: &mut VerificationResult) {
        for node in self.nodes.values() {
            if let Some(ref dec) = node.security.declassification {
                // The gate must have a registered, valid proof.
                if let Some(proof) = self.declassification_proofs.get(&dec.gate_function) {
                    if proof.is_valid() {
                        result.pass();
                    } else {
                        result.fail(SecurityViolation::DeclassificationWithoutProof {
                            from_level: dec.from_level,
                            to_level: dec.to_level,
                        });
                    }
                } else {
                    result.fail(SecurityViolation::DeclassificationWithoutProof {
                        from_level: dec.from_level,
                        to_level: dec.to_level,
                    });
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // Helper to make a ProgramPoint for testing.
    fn pp(line: u32) -> ProgramPoint {
        ProgramPoint::new("test.vu", line, 1)
    }

    // ── Lattice tests ──────────────────────────────────────────────────

    #[test]
    fn lattice_ordering() {
        assert!(SecurityLevel::Public < SecurityLevel::Internal);
        assert!(SecurityLevel::Internal < SecurityLevel::Confidential);
        assert!(SecurityLevel::Confidential < SecurityLevel::Secret);
        assert!(SecurityLevel::Secret < SecurityLevel::TopSecret);
    }

    #[test]
    fn lattice_join_and_meet() {
        assert_eq!(
            SecurityLevel::Public.join(SecurityLevel::Secret),
            SecurityLevel::Secret
        );
        assert_eq!(
            SecurityLevel::Confidential.meet(SecurityLevel::Secret),
            SecurityLevel::Confidential
        );
        // Idempotence
        assert_eq!(SecurityLevel::Secret.join(SecurityLevel::Secret), SecurityLevel::Secret);
        assert_eq!(SecurityLevel::Secret.meet(SecurityLevel::Secret), SecurityLevel::Secret);
    }

    #[test]
    fn lattice_commutativity_and_associativity() {
        let a = SecurityLevel::Public;
        let b = SecurityLevel::Confidential;
        let c = SecurityLevel::TopSecret;

        // Commutativity
        assert_eq!(a.join(b), b.join(a));
        assert_eq!(a.meet(b), b.meet(a));

        // Associativity
        assert_eq!(a.join(b).join(c), a.join(b.join(c)));
        assert_eq!(a.meet(b).meet(c), a.meet(b.meet(c)));
    }

    #[test]
    fn lattice_absorption() {
        let a = SecurityLevel::Confidential;
        let b = SecurityLevel::Secret;
        assert_eq!(a.join(a.meet(b)), a);
        assert_eq!(a.meet(a.join(b)), a);
    }

    #[test]
    fn can_flow_to_respects_lattice() {
        assert!(SecurityLevel::Public.can_flow_to(SecurityLevel::Secret));
        assert!(SecurityLevel::Secret.can_flow_to(SecurityLevel::Secret));
        assert!(!SecurityLevel::Secret.can_flow_to(SecurityLevel::Public));
    }

    #[test]
    fn top_and_bottom() {
        assert_eq!(SecurityLevel::BOTTOM, SecurityLevel::Public);
        assert_eq!(SecurityLevel::TOP, SecurityLevel::TopSecret);
    }

    // ── Taint tracking tests ───────────────────────────────────────────

    #[test]
    fn taint_propagation_unions_sources() {
        let t1 = TaintStatus::tainted(TaintSource::UserInput, true);
        let t2 = TaintStatus::tainted(TaintSource::Network, true);
        let combined = t1.propagate(&t2);
        match combined {
            TaintStatus::Tainted { sources, sanitizable } => {
                assert!(sources.contains(&TaintSource::UserInput));
                assert!(sources.contains(&TaintSource::Network));
                assert!(sanitizable); // both sanitizable
            }
            TaintStatus::Clean => panic!("expected Tainted"),
        }
    }

    #[test]
    fn taint_sanitization_succeeds_when_sanitizable() {
        let tainted = TaintStatus::tainted(TaintSource::UserInput, true);
        let result = tainted.sanitize();
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), TaintStatus::Clean);
    }

    #[test]
    fn taint_sanitization_fails_when_not_sanitizable() {
        let tainted = TaintStatus::tainted(TaintSource::Network, false);
        let result = tainted.sanitize();
        assert!(result.is_err());
    }

    #[test]
    fn taint_effective_level_boosts_to_internal() {
        let tainted = TaintStatus::tainted(TaintSource::UserInput, true);
        // Public + Internal = Internal
        assert_eq!(tainted.effective_level(SecurityLevel::Public), SecurityLevel::Internal);
        // Secret + Internal = Secret (Secret is already higher)
        assert_eq!(tainted.effective_level(SecurityLevel::Secret), SecurityLevel::Secret);
        // Clean data stays at its level
        assert_eq!(TaintStatus::Clean.effective_level(SecurityLevel::Public), SecurityLevel::Public);
    }

    // ── SecurityRel tests ──────────────────────────────────────────────

    #[test]
    fn security_rel_flow_check_allows_upward() {
        let src = SecurityRel::at(SecurityLevel::Public);
        let dst = SecurityRel::at(SecurityLevel::Secret);
        assert!(src.check_flow_to(&dst).is_ok());
    }

    #[test]
    fn security_rel_flow_check_blocks_downward() {
        let src = SecurityRel::at(SecurityLevel::Secret);
        let dst = SecurityRel::at(SecurityLevel::Public);
        assert!(src.check_flow_to(&dst).is_err());
    }

    #[test]
    fn security_rel_no_flow_blocks_everything() {
        let key = SecurityRel::for_key_material(SecurityLevel::Secret);
        let dst = SecurityRel::at(SecurityLevel::TopSecret);
        assert!(key.check_flow_to(&dst).is_err());
    }

    #[test]
    fn security_rel_join_combines_levels() {
        let a = SecurityRel::at(SecurityLevel::Public);
        let b = SecurityRel::at(SecurityLevel::Secret);
        let joined = a.join(&b);
        assert_eq!(joined.level, SecurityLevel::Secret);
    }

    // ── Flow Policy tests ──────────────────────────────────────────────

    #[test]
    fn flow_policy_ordering() {
        assert_eq!(
            FlowPolicy::FreeFlow.more_restrictive(FlowPolicy::NoDowngrade),
            FlowPolicy::NoDowngrade
        );
        assert_eq!(
            FlowPolicy::NoDowngrade.more_restrictive(FlowPolicy::NoFlow),
            FlowPolicy::NoFlow
        );
        assert_eq!(
            FlowPolicy::FreeFlow.more_restrictive(FlowPolicy::NoFlow),
            FlowPolicy::NoFlow
        );
    }

    // ── Boundary tests ─────────────────────────────────────────────────

    #[test]
    fn boundary_allows_upward_read() {
        let boundary = SecurityBoundary::new(
            BoundaryId(1),
            RegionId(10),
            RegionId(20),
            SecurityLevel::Secret,
            SecurityLevel::Public,
        );
        // Low → High: always permitted
        assert!(boundary.check_read_across(RegionId(20), SecurityLevel::Public).is_ok());
    }

    #[test]
    fn boundary_blocks_downward_read_without_gate() {
        let boundary = SecurityBoundary::new(
            BoundaryId(1),
            RegionId(10),
            RegionId(20),
            SecurityLevel::Secret,
            SecurityLevel::Public,
        );
        // High → Low: Secret cannot flow to Public
        assert!(boundary.check_read_across(RegionId(10), SecurityLevel::Secret).is_err());
    }

    #[test]
    fn boundary_allows_downward_read_with_gate() {
        let mut boundary = SecurityBoundary::new(
            BoundaryId(1),
            RegionId(10),
            RegionId(20),
            SecurityLevel::Secret,
            SecurityLevel::Public,
        );
        boundary.declassification_gate = Some(GateFunctionId(42));
        // High → Low: with gate, declassification is possible
        assert!(boundary.check_read_across(RegionId(10), SecurityLevel::Secret).is_ok());
    }

    #[test]
    fn boundary_control_flow_requires_capabilities() {
        let mut boundary = SecurityBoundary::new(
            BoundaryId(1),
            RegionId(10),
            RegionId(20),
            SecurityLevel::Secret,
            SecurityLevel::Public,
        );
        boundary.cross_permissions = [SecurityCapability::Read, SecurityCapability::Execute]
            .into_iter()
            .collect();

        // Missing Execute
        let caps: HashSet<SecurityCapability> = [SecurityCapability::Read].into_iter().collect();
        assert!(boundary.check_control_flow_across(&caps).is_err());

        // All present
        let caps: HashSet<SecurityCapability> =
            [SecurityCapability::Read, SecurityCapability::Execute]
                .into_iter()
                .collect();
        assert!(boundary.check_control_flow_across(&caps).is_ok());
    }

    // ── Declassification tests ─────────────────────────────────────────

    #[test]
    fn declassification_proof_requires_all_verifications() {
        let mut proof = DeclassificationProof::new(
            GateFunctionId(1),
            SecurityLevel::Secret,
            SecurityLevel::Public,
        );
        assert!(!proof.is_valid()); // nothing verified yet

        proof.output_independence_verified = true;
        assert!(!proof.is_valid()); // still missing others

        proof.no_side_channels_verified = true;
        assert!(!proof.is_valid()); // still missing completeness

        proof.completeness_verified = true;
        assert!(proof.is_valid()); // all verified
    }

    #[test]
    fn declassification_proof_verify_all() {
        let mut proof = DeclassificationProof::new(
            GateFunctionId(1),
            SecurityLevel::Secret,
            SecurityLevel::Public,
        );
        proof.verify_all();
        assert!(proof.is_valid());
    }

    // ── ARM64 mapping tests ────────────────────────────────────────────

    #[test]
    fn arm64_mapping_capability_to_hw() {
        let mapping = Arm64SecurityMapping::pi5_development();

        assert!(mapping.capability_to_hw(SecurityCapability::DerivePtr).contains(&Arm64Feature::Pac));
        assert!(mapping.capability_to_hw(SecurityCapability::Execute).contains(&Arm64Feature::Bti));
        assert!(mapping.capability_to_hw(SecurityCapability::Read).contains(&Arm64Feature::Mte));
        assert!(mapping.capability_to_hw(SecurityCapability::Write).contains(&Arm64Feature::Mte));
    }

    #[test]
    fn arm64_mapping_disabled_returns_empty() {
        let mapping = Arm64SecurityMapping::disabled();
        assert!(mapping.capability_to_hw(SecurityCapability::DerivePtr).is_empty());
        assert!(mapping.capability_to_hw(SecurityCapability::Execute).is_empty());
    }

    #[test]
    fn arm64_pac_sign_pseudocode() {
        let mapping = Arm64SecurityMapping::pi5_development();
        assert!(mapping.emit_pac_sign(true).contains("pac_sign"));
        assert!(mapping.emit_pac_sign(false).contains("missing DerivePtr"));
    }

    #[test]
    fn arm64_bti_landing_pad() {
        let mapping = Arm64SecurityMapping::pi5_development();
        assert!(mapping.emit_bti_landing_pad(true).contains("bti c"));
        assert!(mapping.emit_bti_landing_pad(false).contains("bti j"));
    }

    #[test]
    fn arm64_mte_mode_difference() {
        let dev = Arm64SecurityMapping::pi5_development();
        let prod = Arm64SecurityMapping::pi5_production();
        assert_eq!(dev.mte_mode, MteMode::Synchronous);
        assert_eq!(prod.mte_mode, MteMode::Asynchronous);
    }

    // ── Security Verifier tests ────────────────────────────────────────

    fn make_sec_node(
        id: u64,
        level: SecurityLevel,
        region: Option<RegionId>,
        caps: Vec<SecurityCapability>,
        taint: TaintStatus,
    ) -> SecNode {
        SecNode {
            id: NodeId(id),
            security: SecurityRel {
                level,
                flow: FlowPolicy::NoDowngrade,
                taint,
                declassification: None,
            },
            region,
            capabilities: caps.into_iter().collect(),
            taint_sources: HashSet::new(),
        }
    }

    #[test]
    fn verifier_passes_clean_upward_flow() {
        let mut v = SecurityVerifier::new();
        v.add_node(make_sec_node(
            1,
            SecurityLevel::Public,
            None,
            vec![SecurityCapability::Read, SecurityCapability::Write],
            TaintStatus::Clean,
        ));
        v.add_node(make_sec_node(
            2,
            SecurityLevel::Secret,
            None,
            vec![SecurityCapability::Read],
            TaintStatus::Clean,
        ));
        v.add_edge(SecEdge {
            from: NodeId(1),
            to: NodeId(2),
            implicit: false,
            boundary: None,
        });

        let result = v.verify();
        assert!(result.all_passed(), "Expected all checks to pass, got violations: {:?}", result.violations);
    }

    #[test]
    fn verifier_detects_information_leak() {
        let mut v = SecurityVerifier::new();
        v.add_node(make_sec_node(
            1,
            SecurityLevel::Secret,
            None,
            vec![SecurityCapability::Read],
            TaintStatus::Clean,
        ));
        v.add_node(make_sec_node(
            2,
            SecurityLevel::Public,
            None,
            vec![SecurityCapability::Read],
            TaintStatus::Clean,
        ));
        v.add_edge(SecEdge {
            from: NodeId(1),
            to: NodeId(2),
            implicit: false,
            boundary: None,
        });

        let result = v.verify();
        assert!(!result.all_passed());
        assert!(result.violations.iter().any(|v| matches!(
            v,
            SecurityViolation::InformationLeak { .. }
        )));
    }

    #[test]
    fn verifier_detects_execute_on_untrusted() {
        let mut v = SecurityVerifier::new();
        v.add_node(SecNode {
            id: NodeId(1),
            security: SecurityRel::for_untrusted(SecurityLevel::Internal, TaintSource::Network),
            region: None,
            capabilities: [SecurityCapability::Read, SecurityCapability::Execute]
                .into_iter()
                .collect(),
            taint_sources: [TaintSource::Network].into_iter().collect(),
        });

        let result = v.verify();
        assert!(!result.all_passed());
        assert!(result.violations.iter().any(|v| matches!(
            v,
            SecurityViolation::ExecuteOnUntrusted { .. }
        )));
    }

    #[test]
    fn verifier_detects_capability_monotonicity_violation() {
        let mut v = SecurityVerifier::new();
        v.add_node(make_sec_node(
            1,
            SecurityLevel::Internal,
            None,
            vec![SecurityCapability::Read],
            TaintStatus::Clean,
        ));
        v.add_node(make_sec_node(
            2,
            SecurityLevel::Internal,
            None,
            vec![SecurityCapability::Read, SecurityCapability::Execute],
            TaintStatus::Clean,
        ));
        v.add_edge(SecEdge {
            from: NodeId(1),
            to: NodeId(2),
            implicit: false,
            boundary: None,
        });

        let result = v.verify();
        assert!(!result.all_passed());
        assert!(result.violations.iter().any(|v| matches!(
            v,
            SecurityViolation::CapabilityMonotonicityViolation { .. }
        )));
    }

    #[test]
    fn verifier_detects_declassification_without_proof() {
        let mut v = SecurityVerifier::new();
        let mut node = make_sec_node(
            1,
            SecurityLevel::Public, // level after declassification
            None,
            vec![SecurityCapability::Read],
            TaintStatus::Clean,
        );
        node.security.declassification = Some(DeclassificationRecord {
            gate_function: GateFunctionId(99),
            from_level: SecurityLevel::Secret,
            to_level: SecurityLevel::Public,
            source_location: pp(42),
            proof: DeclassificationProof::new(
                GateFunctionId(99),
                SecurityLevel::Secret,
                SecurityLevel::Public,
            ),
        });
        v.add_node(node);

        let result = v.verify();
        assert!(!result.all_passed());
        assert!(result.violations.iter().any(|v| matches!(
            v,
            SecurityViolation::DeclassificationWithoutProof { .. }
        )));
    }

    #[test]
    fn verifier_accepts_valid_declassification() {
        let mut v = SecurityVerifier::new();
        let mut proof =
            DeclassificationProof::new(GateFunctionId(99), SecurityLevel::Secret, SecurityLevel::Public);
        proof.verify_all();
        v.register_declassification_proof(proof);

        let mut node = make_sec_node(
            1,
            SecurityLevel::Public,
            None,
            vec![SecurityCapability::Read],
            TaintStatus::Clean,
        );
        let mut valid_proof =
            DeclassificationProof::new(GateFunctionId(99), SecurityLevel::Secret, SecurityLevel::Public);
        valid_proof.verify_all();
        node.security.declassification = Some(DeclassificationRecord {
            gate_function: GateFunctionId(99),
            from_level: SecurityLevel::Secret,
            to_level: SecurityLevel::Public,
            source_location: pp(42),
            proof: valid_proof,
        });
        v.add_node(node);

        let result = v.verify();
        // The declassification proof check should pass.
        assert!(result.all_passed(), "Expected all checks to pass, got violations: {:?}", result.violations);
    }

    #[test]
    fn verifier_detects_boundary_violation() {
        let mut v = SecurityVerifier::new();

        // Create boundary: Region 10 (Secret) → Region 20 (Public)
        let boundary = SecurityBoundary::new(
            BoundaryId(1),
            RegionId(10),
            RegionId(20),
            SecurityLevel::Secret,
            SecurityLevel::Public,
        );
        v.add_boundary(boundary);

        // Secret value in high region
        v.add_node(make_sec_node(
            1,
            SecurityLevel::Secret,
            Some(RegionId(10)),
            vec![SecurityCapability::Read],
            TaintStatus::Clean,
        ));
        // Public value in low region
        v.add_node(make_sec_node(
            2,
            SecurityLevel::Public,
            Some(RegionId(20)),
            vec![SecurityCapability::Read],
            TaintStatus::Clean,
        ));
        // Edge crossing from high to low without declassification
        v.add_edge(SecEdge {
            from: NodeId(1),
            to: NodeId(2),
            implicit: false,
            boundary: Some(BoundaryId(1)),
        });

        let result = v.verify();
        assert!(!result.all_passed());
    }

    #[test]
    fn verifier_allows_boundary_crossing_upward() {
        let mut v = SecurityVerifier::new();

        let boundary = SecurityBoundary::new(
            BoundaryId(1),
            RegionId(10),
            RegionId(20),
            SecurityLevel::Secret,
            SecurityLevel::Public,
        );
        v.add_boundary(boundary);

        // Public value in low region
        v.add_node(make_sec_node(
            1,
            SecurityLevel::Public,
            Some(RegionId(20)),
            vec![SecurityCapability::Read],
            TaintStatus::Clean,
        ));
        // Secret value in high region
        v.add_node(make_sec_node(
            2,
            SecurityLevel::Secret,
            Some(RegionId(10)),
            vec![SecurityCapability::Read],
            TaintStatus::Clean,
        ));
        // Edge crossing from low to high (upward flow — always OK)
        v.add_edge(SecEdge {
            from: NodeId(1),
            to: NodeId(2),
            implicit: false,
            boundary: Some(BoundaryId(1)),
        });

        let result = v.verify();
        assert!(result.all_passed(), "Expected all checks to pass, got violations: {:?}", result.violations);
    }

    #[test]
    fn verifier_detects_implicit_flow_across_boundary() {
        let mut v = SecurityVerifier::new();

        let boundary = SecurityBoundary::new(
            BoundaryId(1),
            RegionId(10),
            RegionId(20),
            SecurityLevel::Secret,
            SecurityLevel::Public,
        );
        v.add_boundary(boundary);

        // Secret condition in high region
        v.add_node(make_sec_node(
            1,
            SecurityLevel::Secret,
            Some(RegionId(10)),
            vec![SecurityCapability::Read],
            TaintStatus::Clean,
        ));
        // Public value in low region
        v.add_node(make_sec_node(
            2,
            SecurityLevel::Public,
            Some(RegionId(20)),
            vec![SecurityCapability::Read],
            TaintStatus::Clean,
        ));
        // Implicit edge from high to low
        v.add_edge(SecEdge {
            from: NodeId(1),
            to: NodeId(2),
            implicit: true,
            boundary: Some(BoundaryId(1)),
        });

        let result = v.verify();
        assert!(!result.all_passed());
        assert!(result.violations.iter().any(|v| matches!(
            v,
            SecurityViolation::ImplicitFlowAcrossBoundary { .. }
        )));
    }

    // ── Taint through derivation chain test ─────────────────────────────

    #[test]
    fn taint_propagation_through_derivation_chain() {
        use crate::address::Address;
        use crate::derivation::{DerivationKind, DerivationSource};

        let d1 = Derivation {
            id: DerivationId(1),
            source: DerivationSource::Region(RegionId(1)),
            kind: DerivationKind::Direct,
            proven_range: (Address::from(0x1000_u64), Address::from(0x2000_u64)),
        };
        let d2 = Derivation {
            id: DerivationId(2),
            source: DerivationSource::AnotherDerivation(DerivationId(1)),
            kind: DerivationKind::Offset { by: 0x40 },
            proven_range: (Address::from(0x1040_u64), Address::from(0x1080_u64)),
        };
        let d3 = Derivation {
            id: DerivationId(3),
            source: DerivationSource::AnotherDerivation(DerivationId(2)),
            kind: DerivationKind::Offset { by: 0x10 },
            proven_range: (Address::from(0x1050_u64), Address::from(0x1060_u64)),
        };

        // d1 is tainted with UserInput, d2 is tainted with Network.
        let mut taint_map: HashMap<DerivationId, TaintStatus> = HashMap::new();
        taint_map.insert(DerivationId(1), TaintStatus::tainted(TaintSource::UserInput, true));
        taint_map.insert(DerivationId(2), TaintStatus::tainted(TaintSource::Network, true));

        let lookup = |id: DerivationId| match id.0 {
            1 => Some(d1.clone()),
            2 => Some(d2.clone()),
            _ => None,
        };

        // d3 should inherit taint from both d1 and d2.
        let result = propagate_taint_through_chain(&d3, &taint_map, lookup);
        match result {
            TaintStatus::Tainted { sources, .. } => {
                assert!(sources.contains(&TaintSource::UserInput));
                assert!(sources.contains(&TaintSource::Network));
            }
            TaintStatus::Clean => panic!("expected Tainted"),
        }
    }

    // ── Full verification result display test ───────────────────────────

    #[test]
    fn verification_result_display() {
        let mut result = VerificationResult::new();
        result.pass();
        result.pass();
        result.fail(SecurityViolation::InformationLeak {
            src_level: SecurityLevel::Secret,
            dst_level: SecurityLevel::Public,
        });
        let display = format!("{}", result);
        assert!(display.contains("2/3 passed"));
        assert!(display.contains("1 violations"));
    }

    #[test]
    fn security_level_display() {
        assert_eq!(format!("{}", SecurityLevel::Public), "Public");
        assert_eq!(format!("{}", SecurityLevel::TopSecret), "TopSecret");
    }

    #[test]
    fn arm64_capabilities_to_hw_set() {
        let mapping = Arm64SecurityMapping::pi5_development();
        let caps: HashSet<SecurityCapability> = [
            SecurityCapability::DerivePtr,
            SecurityCapability::Execute,
            SecurityCapability::Read,
        ]
        .into_iter()
        .collect();
        let hw = mapping.capabilities_to_hw(&caps);
        assert!(hw.contains(&Arm64Feature::Pac));
        assert!(hw.contains(&Arm64Feature::Bti));
        assert!(hw.contains(&Arm64Feature::Mte));
    }

    // ── TaintLabel tests ───────────────────────────────────────────────

    #[test]
    fn taint_label_clean_by_default() {
        let label = TaintLabel::clean();
        assert!(label.is_clean());
        assert!(!label.is_tainted());
        assert!(label.sources().is_empty());
    }

    #[test]
    fn taint_label_from_source() {
        let label = TaintLabel::from_source(TaintSource::UserInput);
        assert!(label.is_tainted());
        assert!(!label.is_clean());
        assert!(label.contains(&TaintSource::UserInput));
        assert!(!label.contains(&TaintSource::Network));
    }

    #[test]
    fn taint_label_join_unions_sources() {
        let a = TaintLabel::from_source(TaintSource::UserInput);
        let b = TaintLabel::from_source(TaintSource::Network);
        let joined = a.join(&b);
        assert!(joined.is_tainted());
        assert!(joined.contains(&TaintSource::UserInput));
        assert!(joined.contains(&TaintSource::Network));
        // Join with clean is identity
        let clean = TaintLabel::clean();
        let joined_with_clean = a.join(&clean);
        assert!(joined_with_clean.contains(&TaintSource::UserInput));
        assert!(!joined_with_clean.contains(&TaintSource::Network));
    }

    #[test]
    fn taint_label_to_status_conversion() {
        let label = TaintLabel::from_source(TaintSource::UntrustedFile);
        let status = label.to_status(true);
        assert!(status.is_tainted());
        match status {
            TaintStatus::Tainted { sources, sanitizable } => {
                assert!(sources.contains(&TaintSource::UntrustedFile));
                assert!(sanitizable);
            }
            TaintStatus::Clean => panic!("expected Tainted"),
        }
        // Clean label → Clean status
        let clean_label = TaintLabel::clean();
        let clean_status = clean_label.to_status(false);
        assert_eq!(clean_status, TaintStatus::Clean);
    }

    #[test]
    fn taint_label_display() {
        let label = TaintLabel::from_source(TaintSource::Network);
        let display = format!("{}", label);
        assert!(display.contains("Network"));
        let clean = TaintLabel::clean();
        assert_eq!(format!("{}", clean), "Clean");
    }

    // ── TaintTracker tests ─────────────────────────────────────────────

    #[test]
    fn taint_tracker_propagation_simple() {
        let mut tracker = TaintTracker::new();
        tracker.set_label(NodeId(1), TaintLabel::from_source(TaintSource::UserInput));
        tracker.set_label(NodeId(2), TaintLabel::clean());
        tracker.add_edge(NodeId(1), NodeId(2));
        let iters = tracker.propagate();
        assert_eq!(iters, 2); // first pass propagates, second pass is stable
        let label = tracker.get_label(NodeId(2));
        assert!(label.contains(&TaintSource::UserInput));
    }

    #[test]
    fn taint_tracker_propagation_chain() {
        let mut tracker = TaintTracker::new();
        tracker.set_label(NodeId(1), TaintLabel::from_source(TaintSource::Network));
        tracker.set_label(NodeId(2), TaintLabel::clean());
        tracker.set_label(NodeId(3), TaintLabel::clean());
        tracker.add_edge(NodeId(1), NodeId(2));
        tracker.add_edge(NodeId(2), NodeId(3));
        tracker.propagate();
        // Taint should flow 1 → 2 → 3
        assert!(tracker.get_label(NodeId(2)).contains(&TaintSource::Network));
        assert!(tracker.get_label(NodeId(3)).contains(&TaintSource::Network));
    }

    #[test]
    fn taint_tracker_multiple_sources_merge() {
        let mut tracker = TaintTracker::new();
        tracker.set_label(NodeId(1), TaintLabel::from_source(TaintSource::UserInput));
        tracker.set_label(NodeId(2), TaintLabel::from_source(TaintSource::Network));
        tracker.set_label(NodeId(3), TaintLabel::clean());
        tracker.add_edge(NodeId(1), NodeId(3));
        tracker.add_edge(NodeId(2), NodeId(3));
        tracker.propagate();
        let label = tracker.get_label(NodeId(3));
        assert!(label.contains(&TaintSource::UserInput));
        assert!(label.contains(&TaintSource::Network));
    }

    #[test]
    fn taint_tracker_tainted_nodes() {
        let mut tracker = TaintTracker::new();
        tracker.set_label(NodeId(1), TaintLabel::from_source(TaintSource::UserInput));
        tracker.set_label(NodeId(2), TaintLabel::clean());
        tracker.set_label(NodeId(3), TaintLabel::from_source(TaintSource::Network));
        let tainted = tracker.tainted_nodes();
        assert_eq!(tainted.len(), 2);
    }

    #[test]
    fn taint_tracker_propagate_chain_through_derivation() {
        use crate::address::Address;
        use crate::derivation::{DerivationKind, DerivationSource};

        let d1 = Derivation {
            id: DerivationId(1),
            source: DerivationSource::Region(RegionId(1)),
            kind: DerivationKind::Direct,
            proven_range: (Address::from(0x1000_u64), Address::from(0x2000_u64)),
        };
        let d2 = Derivation {
            id: DerivationId(2),
            source: DerivationSource::AnotherDerivation(DerivationId(1)),
            kind: DerivationKind::Offset { by: 0x40 },
            proven_range: (Address::from(0x1040_u64), Address::from(0x1080_u64)),
        };

        let mut taint_map: HashMap<DerivationId, TaintLabel> = HashMap::new();
        taint_map.insert(DerivationId(1), TaintLabel::from_source(TaintSource::UntrustedFile));

        let lookup = |id: DerivationId| match id.0 {
            1 => Some(d1.clone()),
            _ => None,
        };

        let result = TaintTracker::propagate_chain(&d2, &taint_map, lookup);
        assert!(result.contains(&TaintSource::UntrustedFile));
    }
}
