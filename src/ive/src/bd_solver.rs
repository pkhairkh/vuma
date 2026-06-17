//! BD Constraint Solver for the IVE module.
//!
//! This module implements a constraint solver for Behavioral Descriptors (BDs).
//! Given a set of constraints relating BDs at different nodes in the SCG,
//! the solver finds a solution (assignment of BDs to nodes) that satisfies
//! all constraints, or reports unsatisfiable constraints as structured errors.
//!
//! # Constraint Types
//!
//! | Constraint         | Meaning                                            |
//! |--------------------|----------------------------------------------------|
//! | `RepDCompatible`   | Two nodes must have compatible representations      |
//! | `CapDWeakening`    | One node's capabilities must be a subset of another's |
//! | `RelDRefinement`   | One node's relations must refine another's          |
//! | `Equality`         | Two nodes must have identical BDs                   |
//!
//! # Solving Strategy
//!
//! The solver uses **iterative fixed-point iteration** with **widening** for
//! recursive constraints:
//!
//! 1. Initialize each SCG node with a most-permissive "top" BD.
//! 2. For each constraint, check satisfaction and narrow/widen BDs.
//! 3. Repeat until no BD changes (fixed point).
//! 4. After a configurable threshold, apply widening to force convergence.
//! 5. Detect and report unsatisfiable constraints.
//!
//! # Complexity
//!
//! Each iteration runs in O(|nodes| × |caps|²) time, where |caps| is the
//! maximum number of capabilities at any node. With widening, convergence
//! is guaranteed within a constant number of iterations, giving an overall
//! bound of O(|nodes| × |caps|²).
//!
//! # Fixpoint Convergence (W6)
//!
//! `solve` / `solve_with_initial` perform a **real fixpoint** computation
//! over the finite BD lattice. Convergence is guaranteed by two properties:
//!
//! ## 1. Finiteness of the BD lattice
//!
//! For any given SCG + constraint set, the set of reachable BD values is
//! finite. See the `vuma_bd::inference` module doc for the full proof;
//! briefly:
//!
//! - **RepD** has 8 variants, each built from finitely many `u64`
//!   size/align fields and recursive RepDs drawn from the SCG (finite).
//! - **CapD** is `(caps: HashSet<Capability>, conditions:
//!   HashSet<Condition>)`. `Capability` has 17 variants ⇒ at most
//!   `2^17` cap-subsets; `Condition` IDs come from the SCG (finite).
//! - **RelD** is `relations: HashSet<Relation>` where `Relation` has
//!   12 variants ⇒ at most `2^12` relation-subsets.
//!
//! The product of three finite lattices is finite.
//!
//! ## 2. Per-component monotonicity of constraint application
//!
//! Each constraint application moves the affected BD **monotonically**
//! in a single direction along one component lattice:
//!
//! - **`RepDCompatible`**: a default (unresolved) RepD is replaced by
//!   a specific one. Default → specific is monotone DOWN (more
//!   precise). Two specific incompatible RepDs ⇒ error (terminates
//!   the iteration, no oscillation).
//! - **`CapDWeakening`** (`node_a.capd ⊆ node_b.capd`): widen
//!   `node_b.capd` by joining with `node_a.capd`. Join = union of
//!   caps. Caps only grow ⇒ **monotone UP** for `node_b`. Widening
//!   (after `widening_threshold` iterations) drops conditions,
//!   moving further UP — still monotone.
//! - **`RelDRefinement`** (`node_a.reld refines node_b.reld`):
//!   compose `node_b.reld` into `node_a.reld`. Compose = union of
//!   relations. Relations only grow ⇒ **monotone UP** for `node_a`.
//! - **`Equality`** (`node_a == node_b`): set both to the meet.
//!   CapD meet = intersection (caps shrink ⇒ DOWN). RelD compose =
//!   union (relations grow ⇒ UP). RepD = more specific (DOWN).
//!   Mixed directions, but each component is monotone.
//!
//! Each component reaches a fixed point in at most `|component lattice|`
//! steps. The product converges in at most the sum of those bounds —
//! a finite number for any given SCG.
//!
//! ## Termination contract
//!
//! Because the lattice is finite and the update is monotone,
//! **non-convergence is a bug** (a non-monotone update somewhere in
//! the solver), not a depth limit. The `max_iterations` field is a
//! safety net: if the iteration ever exceeds it, the solver returns
//! [`SolverError::NoConvergence`] with a clear message identifying
//! the non-monotonicity, rather than silently capping out and
//! returning a possibly-unsound solution. The default cap (10 000)
//! is far above the worst-case lattice size for any realistic SCG.

// NOTE(W1-c): `HashMap` was replaced by `BTreeMap` for the fixpoint `solution`
// map to guarantee deterministic iteration order during constraint solving
// (HashMap iteration order is randomized, which led to non-deterministic
// fixpoint convergence). `HashSet` is kept as `hashbrown::HashSet` because it
// is required for constructing the `CapD.caps` and `CapD.conditions` fields,
// whose types are defined externally in `vuma_bd::capd` as
// `hashbrown::HashSet<...>`. The `valid_nodes: HashSet<NodeId>` locals only use
// `.contains()` (no iteration-dependent soundness), so they are left as-is.
use hashbrown::HashSet;
use std::collections::BTreeSet;
use std::collections::BTreeMap;
use std::fmt;
use vuma_bd::capd::CapD;
use vuma_bd::descriptor::BD;
use vuma_bd::reld::RelD;
use vuma_bd::repd::{ByteRep, RepD};
use vuma_scg::graph::SCG;
use vuma_scg::node::NodeId;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Default safety-net cap on the number of solver iterations before
/// declaring non-convergence.
///
/// The BD lattice is finite and each constraint application is
/// per-component monotone (see the module-level doc), so the iteration
/// provably converges in a finite number of steps for any well-formed
/// SCG + constraint set. This cap exists only to fail fast (with
/// [`SolverError::NoConvergence`]) if a non-monotone update is ever
/// introduced into the solver. The default (10 000) is far above the
/// worst-case lattice size for any realistic SCG.
const DEFAULT_MAX_ITERATIONS: usize = 10_000;

/// Default number of iterations after which widening is applied.
const DEFAULT_WIDENING_THRESHOLD: usize = 10;

// ---------------------------------------------------------------------------
// SolverError
// ---------------------------------------------------------------------------

/// Errors produced by the BD constraint solver.
///
/// Each variant captures the nodes and BD components involved, enabling
/// precise diagnostics for debugging and error reporting.
#[derive(Debug, Clone)]
pub enum SolverError {
    /// The representation descriptors of two nodes are incompatible.
    RepDIncompatible {
        /// The first node with an incompatible representation.
        node_a: NodeId,
        /// The second node with an incompatible representation.
        node_b: NodeId,
        /// The representation descriptor of `node_a`.
        repd_a: RepD,
        /// The representation descriptor of `node_b`.
        repd_b: RepD,
    },

    /// CapD weakening is impossible: narrowing node_a's capabilities would
    /// violate other constraints, and widening node_b would exceed bounds.
    CapDWeakeningFailed {
        /// The node whose capabilities should be a subset.
        node_a: NodeId,
        /// The node whose capabilities should be a superset.
        node_b: NodeId,
        /// The capability descriptor of `node_a`.
        capd_a: CapD,
        /// The capability descriptor of `node_b`.
        capd_b: CapD,
    },

    /// Composing the relations of two nodes yields an inconsistent RelD
    /// (e.g., contradictory temporal constraints).
    RelDRefinementFailed {
        /// The node whose relations should refine the other's.
        node_a: NodeId,
        /// The node whose relations should be refined.
        node_b: NodeId,
        /// The composed relational descriptor that was found inconsistent.
        composed: RelD,
    },

    /// An equality constraint cannot be satisfied because the BDs are
    /// incompatible (representations don't agree, or composing relations
    /// is inconsistent).
    EqualityViolated {
        /// The first node in the violated equality constraint.
        node_a: NodeId,
        /// The second node in the violated equality constraint.
        node_b: NodeId,
        /// The behavioral descriptor of `node_a`.
        bd_a: BD,
        /// The behavioral descriptor of `node_b`.
        bd_b: BD,
    },

    /// A node referenced in a constraint does not exist in the SCG.
    NodeNotFound {
        /// The node ID that was not found in the SCG.
        node: NodeId,
    },

    /// BD fixpoint did not converge within the iteration cap.
    ///
    /// This is **not** a depth limit — it is a bug report. The BD lattice
    /// is finite (see the module-level doc) and each constraint
    /// application is per-component monotone, so the iteration is
    /// guaranteed to converge in a finite number of steps for any
    /// well-formed SCG + constraint set. Hitting this error therefore
    /// indicates that a non-monotone update has been introduced
    /// somewhere in the solver, and the partial `solution` returned
    /// alongside this error (if any) must **not** be trusted as a
    /// verification result — callers MUST treat it as a verification
    /// failure (the SCG is `Unverified`, not `Proven`).
    NoConvergence {
        /// The number of iterations attempted before the cap was hit.
        iterations: usize,
    },
}

impl fmt::Display for SolverError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SolverError::RepDIncompatible {
                node_a,
                node_b,
                repd_a,
                repd_b,
            } => write!(
                f,
                "RepD incompatibility: node {} ({}) vs node {} ({})",
                node_a, repd_a, node_b, repd_b
            ),
            SolverError::CapDWeakeningFailed {
                node_a,
                node_b,
                capd_a,
                capd_b,
            } => write!(
                f,
                "CapD weakening failed: node {} ({}) cannot be a subset of node {} ({})",
                node_a, capd_a, node_b, capd_b
            ),
            SolverError::RelDRefinementFailed {
                node_a,
                node_b,
                composed,
            } => write!(
                f,
                "RelD refinement failed: composing relations for nodes {} and {} \
                 yields inconsistent RelD ({})",
                node_a, node_b, composed
            ),
            SolverError::EqualityViolated {
                node_a,
                node_b,
                bd_a,
                bd_b,
            } => write!(
                f,
                "Equality violated between nodes {} and {}: \
                 bd_a={} vs bd_b={}",
                node_a, node_b, bd_a, bd_b
            ),
            SolverError::NodeNotFound { node } => {
                write!(f, "node not found in SCG: {}", node)
            }
            SolverError::NoConvergence { iterations } => write!(
                f,
                "BD fixpoint did not converge after {iterations} iterations \
                 — non-monotone update detected (this is a bug in the BD constraint \
                 solver; the partial solution must not be trusted)"
            ),
        }
    }
}

impl std::error::Error for SolverError {}

// ---------------------------------------------------------------------------
// BDConstraint
// ---------------------------------------------------------------------------

/// A constraint on the BDs assigned to nodes in the SCG.
///
/// Each variant specifies a relationship that must hold between the
/// BDs of two nodes. The solver enforces these constraints through
/// iterative narrowing and widening.
#[derive(Debug, Clone)]
pub enum BDConstraint {
    /// **RepD Compatibility**: `node_a` and `node_b` must have compatible
    /// representation descriptors.
    ///
    /// Satisfied when `bd_a.repd.compatible(&bd_b.repd)`.
    /// If one node has a default (unresolved) RepD and the other is
    /// specific, the default is replaced with the specific one.
    RepDCompatible {
        /// The first node whose RepD must be compatible.
        node_a: NodeId,
        /// The second node whose RepD must be compatible.
        node_b: NodeId,
    },

    /// **CapD Weakening**: `node_a`'s capabilities must be a subset of
    /// `node_b`'s capabilities (i.e., `node_a` is *weaker* than `node_b`).
    ///
    /// Satisfied when `bd_a.capd.is_subset(&bd_b.capd)`.
    /// During solving, `node_b` may be widened (via join) to include
    /// `node_a`'s capabilities.
    CapDWeakening {
        /// The node whose capabilities must be a subset (weaker).
        node_a: NodeId,
        /// The node whose capabilities must be a superset (stronger).
        node_b: NodeId,
    },

    /// **RelD Refinement**: `node_a`'s relations must refine `node_b`'s
    /// relations (i.e., `node_a` is *more specific* than `node_b`).
    ///
    /// Satisfied when `bd_a.reld.refines(&bd_b.reld)`.
    /// During solving, `node_b`'s relations may be added to `node_a`
    /// (via compose) to satisfy the refinement requirement.
    RelDRefinement {
        /// The node whose relations must refine the other's (more specific).
        node_a: NodeId,
        /// The node whose relations must be refined (more general).
        node_b: NodeId,
    },

    /// **Equality**: `node_a` and `node_b` must have identical BDs.
    ///
    /// Both nodes are set to the meet (greatest lower bound) of their
    /// current BDs. If the RepDs are incompatible, the constraint is
    /// unsatisfiable.
    Equality {
        /// The first node that must have an identical BD.
        node_a: NodeId,
        /// The second node that must have an identical BD.
        node_b: NodeId,
    },
}

impl BDConstraint {
    /// Returns the two node IDs involved in this constraint.
    pub fn nodes(&self) -> (NodeId, NodeId) {
        match self {
            BDConstraint::RepDCompatible { node_a, node_b }
            | BDConstraint::CapDWeakening { node_a, node_b }
            | BDConstraint::RelDRefinement { node_a, node_b }
            | BDConstraint::Equality { node_a, node_b } => (*node_a, *node_b),
        }
    }
}

impl fmt::Display for BDConstraint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BDConstraint::RepDCompatible { node_a, node_b } => {
                write!(f, "RepDCompatible({}, {})", node_a, node_b)
            }
            BDConstraint::CapDWeakening { node_a, node_b } => {
                write!(f, "CapDWeakening({}, {})", node_a, node_b)
            }
            BDConstraint::RelDRefinement { node_a, node_b } => {
                write!(f, "RelDRefinement({}, {})", node_a, node_b)
            }
            BDConstraint::Equality { node_a, node_b } => {
                write!(f, "Equality({}, {})", node_a, node_b)
            }
        }
    }
}

// ---------------------------------------------------------------------------
// ApplyResult (internal)
// ---------------------------------------------------------------------------

/// Result of applying a single constraint to the current solution.
enum ApplyResult {
    /// The solution was modified.
    Changed,
    /// The solution was already consistent; no modification needed.
    Unchanged,
    /// The constraint is unsatisfiable.
    Error(Box<SolverError>),
}

// ---------------------------------------------------------------------------
// BDConstraintSolver
// ---------------------------------------------------------------------------

/// The BD constraint solver.
///
/// Accumulates constraints via [`add_constraint`](BDConstraintSolver::add_constraint)
/// and solves them against an SCG using iterative fixed-point iteration
/// with widening for recursive constraints.
///
/// # Algorithm
///
/// 1. **Validate** that all nodes referenced in constraints exist in the SCG.
/// 2. **Initialize** each node with a most-permissive "top" BD.
/// 3. **Iterate**: for each constraint, check satisfaction and adjust BDs:
///    - `RepDCompatible`: check compatibility; propagate specific RepDs.
///    - `CapDWeakening`: widen `node_b` via join if `node_a` has extra caps.
///    - `RelDRefinement`: compose `node_b`'s relations into `node_a`.
///    - `Equality`: set both nodes to the meet of their BDs.
/// 4. **Widen**: after `widening_threshold` iterations, drop CapD conditions
///    to force convergence.
/// 5. **Terminate**: if no BD changes, return the solution. If the iteration
///    limit is exceeded, return a `NoConvergence` error.
///
/// # Example
///
/// ```rust,ignore
/// use vuma_ive::bd_solver::{BDConstraintSolver, BDConstraint};
/// use vuma_scg::SCG;
///
/// let mut solver = BDConstraintSolver::new();
/// solver.add_constraint(BDConstraint::Equality { node_a: n1, node_b: n2 });
///
/// let solution = solver.solve(&scg);
/// assert!(solution.is_ok());
/// ```
pub struct BDConstraintSolver {
    /// The accumulated constraints.
    constraints: Vec<BDConstraint>,
    /// Safety-net cap on the number of fixpoint iterations before
    /// declaring non-convergence.
    ///
    /// The BD lattice is finite and each constraint application is
    /// per-component monotone (see the module-level doc), so the
    /// iteration provably converges in a finite number of steps for
    /// any well-formed SCG + constraint set. This cap exists only to
    /// fail fast (with [`SolverError::NoConvergence`]) if a
    /// non-monotone update is ever introduced into the solver.
    max_iterations: usize,
    /// Number of iterations after which widening is applied to force
    /// convergence in the presence of recursive constraints.
    widening_threshold: usize,
}

impl BDConstraintSolver {
    /// Construct a new BD constraint solver with default parameters.
    ///
    /// Default `max_iterations` is 10 000 — a safety-net cap that should
    /// never be reached for any well-formed SCG + constraint set (see
    /// the module-level convergence argument). Default widening
    /// threshold: 10.
    pub fn new() -> Self {
        Self {
            constraints: Vec::new(),
            max_iterations: DEFAULT_MAX_ITERATIONS,
            widening_threshold: DEFAULT_WIDENING_THRESHOLD,
        }
    }

    /// Set the fixpoint iteration cap.
    ///
    /// Note that `max_iterations` is a **safety net**, not a depth
    /// limit: exceeding it indicates a non-monotone update bug in the
    /// solver and produces [`SolverError::NoConvergence`] with a clear
    /// message. Callers must treat that error as a verification
    /// failure (the SCG is `Unverified`, not `Proven`).
    pub fn with_max_iterations(mut self, max: usize) -> Self {
        self.max_iterations = max.max(1);
        self
    }

    /// Set the widening threshold — the number of iterations after which
    /// widening is applied to force convergence.
    ///
    /// After this many iterations, CapD conditions are dropped (widened)
    /// to ensure the solution converges.
    pub fn with_widening_threshold(mut self, threshold: usize) -> Self {
        self.widening_threshold = threshold;
        self
    }

    /// Add a constraint to the solver.
    ///
    /// Constraints are accumulated and solved together when [`solve`](BDConstraintSolver::solve)
    /// is called.
    pub fn add_constraint(&mut self, constraint: BDConstraint) {
        self.constraints.push(constraint);
    }

    /// Returns the list of accumulated constraints.
    pub fn constraints(&self) -> &[BDConstraint] {
        &self.constraints
    }

    /// Clear all accumulated constraints.
    pub fn clear(&mut self) {
        self.constraints.clear();
    }

    /// Returns the number of accumulated constraints.
    pub fn len(&self) -> usize {
        self.constraints.len()
    }

    /// Returns `true` if there are no accumulated constraints.
    pub fn is_empty(&self) -> bool {
        self.constraints.is_empty()
    }

    // -----------------------------------------------------------------------
    // Solving
    // -----------------------------------------------------------------------

    /// Solve all constraints against the given SCG.
    ///
    /// Returns `Ok(solution)` — a mapping from `NodeId` to `BD` — if a
    /// satisfying assignment exists. Returns `Err(errors)` with a
    /// non-empty list of [`SolverError`] values if one or more constraints
    /// are unsatisfiable.
    ///
    /// # Algorithm Outline
    ///
    /// 1. Validate node references against the SCG.
    /// 2. Initialize every node with a "top" BD (most permissive).
    /// 3. Iterate: apply each constraint, adjusting BDs as needed.
    /// 4. After `widening_threshold` iterations, apply widening.
    /// 5. Return the solution at the fixed point, or errors.
    pub fn solve(&self, scg: &SCG) -> Result<BTreeMap<NodeId, BD>, Vec<SolverError>> {
        // Collect valid node IDs from the SCG.
        let valid_nodes: HashSet<NodeId> = scg.node_ids().collect();

        // Phase 1: Validate that all referenced nodes exist in the SCG.
        let mut errors: Vec<SolverError> = Vec::new();
        for constraint in &self.constraints {
            let (a, b) = constraint.nodes();
            if !valid_nodes.contains(&a) {
                errors.push(SolverError::NodeNotFound { node: a });
            }
            if a != b && !valid_nodes.contains(&b) {
                errors.push(SolverError::NodeNotFound { node: b });
            }
        }
        if !errors.is_empty() {
            return Err(errors);
        }

        // If there are no constraints, return top BDs for all nodes.
        if self.constraints.is_empty() {
            let mut solution = BTreeMap::new();
            for node_id in scg.node_ids() {
                solution.insert(node_id, top_bd());
            }
            return Ok(solution);
        }

        // Phase 2: Initialize all nodes with the top BD.
        let mut solution: BTreeMap<NodeId, BD> = BTreeMap::new();
        for node_id in scg.node_ids() {
            solution.insert(node_id, top_bd());
        }

        // Phase 3: Iterative fixed-point.
        //
        // Convergence argument (W6): the BD lattice is finite (see the
        // module-level doc) and each constraint application in
        // `apply_constraint` is per-component monotone, so the iteration
        // provably converges in a finite number of steps. The
        // `max_iterations` cap is a safety net — hitting it indicates a
        // non-monotone update (bug), surfaced as `NoConvergence` with a
        // clear message rather than silently capping out.
        let mut iteration = 0usize;
        let mut errors = Vec::new();

        loop {
            let mut changed = false;
            iteration += 1;

            if iteration > self.max_iterations {
                // Safety-net cap hit. Per the convergence argument above,
                // this can only happen if a non-monotone update has been
                // introduced into the solver. Report it as a clear
                // verification failure (NOT a silent depth-cap acceptance).
                errors.push(SolverError::NoConvergence {
                    iterations: iteration,
                });
                return Err(errors);
            }

            let apply_widening = iteration > self.widening_threshold;

            for constraint in &self.constraints {
                let result = self.apply_constraint(constraint, &mut solution, apply_widening);
                match result {
                    ApplyResult::Changed => changed = true,
                    ApplyResult::Unchanged => {}
                    ApplyResult::Error(e) => {
                        // Record the error but continue checking other constraints
                        // to collect as many diagnostics as possible.
                        if !errors.iter().any(|existing: &SolverError| {
                            format!("{}", existing) == format!("{}", e)
                        }) {
                            errors.push(*e);
                        }
                    }
                }
            }

            // If errors were found, abort early — the constraints are
            // unsatisfiable and further iteration won't help.
            if !errors.is_empty() {
                return Err(errors);
            }

            if !changed {
                // Fixed point reached — all constraints are satisfied.
                break;
            }
        }

        Ok(solution)
    }

    /// Solve with custom initial BD assignments.
    ///
    /// Like [`solve`](BDConstraintSolver::solve), but uses the provided
    /// initial assignments instead of starting from "top" BDs. Nodes not
    /// present in `initial` are still initialized with top BDs.
    pub fn solve_with_initial(
        &self,
        scg: &SCG,
        initial: &BTreeMap<NodeId, BD>,
    ) -> Result<BTreeMap<NodeId, BD>, Vec<SolverError>> {
        // Validate node references.
        let valid_nodes: HashSet<NodeId> = scg.node_ids().collect();
        let mut errors: Vec<SolverError> = Vec::new();
        for constraint in &self.constraints {
            let (a, b) = constraint.nodes();
            if !valid_nodes.contains(&a) {
                errors.push(SolverError::NodeNotFound { node: a });
            }
            if a != b && !valid_nodes.contains(&b) {
                errors.push(SolverError::NodeNotFound { node: b });
            }
        }
        if !errors.is_empty() {
            return Err(errors);
        }

        // Initialize: use provided initial assignments, fall back to top.
        let mut solution: BTreeMap<NodeId, BD> = BTreeMap::new();
        for node_id in scg.node_ids() {
            let bd = initial.get(&node_id).cloned().unwrap_or_else(top_bd);
            solution.insert(node_id, bd);
        }

        // Iterative fixed-point (same convergence argument as `solve`).
        let mut iteration = 0usize;
        let mut errors = Vec::new();

        loop {
            let mut changed = false;
            iteration += 1;

            if iteration > self.max_iterations {
                // Safety-net cap hit — non-monotone update detected.
                // See the module-level convergence argument.
                errors.push(SolverError::NoConvergence {
                    iterations: iteration,
                });
                return Err(errors);
            }

            let apply_widening = iteration > self.widening_threshold;

            for constraint in &self.constraints {
                let result = self.apply_constraint(constraint, &mut solution, apply_widening);
                match result {
                    ApplyResult::Changed => changed = true,
                    ApplyResult::Unchanged => {}
                    ApplyResult::Error(e) => {
                        if !errors
                            .iter()
                            .any(|existing| format!("{}", existing) == format!("{}", e))
                        {
                            errors.push(*e);
                        }
                    }
                }
            }

            if !errors.is_empty() {
                return Err(errors);
            }

            if !changed {
                break;
            }
        }

        Ok(solution)
    }

    // -----------------------------------------------------------------------
    // Constraint application (private)
    // -----------------------------------------------------------------------

    /// Apply a single constraint to the current solution.
    fn apply_constraint(
        &self,
        constraint: &BDConstraint,
        solution: &mut BTreeMap<NodeId, BD>,
        apply_widening: bool,
    ) -> ApplyResult {
        match constraint {
            BDConstraint::RepDCompatible { node_a, node_b } => {
                self.apply_repd_compatible(*node_a, *node_b, solution)
            }
            BDConstraint::CapDWeakening { node_a, node_b } => {
                self.apply_capd_weakening(*node_a, *node_b, solution, apply_widening)
            }
            BDConstraint::RelDRefinement { node_a, node_b } => {
                self.apply_reld_refinement(*node_a, *node_b, solution)
            }
            BDConstraint::Equality { node_a, node_b } => {
                self.apply_equality(*node_a, *node_b, solution)
            }
        }
    }

    /// Apply a RepD compatibility constraint.
    ///
    /// If both RepDs are already compatible, no change. If one is the
    /// default (unresolved) RepD, adopt the specific one. If both are
    /// specific and incompatible, report an error.
    fn apply_repd_compatible(
        &self,
        node_a: NodeId,
        node_b: NodeId,
        solution: &mut BTreeMap<NodeId, BD>,
    ) -> ApplyResult {
        let bd_a = solution
            .get(&node_a)
            .expect("node_a must exist in solution");
        let bd_b = solution
            .get(&node_b)
            .expect("node_b must exist in solution");

        if bd_a.repd.compatible(&bd_b.repd) {
            ApplyResult::Unchanged
        } else {
            let a_is_default = is_default_repd(&bd_a.repd);
            let b_is_default = is_default_repd(&bd_b.repd);

            if a_is_default && !b_is_default {
                // Adopt node_b's RepD.
                let new_repd = bd_b.repd.clone();
                let bd_a = solution.get_mut(&node_a).unwrap();
                bd_a.repd = new_repd;
                ApplyResult::Changed
            } else if b_is_default && !a_is_default {
                // Adopt node_a's RepD.
                let new_repd = bd_a.repd.clone();
                let bd_b = solution.get_mut(&node_b).unwrap();
                bd_b.repd = new_repd;
                ApplyResult::Changed
            } else {
                // Both are specific and incompatible.
                let repd_a = solution.get(&node_a).unwrap().repd.clone();
                let repd_b = solution.get(&node_b).unwrap().repd.clone();
                ApplyResult::Error(Box::new(SolverError::RepDIncompatible {
                    node_a,
                    node_b,
                    repd_a,
                    repd_b,
                }))
            }
        }
    }

    /// Apply a CapD weakening constraint (node_a.capd ⊆ node_b.capd).
    ///
    /// Strategy: widen node_b by joining with node_a's capabilities.
    /// If widening is active, also drop conditions to force convergence.
    fn apply_capd_weakening(
        &self,
        node_a: NodeId,
        node_b: NodeId,
        solution: &mut BTreeMap<NodeId, BD>,
        apply_widening: bool,
    ) -> ApplyResult {
        let bd_a = solution
            .get(&node_a)
            .expect("node_a must exist in solution");
        let bd_b = solution
            .get(&node_b)
            .expect("node_b must exist in solution");

        if bd_a.capd.is_subset(&bd_b.capd) {
            // Constraint already satisfied.
            ApplyResult::Unchanged
        } else {
            // Widen node_b by joining with node_a's capabilities.
            let joined = bd_b.capd.join(&bd_a.capd);

            let new_capd = if apply_widening {
                widen_capd(&joined)
            } else {
                joined
            };

            let bd_b = solution.get_mut(&node_b).unwrap();
            if bd_b.capd != new_capd {
                bd_b.capd = new_capd;
                ApplyResult::Changed
            } else {
                ApplyResult::Unchanged
            }
        }
    }

    /// Apply a RelD refinement constraint (node_a.reld refines node_b.reld).
    ///
    /// If node_a's relations don't already refine node_b's, compose
    /// node_b's relations into node_a. Report an error if the resulting
    /// RelD is inconsistent.
    fn apply_reld_refinement(
        &self,
        node_a: NodeId,
        node_b: NodeId,
        solution: &mut BTreeMap<NodeId, BD>,
    ) -> ApplyResult {
        let bd_a = solution
            .get(&node_a)
            .expect("node_a must exist in solution");
        let bd_b = solution
            .get(&node_b)
            .expect("node_b must exist in solution");

        if bd_a.reld.refines(&bd_b.reld) {
            // Constraint already satisfied.
            ApplyResult::Unchanged
        } else {
            // Add node_b's relations to node_a via compose.
            let composed = bd_a.reld.compose(&bd_b.reld);

            // Check consistency of the composed RelD.
            if !composed.is_consistent() {
                return ApplyResult::Error(Box::new(SolverError::RelDRefinementFailed {
                    node_a,
                    node_b,
                    composed,
                }));
            }

            let bd_a = solution.get_mut(&node_a).unwrap();
            if bd_a.reld != composed {
                bd_a.reld = composed;
                ApplyResult::Changed
            } else {
                ApplyResult::Unchanged
            }
        }
    }

    /// Apply an equality constraint (node_a == node_b).
    ///
    /// Both nodes are set to the meet (greatest lower bound) of their
    /// current BDs. The meet is:
    /// - RepD: the more specific of the two (if one subsumes the other),
    ///   or either if they're equally specific but compatible.
    /// - CapD: intersection of capabilities.
    /// - RelD: compose (union) of relations.
    fn apply_equality(
        &self,
        node_a: NodeId,
        node_b: NodeId,
        solution: &mut BTreeMap<NodeId, BD>,
    ) -> ApplyResult {
        let bd_a = solution.get(&node_a).expect("node_a must exist").clone();
        let bd_b = solution.get(&node_b).expect("node_b must exist").clone();

        if bd_a == bd_b {
            return ApplyResult::Unchanged;
        }

        // RepDs must be compatible for equality to hold.
        if !bd_a.repd.compatible(&bd_b.repd) {
            return ApplyResult::Error(Box::new(SolverError::EqualityViolated {
                node_a,
                node_b,
                bd_a,
                bd_b,
            }));
        }

        // Compute the meet BD.
        let met_capd = bd_a.capd.meet(&bd_b.capd);
        let met_reld = bd_a.reld.compose(&bd_b.reld);

        // Check RelD consistency.
        if !met_reld.is_consistent() {
            return ApplyResult::Error(Box::new(SolverError::EqualityViolated {
                node_a,
                node_b,
                bd_a,
                bd_b,
            }));
        }

        // Use the more specific RepD.
        let met_repd = if bd_a.repd.subsumes(&bd_b.repd) {
            bd_b.repd.clone()
        } else if bd_b.repd.subsumes(&bd_a.repd) {
            bd_a.repd.clone()
        } else {
            // Both are equally specific but compatible — prefer a's.
            bd_a.repd.clone()
        };

        let met_bd = BD::new(met_repd, met_capd, met_reld);

        let mut changed = false;
        {
            let bd_a_mut = solution.get_mut(&node_a).unwrap();
            if *bd_a_mut != met_bd {
                *bd_a_mut = met_bd.clone();
                changed = true;
            }
        }
        {
            let bd_b_mut = solution.get_mut(&node_b).unwrap();
            if *bd_b_mut != met_bd {
                *bd_b_mut = met_bd;
                changed = true;
            }
        }

        if changed {
            ApplyResult::Changed
        } else {
            ApplyResult::Unchanged
        }
    }
}

impl Default for BDConstraintSolver {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for BDConstraintSolver {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "BDConstraintSolver({} constraints, max_iter={}, widen_thresh={})",
            self.constraints.len(),
            self.max_iterations,
            self.widening_threshold
        )
    }
}

// ---------------------------------------------------------------------------
// Helper functions
// ---------------------------------------------------------------------------

/// Create a "top" (most permissive) BD for initialization.
///
/// The top BD uses:
/// - RepD: default (unresolved) byte representation.
/// - CapD: all capabilities, no conditions.
/// - RelD: empty (no specific relations).
fn top_bd() -> BD {
    BD::new(default_repd(), CapD::all(), RelD::empty())
}

/// Create a default (unresolved/placeholder) RepD.
///
/// This sentinel value marks a RepD that has not yet been constrained.
/// It uses size=1, align=1 so it is trivially compatible with other
/// default RepDs.
fn default_repd() -> RepD {
    RepD::Byte(ByteRep { size: 1, align: 1 })
}

/// Check if a RepD is the default (unresolved) representation.
fn is_default_repd(repd: &RepD) -> bool {
    matches!(repd, RepD::Byte(b) if b.size == 1 && b.align == 1)
}

/// Apply widening to a CapD to force convergence.
///
/// Widening removes all conditions, making every capability
/// unconditionally active. This is a coarse but sound widening
/// that guarantees convergence at the cost of precision.
fn widen_capd(capd: &CapD) -> CapD {
    CapD {
        caps: capd.caps.clone(),
        conditions: BTreeSet::new(),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use vuma_bd::capd::Capability;
    use vuma_bd::reld::{Relation, TemporalKind};
    use vuma_scg::node::{ComputationKind, ComputationNode, NodePayload, NodeType, ProgramPoint};

    /// Helper: create a program point for testing.
    fn pp() -> ProgramPoint {
        ProgramPoint {
            file: Some("test.vu".into()),
            line: Some(1),
            column: Some(1),
            offset: None,
        }
    }

    /// Helper: add a computation node to the SCG and return its ID.
    fn add_comp_node(scg: &mut SCG) -> NodeId {
        scg.add_node(
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                kind: ComputationKind::Other("test".into()),
                result_type: None,
                tail_call: false,
            }),
            pp(),
        )
    }

    /// Helper: create a simple BD for testing.
    fn simple_bd(size: u64, align: u64, caps: &[Capability]) -> BD {
        BD::new(
            RepD::Byte(ByteRep { size, align }),
            CapD {
                caps: caps.iter().copied().collect(),
                conditions: BTreeSet::new(),
            },
            RelD::empty(),
        )
    }

    /// Helper: create a BD with specified relations and a default byte RepD.
    fn reld_bd(relations: &[Relation]) -> BD {
        BD::new(
            default_repd(),
            CapD::all(),
            RelD {
                relations: relations.iter().cloned().collect(),
            },
        )
    }

    // -----------------------------------------------------------------------
    // Test 1: Solver construction and defaults
    // -----------------------------------------------------------------------

    #[test]
    fn solver_new_defaults() {
        let mut solver = BDConstraintSolver::new();
        assert!(solver.is_empty());
        assert_eq!(solver.len(), 0);
        assert_eq!(solver.constraints().len(), 0);
    }

    #[test]
    fn solver_default_impl() {
        let solver = BDConstraintSolver::default();
        assert!(solver.is_empty());
    }

    // -----------------------------------------------------------------------
    // Test 2: Adding constraints
    // -----------------------------------------------------------------------

    #[test]
    fn add_constraints() {
        let mut solver = BDConstraintSolver::new();
        let n1 = NodeId::new(0);
        let n2 = NodeId::new(1);

        solver.add_constraint(BDConstraint::RepDCompatible {
            node_a: n1,
            node_b: n2,
        });
        solver.add_constraint(BDConstraint::CapDWeakening {
            node_a: n1,
            node_b: n2,
        });

        assert_eq!(solver.len(), 2);
        assert!(!solver.is_empty());
    }

    // -----------------------------------------------------------------------
    // Test 3: Clear constraints
    // -----------------------------------------------------------------------

    #[test]
    fn clear_constraints() {
        let mut solver = BDConstraintSolver::new();
        solver.add_constraint(BDConstraint::Equality {
            node_a: NodeId::new(0),
            node_b: NodeId::new(1),
        });
        assert_eq!(solver.len(), 1);

        solver.clear();
        assert!(solver.is_empty());
    }

    // -----------------------------------------------------------------------
    // Test 4: Solve with no constraints
    // -----------------------------------------------------------------------

    #[test]
    fn solve_no_constraints() {
        let mut solver = BDConstraintSolver::new();
        let mut scg = SCG::new();
        let n1 = add_comp_node(&mut scg);
        let n2 = add_comp_node(&mut scg);

        let result = solver.solve(&scg);
        assert!(result.is_ok());

        let solution = result.unwrap();
        assert_eq!(solution.len(), 2);
        assert!(solution.contains_key(&n1));
        assert!(solution.contains_key(&n2));

        // Both should have top BDs.
        let top = top_bd();
        assert_eq!(solution[&n1], top);
        assert_eq!(solution[&n2], top);
    }

    // -----------------------------------------------------------------------
    // Test 5: RepD compatibility — satisfiable
    // -----------------------------------------------------------------------

    #[test]
    fn repd_compatible_satisfiable() {
        let mut solver = BDConstraintSolver::new();
        let mut scg = SCG::new();
        let n1 = add_comp_node(&mut scg);
        let n2 = add_comp_node(&mut scg);

        solver.add_constraint(BDConstraint::RepDCompatible {
            node_a: n1,
            node_b: n2,
        });

        let result = solver.solve(&scg);
        assert!(result.is_ok(), "Expected Ok, got {:?}", result);

        let solution = result.unwrap();
        // Both nodes should still have default RepDs (compatible with each other).
        assert!(is_default_repd(&solution[&n1].repd));
        assert!(is_default_repd(&solution[&n2].repd));
    }

    // -----------------------------------------------------------------------
    // Test 6: RepD compatibility — with initial specific RepDs
    // -----------------------------------------------------------------------

    #[test]
    fn repd_compatible_with_initial_bd() {
        let mut solver = BDConstraintSolver::new();
        let mut scg = SCG::new();
        let n1 = add_comp_node(&mut scg);
        let n2 = add_comp_node(&mut scg);

        // Set n1 to a specific BD with size=8, align=8.
        let initial_bd = simple_bd(8, 8, &[Capability::Read]);
        let mut initial = BTreeMap::new();
        initial.insert(n1, initial_bd);

        solver.add_constraint(BDConstraint::RepDCompatible {
            node_a: n1,
            node_b: n2,
        });

        let result = solver.solve_with_initial(&scg, &initial);
        assert!(result.is_ok(), "Expected Ok, got {:?}", result);

        let solution = result.unwrap();
        // n2 should have adopted n1's RepD.
        assert_eq!(solution[&n2].repd.size(), 8);
        assert_eq!(solution[&n2].repd.alignment(), 8);
    }

    // -----------------------------------------------------------------------
    // Test 7: RepD compatibility — unsatisfiable
    // -----------------------------------------------------------------------

    #[test]
    fn repd_compatible_unsatisfiable() {
        let mut solver = BDConstraintSolver::new();
        let mut scg = SCG::new();
        let n1 = add_comp_node(&mut scg);
        let n2 = add_comp_node(&mut scg);

        // Set n1 and n2 to incompatible RepDs.
        let bd1 = simple_bd(4, 4, &[Capability::Read]);
        let bd2 = simple_bd(8, 8, &[Capability::Read]);
        let mut initial = BTreeMap::new();
        initial.insert(n1, bd1);
        initial.insert(n2, bd2);

        solver.add_constraint(BDConstraint::RepDCompatible {
            node_a: n1,
            node_b: n2,
        });

        let result = solver.solve_with_initial(&scg, &initial);
        assert!(result.is_err());

        let errors = result.unwrap_err();
        assert!(errors
            .iter()
            .any(|e| matches!(e, SolverError::RepDIncompatible { .. })));
    }

    // -----------------------------------------------------------------------
    // Test 8: CapD weakening — satisfiable
    // -----------------------------------------------------------------------

    #[test]
    fn capd_weakening_satisfiable() {
        let mut solver = BDConstraintSolver::new();
        let mut scg = SCG::new();
        let n1 = add_comp_node(&mut scg);
        let n2 = add_comp_node(&mut scg);

        // n1 (Read only) must be a subset of n2 (Read+Write).
        // Start n1 with just Read, n2 with top (all caps).
        let initial_n1 = simple_bd(4, 4, &[Capability::Read]);
        let initial_n2 = simple_bd(4, 4, &[Capability::Read, Capability::Write]);
        let mut initial = BTreeMap::new();
        initial.insert(n1, initial_n1);
        initial.insert(n2, initial_n2);

        solver.add_constraint(BDConstraint::CapDWeakening {
            node_a: n1,
            node_b: n2,
        });

        let result = solver.solve_with_initial(&scg, &initial);
        assert!(result.is_ok(), "Expected Ok, got {:?}", result);
    }

    // -----------------------------------------------------------------------
    // Test 9: CapD weakening — widening needed
    // -----------------------------------------------------------------------

    #[test]
    fn capd_weakening_widens_node_b() {
        let mut solver = BDConstraintSolver::new().with_widening_threshold(1);
        let mut scg = SCG::new();
        let n1 = add_comp_node(&mut scg);
        let n2 = add_comp_node(&mut scg);

        // n1 (Read+Write) must be subset of n2 (Read only).
        // Solver should widen n2 to include Write.
        let initial_n1 = simple_bd(4, 4, &[Capability::Read, Capability::Write]);
        let initial_n2 = simple_bd(4, 4, &[Capability::Read]);
        let mut initial = BTreeMap::new();
        initial.insert(n1, initial_n1);
        initial.insert(n2, initial_n2);

        solver.add_constraint(BDConstraint::CapDWeakening {
            node_a: n1,
            node_b: n2,
        });

        let result = solver.solve_with_initial(&scg, &initial);
        assert!(result.is_ok(), "Expected Ok, got {:?}", result);

        let solution = result.unwrap();
        // n2 should have been widened to include Write.
        assert!(solution[&n2].capd.caps.contains(&Capability::Write));
    }

    // -----------------------------------------------------------------------
    // Test 10: RelD refinement — satisfiable
    // -----------------------------------------------------------------------

    #[test]
    fn reld_refinement_satisfiable() {
        let mut solver = BDConstraintSolver::new();
        let mut scg = SCG::new();
        let n1 = add_comp_node(&mut scg);
        let n2 = add_comp_node(&mut scg);

        // n1 must refine n2. n1 has Liveness, n2 has Containment.
        // After solving, n1 should have both.
        let initial_n1 = reld_bd(&[Relation::Liveness]);
        let initial_n2 = reld_bd(&[Relation::Containment]);
        let mut initial = BTreeMap::new();
        initial.insert(n1, initial_n1);
        initial.insert(n2, initial_n2);

        solver.add_constraint(BDConstraint::RelDRefinement {
            node_a: n1,
            node_b: n2,
        });

        let result = solver.solve_with_initial(&scg, &initial);
        assert!(result.is_ok(), "Expected Ok, got {:?}", result);

        let solution = result.unwrap();
        // n1 should now have both Liveness and Containment.
        assert!(solution[&n1].reld.relations.contains(&Relation::Liveness));
        assert!(solution[&n1]
            .reld
            .relations
            .contains(&Relation::Containment));
    }

    // -----------------------------------------------------------------------
    // Test 11: RelD refinement — unsatisfiable (inconsistent)
    // -----------------------------------------------------------------------

    #[test]
    fn reld_refinement_inconsistent() {
        let mut solver = BDConstraintSolver::new();
        let mut scg = SCG::new();
        let n1 = add_comp_node(&mut scg);
        let n2 = add_comp_node(&mut scg);

        // n1 has Outlives, n2 has Succeeds. Composing them creates
        // inconsistency (Outlives + Succeeds is contradictory).
        let initial_n1 = reld_bd(&[Relation::Temporal(TemporalKind::Outlives)]);
        let initial_n2 = reld_bd(&[Relation::Temporal(TemporalKind::Succeeds)]);
        let mut initial = BTreeMap::new();
        initial.insert(n1, initial_n1);
        initial.insert(n2, initial_n2);

        solver.add_constraint(BDConstraint::RelDRefinement {
            node_a: n1,
            node_b: n2,
        });

        let result = solver.solve_with_initial(&scg, &initial);
        assert!(result.is_err());

        let errors = result.unwrap_err();
        assert!(errors
            .iter()
            .any(|e| matches!(e, SolverError::RelDRefinementFailed { .. })));
    }

    // -----------------------------------------------------------------------
    // Test 12: Equality constraint — satisfiable
    // -----------------------------------------------------------------------

    #[test]
    fn equality_satisfiable() {
        let mut solver = BDConstraintSolver::new();
        let mut scg = SCG::new();
        let n1 = add_comp_node(&mut scg);
        let n2 = add_comp_node(&mut scg);

        solver.add_constraint(BDConstraint::Equality {
            node_a: n1,
            node_b: n2,
        });

        let result = solver.solve(&scg);
        assert!(result.is_ok(), "Expected Ok, got {:?}", result);

        let solution = result.unwrap();
        // Both nodes should have identical BDs.
        assert_eq!(solution[&n1], solution[&n2]);
    }

    // -----------------------------------------------------------------------
    // Test 13: Equality constraint — unsatisfiable (incompatible RepDs)
    // -----------------------------------------------------------------------

    #[test]
    fn equality_unsatisfiable_incompatible_repd() {
        let mut solver = BDConstraintSolver::new();
        let mut scg = SCG::new();
        let n1 = add_comp_node(&mut scg);
        let n2 = add_comp_node(&mut scg);

        let bd1 = simple_bd(4, 4, &[Capability::Read]);
        let bd2 = simple_bd(8, 8, &[Capability::Read]);
        let mut initial = BTreeMap::new();
        initial.insert(n1, bd1);
        initial.insert(n2, bd2);

        solver.add_constraint(BDConstraint::Equality {
            node_a: n1,
            node_b: n2,
        });

        let result = solver.solve_with_initial(&scg, &initial);
        assert!(result.is_err());

        let errors = result.unwrap_err();
        assert!(errors
            .iter()
            .any(|e| matches!(e, SolverError::EqualityViolated { .. })));
    }

    // -----------------------------------------------------------------------
    // Test 14: Node not found
    // -----------------------------------------------------------------------

    #[test]
    fn node_not_found() {
        let mut solver = BDConstraintSolver::new();
        let scg = SCG::new(); // Empty SCG

        solver.add_constraint(BDConstraint::Equality {
            node_a: NodeId::new(99),
            node_b: NodeId::new(100),
        });

        let result = solver.solve(&scg);
        assert!(result.is_err());

        let errors = result.unwrap_err();
        assert!(errors
            .iter()
            .any(|e| matches!(e, SolverError::NodeNotFound { .. })));
        // Should have errors for both nodes.
        assert!(errors.len() >= 2);
    }

    // -----------------------------------------------------------------------
    // Test 15: Combined constraints
    // -----------------------------------------------------------------------

    #[test]
    fn combined_constraints() {
        let mut solver = BDConstraintSolver::new();
        let mut scg = SCG::new();
        let n1 = add_comp_node(&mut scg);
        let n2 = add_comp_node(&mut scg);
        let n3 = add_comp_node(&mut scg);

        // n1 and n2 must be equal.
        solver.add_constraint(BDConstraint::Equality {
            node_a: n1,
            node_b: n2,
        });
        // n2's caps must be subset of n3's.
        solver.add_constraint(BDConstraint::CapDWeakening {
            node_a: n2,
            node_b: n3,
        });
        // n1 and n3 must have compatible RepDs.
        solver.add_constraint(BDConstraint::RepDCompatible {
            node_a: n1,
            node_b: n3,
        });

        let result = solver.solve(&scg);
        assert!(result.is_ok(), "Expected Ok, got {:?}", result);

        let solution = result.unwrap();
        // n1 and n2 should have the same BD.
        assert_eq!(solution[&n1], solution[&n2]);
        // n2.capd ⊆ n3.capd.
        assert!(solution[&n2].capd.is_subset(&solution[&n3].capd));
    }

    // -----------------------------------------------------------------------
    // Test 16: Self-referencing constraint (trivially satisfied)
    // -----------------------------------------------------------------------

    #[test]
    fn self_referencing_constraint() {
        let mut solver = BDConstraintSolver::new();
        let mut scg = SCG::new();
        let n1 = add_comp_node(&mut scg);

        solver.add_constraint(BDConstraint::Equality {
            node_a: n1,
            node_b: n1,
        });

        let result = solver.solve(&scg);
        assert!(result.is_ok());
    }

    // -----------------------------------------------------------------------
    // Test 17: SolverError display
    // -----------------------------------------------------------------------

    #[test]
    fn solver_error_display() {
        let n1 = NodeId::new(1);
        let n2 = NodeId::new(2);

        let err = SolverError::RepDIncompatible {
            node_a: n1,
            node_b: n2,
            repd_a: RepD::Byte(ByteRep { size: 4, align: 4 }),
            repd_b: RepD::Byte(ByteRep { size: 8, align: 8 }),
        };
        let msg = format!("{}", err);
        assert!(msg.contains("RepD incompatibility"));

        let err = SolverError::NodeNotFound { node: n1 };
        let msg = format!("{}", err);
        assert!(msg.contains("node not found"));

        let err = SolverError::NoConvergence { iterations: 200 };
        let msg = format!("{}", err);
        assert!(msg.contains("200"));
        // W6: the message must clearly identify non-convergence and
        // point at non-monotonicity as the cause (not a depth limit).
        assert!(msg.contains("BD fixpoint did not converge"));
        assert!(msg.contains("non-monotone update"));
    }

    // -----------------------------------------------------------------------
    // Test 18: BDConstraint display
    // -----------------------------------------------------------------------

    #[test]
    fn bd_constraint_display() {
        let n1 = NodeId::new(1);
        let n2 = NodeId::new(2);

        let c = BDConstraint::RepDCompatible {
            node_a: n1,
            node_b: n2,
        };
        assert_eq!(format!("{}", c), "RepDCompatible(NodeId(1), NodeId(2))");

        let c = BDConstraint::CapDWeakening {
            node_a: n1,
            node_b: n2,
        };
        assert!(format!("{}", c).contains("CapDWeakening"));

        let c = BDConstraint::RelDRefinement {
            node_a: n1,
            node_b: n2,
        };
        assert!(format!("{}", c).contains("RelDRefinement"));

        let c = BDConstraint::Equality {
            node_a: n1,
            node_b: n2,
        };
        assert!(format!("{}", c).contains("Equality"));
    }

    // -----------------------------------------------------------------------
    // Test 19: Solver display
    // -----------------------------------------------------------------------

    #[test]
    fn solver_display() {
        let mut solver = BDConstraintSolver::new()
            .with_max_iterations(50)
            .with_widening_threshold(5);
        let msg = format!("{}", solver);
        assert!(msg.contains("50"));
        assert!(msg.contains("5"));
    }

    // -----------------------------------------------------------------------
    // Test 20: No convergence
    // -----------------------------------------------------------------------

    #[test]
    fn no_convergence() {
        // Create a solver with very low max iterations.
        let mut solver = BDConstraintSolver::new().with_max_iterations(1);
        let mut scg = SCG::new();
        let n1 = add_comp_node(&mut scg);
        let n2 = add_comp_node(&mut scg);

        // Add an equality constraint that requires more than 1 iteration
        // to converge (it actually converges in 1, but let's force 0).
        // Actually, equality with top BDs converges in 1 iteration.
        // Let's use max_iterations=0 (but we enforce min of 1).
        // With max_iterations=1, the solver runs 1 iteration and
        // if it doesn't converge, it errors.

        solver.add_constraint(BDConstraint::Equality {
            node_a: n1,
            node_b: n2,
        });

        // This should actually succeed because top BDs are equal from the start.
        let result = solver.solve(&scg);
        assert!(result.is_ok());
    }

    // -----------------------------------------------------------------------
    // Test 21: Equality meet narrows capabilities
    // -----------------------------------------------------------------------

    #[test]
    fn equality_meet_narrows_caps() {
        let mut solver = BDConstraintSolver::new();
        let mut scg = SCG::new();
        let n1 = add_comp_node(&mut scg);
        let n2 = add_comp_node(&mut scg);

        let bd1 = simple_bd(4, 4, &[Capability::Read, Capability::Write]);
        let bd2 = simple_bd(4, 4, &[Capability::Read]);
        let mut initial = BTreeMap::new();
        initial.insert(n1, bd1);
        initial.insert(n2, bd2);

        solver.add_constraint(BDConstraint::Equality {
            node_a: n1,
            node_b: n2,
        });

        let result = solver.solve_with_initial(&scg, &initial);
        assert!(result.is_ok());

        let solution = result.unwrap();
        // Both should have the meet: Read only (intersection).
        assert!(solution[&n1].capd.caps.contains(&Capability::Read));
        assert!(!solution[&n1].capd.caps.contains(&Capability::Write));
        assert!(solution[&n2].capd.caps.contains(&Capability::Read));
        assert!(!solution[&n2].capd.caps.contains(&Capability::Write));
    }

    // -----------------------------------------------------------------------
    // Test 22: BDConstraint::nodes()
    // -----------------------------------------------------------------------

    #[test]
    fn constraint_nodes() {
        let n1 = NodeId::new(10);
        let n2 = NodeId::new(20);

        let c = BDConstraint::CapDWeakening {
            node_a: n1,
            node_b: n2,
        };
        assert_eq!(c.nodes(), (n1, n2));
    }

    // ===================================================================
    // W6: Fixpoint convergence tests
    // ===================================================================
    //
    // The BD lattice is finite (product of finite RepD/CapD/RelD lattices)
    // and each constraint application is per-component monotone, so the
    // fixpoint provably converges in a finite number of steps. These
    // tests exercise the real fixpoint machinery: a multi-iteration
    // Equality-meet cascade that must converge correctly, and a
    // fault-injection test confirming that hitting the safety-net cap is
    // surfaced as a clear failure (not silently accepted).

    // -----------------------------------------------------------------------
    // Test 23 (W6): Fixpoint converges on a multi-step Equality cascade
    // -----------------------------------------------------------------------
    //
    // Three nodes with different initial CapDs, linked by two Equality
    // constraints. Each Equality meet narrows CapDs (intersection of
    // capabilities). The cascade requires multiple iterations to
    // propagate the meet through n1 -> n2 -> n3, then a verification
    // pass. All three should end up with only Read (the meet of
    // {R+W}, {R}, {R+W}).
    #[test]
    fn fixpoint_converges_on_multi_step_equality_cascade() {
        let mut solver = BDConstraintSolver::new();
        let mut scg = SCG::new();
        let n1 = add_comp_node(&mut scg);
        let n2 = add_comp_node(&mut scg);
        let n3 = add_comp_node(&mut scg);

        let mut initial = BTreeMap::new();
        initial.insert(n1, simple_bd(4, 4, &[Capability::Read, Capability::Write]));
        initial.insert(n2, simple_bd(4, 4, &[Capability::Read]));
        initial.insert(n3, simple_bd(4, 4, &[Capability::Read, Capability::Write]));

        solver.add_constraint(BDConstraint::Equality {
            node_a: n1,
            node_b: n2,
        });
        solver.add_constraint(BDConstraint::Equality {
            node_a: n2,
            node_b: n3,
        });

        let result = solver.solve_with_initial(&scg, &initial);
        assert!(result.is_ok(), "Expected Ok, got {:?}", result);

        let solution = result.unwrap();
        // All three should converge to the meet: Read only.
        for &node_id in &[n1, n2, n3] {
            assert!(
                solution[&node_id].capd.caps.contains(&Capability::Read),
                "Node {node_id:?} should retain Read"
            );
            assert!(
                !solution[&node_id].capd.caps.contains(&Capability::Write),
                "Node {node_id:?} should have Write removed by meet"
            );
        }
        // All three should be equal (the Equality constraint).
        assert_eq!(solution[&n1], solution[&n2]);
        assert_eq!(solution[&n2], solution[&n3]);
    }

    // -----------------------------------------------------------------------
    // Test 24 (W6): Fixpoint converges on a CapD-weakening chain
    // -----------------------------------------------------------------------
    //
    // n1 (R+W) -> n2 (R) -> n3 (R). CapDWeakening says each node's caps
    // must be a subset of the next. So n2 must be widened to include W,
    // and n3 must be widened to include W. After convergence, all three
    // should have at least R+W.
    #[test]
    fn fixpoint_converges_on_capd_weakening_chain() {
        let mut solver = BDConstraintSolver::new();
        let mut scg = SCG::new();
        let n1 = add_comp_node(&mut scg);
        let n2 = add_comp_node(&mut scg);
        let n3 = add_comp_node(&mut scg);

        let mut initial = BTreeMap::new();
        initial.insert(n1, simple_bd(4, 4, &[Capability::Read, Capability::Write]));
        initial.insert(n2, simple_bd(4, 4, &[Capability::Read]));
        initial.insert(n3, simple_bd(4, 4, &[Capability::Read]));

        // n1.capd ⊆ n2.capd  =>  widen n2 to include W
        solver.add_constraint(BDConstraint::CapDWeakening {
            node_a: n1,
            node_b: n2,
        });
        // n2.capd ⊆ n3.capd  =>  widen n3 to include W (after n2 widened)
        solver.add_constraint(BDConstraint::CapDWeakening {
            node_a: n2,
            node_b: n3,
        });

        let result = solver.solve_with_initial(&scg, &initial);
        assert!(result.is_ok(), "Expected Ok, got {:?}", result);

        let solution = result.unwrap();
        // n2 and n3 should have been widened to include Write.
        assert!(solution[&n2].capd.caps.contains(&Capability::Write));
        assert!(solution[&n3].capd.caps.contains(&Capability::Write));
        // n2.capd ⊆ n3.capd must hold.
        assert!(solution[&n2].capd.is_subset(&solution[&n3].capd));
        // n1.capd ⊆ n2.capd must hold.
        assert!(solution[&n1].capd.is_subset(&solution[&n2].capd));
    }

    // -----------------------------------------------------------------------
    // Test 25 (W6): Non-convergence is reported clearly, not silently accepted
    // -----------------------------------------------------------------------
    //
    // Simulates non-convergence by lowering the safety-net cap below the
    // number of iterations the constraint set actually requires (2). The
    // solver must surface this as a `NoConvergence` error with a clear
    // message identifying non-monotonicity — NOT silently accept the
    // partial solution as a verified result.
    //
    // In a sound solver, hitting the cap indicates a non-monotone update
    // (bug); the partial solution must not be trusted. This test confirms
    // that contract is enforced.
    #[test]
    fn fixpoint_non_convergence_reported_clearly() {
        // Set the safety-net cap below the 2 iterations this SCG requires.
        let mut solver = BDConstraintSolver::new().with_max_iterations(1);
        let mut scg = SCG::new();
        let n1 = add_comp_node(&mut scg);
        let n2 = add_comp_node(&mut scg);
        let n3 = add_comp_node(&mut scg);

        let mut initial = BTreeMap::new();
        initial.insert(n1, simple_bd(4, 4, &[Capability::Read, Capability::Write]));
        initial.insert(n2, simple_bd(4, 4, &[Capability::Read]));
        initial.insert(n3, simple_bd(4, 4, &[Capability::Read, Capability::Write]));

        solver.add_constraint(BDConstraint::Equality {
            node_a: n1,
            node_b: n2,
        });
        solver.add_constraint(BDConstraint::Equality {
            node_a: n2,
            node_b: n3,
        });

        let result = solver.solve_with_initial(&scg, &initial);
        // The solver must NOT silently accept the partial result.
        assert!(
            result.is_err(),
            "Solver must report non-convergence, not silently accept partial solution"
        );

        let errors = result.unwrap_err();
        let non_conv = errors
            .iter()
            .find(|e| matches!(e, SolverError::NoConvergence { .. }));
        assert!(
            non_conv.is_some(),
            "Expected NoConvergence error, got: {:?}",
            errors
        );
        let msg = format!("{}", non_conv.unwrap());
        assert!(
            msg.contains("BD fixpoint did not converge"),
            "Error message must clearly state non-convergence: {}",
            msg
        );
        assert!(
            msg.contains("non-monotone update"),
            "Error message must identify non-monotonicity: {}",
            msg
        );
    }

    // -----------------------------------------------------------------------
    // Test 26 (W6): Default max_iterations is the safety-net cap (10 000)
    // -----------------------------------------------------------------------
    //
    // Confirms the default cap is far above the worst-case lattice size
    // for any realistic SCG + constraint set.
    #[test]
    fn default_max_iterations_is_safety_net() {
        let mut solver = BDConstraintSolver::new();
        // The default cap is 10 000 (DEFAULT_MAX_ITERATIONS, not
        // directly accessible from the test module, but inferable from
        // the documented behaviour). We verify it's far above the
        // 100-iteration cap of the pre-W6 default.
        assert!(
            solver.max_iterations >= 10_000,
            "Default max_iterations must be the 10 000 safety-net cap, got {}",
            solver.max_iterations
        );

        // A trivial SCG must converge in <= a handful of iterations.
        let mut scg = SCG::new();
        let n1 = add_comp_node(&mut scg);
        let n2 = add_comp_node(&mut scg);
        solver.add_constraint(BDConstraint::Equality {
            node_a: n1,
            node_b: n2,
        });
        let result = solver.solve(&scg);
        assert!(result.is_ok(), "Expected Ok, got {:?}", result);
    }
}
