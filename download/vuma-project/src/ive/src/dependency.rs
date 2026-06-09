//! Cross-invariant dependency analysis for the IVE module.
//!
//! When one VUMA invariant depends on another (e.g., interpretation depends
//! on exclusivity being resolved), this module tracks and validates those
//! dependencies, provides topological ordering, impact analysis, and
//! incremental re-verification planning.
//!
//! # Known VUMA Dependencies
//!
//! | Dependent      | Depends on   | Strength    | Reason                                              |
//! |----------------|-------------|-------------|------------------------------------------------------|
//! | interpretation | exclusivity | Conditional | Can't check BD compatibility without knowing aliasing |
//! | exclusivity    | liveness    | Hard        | Can't check conflicts if memory is freed             |
//! | cleanup        | liveness    | Hard        | Can't track lifecycle if liveness is unknown         |
//! | origin         | liveness    | Hard        | Can't trace derivation chains if source is freed     |
//!
//! # Example
//!
//! ```rust
//! use vuma_ive::dependency::InvariantDependencyGraph;
//!
//! let graph = InvariantDependencyGraph::default();
//!
//! // Validate an execution order respects dependencies
//! let order = vec!["liveness".into(), "origin".into(), "exclusivity".into(),
//!                  "interpretation".into(), "cleanup".into()];
//! assert!(graph.validate_execution_order(&order).is_ok());
//!
//! // Get a valid topological order
//! let topo = graph.topological_order().unwrap();
//! assert_eq!(topo[0], "liveness");
//! ```

use hashbrown::{HashMap, HashSet};
use serde::{Deserialize, Serialize};
use std::fmt;

// ---------------------------------------------------------------------------
// DependencyStrength
// ---------------------------------------------------------------------------

/// How strongly one invariant depends on another.
///
/// Not all dependencies are equally strict. A *hard* dependency means the
/// dependent invariant cannot be checked at all without its prerequisite
/// being resolved. A *conditional* dependency applies only under certain
/// runtime conditions. A *soft* dependency is recommended but not required.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DependencyStrength {
    /// Always required — the dependent invariant cannot produce a
    /// meaningful result without this prerequisite.
    Hard,
    /// Required only when `condition` is true at runtime.
    /// For example, interpretation depends on exclusivity only when
    /// there are concurrent accesses.
    Conditional(String),
    /// Recommended but not required. Violating a soft dependency may
    /// produce less precise results but will not cause incorrect ones.
    Soft,
}

impl DependencyStrength {
    /// Returns `true` if this dependency is *active* given the set of
    /// currently-true conditions.
    ///
    /// - `Hard` is always active.
    /// - `Conditional(cond)` is active if `cond` is in `active_conditions`.
    /// - `Soft` is never considered active for ordering enforcement.
    pub fn is_active(&self, active_conditions: &HashSet<String>) -> bool {
        match self {
            DependencyStrength::Hard => true,
            DependencyStrength::Conditional(cond) => active_conditions.contains(cond),
            DependencyStrength::Soft => false,
        }
    }

    /// Returns `true` if this is a hard dependency.
    pub fn is_hard(&self) -> bool {
        matches!(self, DependencyStrength::Hard)
    }

    /// Returns `true` if this is a conditional dependency.
    pub fn is_conditional(&self) -> bool {
        matches!(self, DependencyStrength::Conditional(_))
    }

    /// Returns `true` if this is a soft dependency.
    pub fn is_soft(&self) -> bool {
        matches!(self, DependencyStrength::Soft)
    }
}

impl fmt::Display for DependencyStrength {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DependencyStrength::Hard => write!(f, "hard"),
            DependencyStrength::Conditional(cond) => write!(f, "conditional({cond})"),
            DependencyStrength::Soft => write!(f, "soft"),
        }
    }
}

// ---------------------------------------------------------------------------
// DependencyEdge
// ---------------------------------------------------------------------------

/// A directed edge in the dependency graph from one invariant to another.
///
/// An edge `(A → B)` with strength `s` means invariant `A` depends on
/// invariant `B` with strength `s`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DependencyEdge {
    /// The invariant that has the dependency.
    pub from: String,
    /// The invariant that is depended upon.
    pub to: String,
    /// How strong the dependency is.
    pub strength: DependencyStrength,
    /// Human-readable explanation of *why* this dependency exists.
    pub reason: String,
}

// ---------------------------------------------------------------------------
// DependencyViolation
// ---------------------------------------------------------------------------

/// Describes a single dependency violation found during execution-order
/// validation.
///
/// A violation occurs when an invariant appears in the execution order
/// before one of its (active) prerequisites.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DependencyViolation {
    /// The invariant that was executed too early.
    pub invariant: String,
    /// The invariant it depends on (but which has not been executed yet).
    pub depends_on: String,
    /// Human-readable explanation of why this dependency matters.
    pub reason: String,
}

impl fmt::Display for DependencyViolation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "'{}' depends on '{}' (not yet executed): {}",
            self.invariant, self.depends_on, self.reason
        )
    }
}

// ---------------------------------------------------------------------------
// CyclicDependency
// ---------------------------------------------------------------------------

/// Error returned when the dependency graph contains a cycle and therefore
/// no topological ordering exists.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CyclicDependency {
    /// The invariants involved in the cycle (in order).
    pub cycle: Vec<String>,
}

impl fmt::Display for CyclicDependency {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "cyclic dependency: ")?;
        let parts: Vec<String> = self
            .cycle
            .iter()
            .chain(self.cycle.first())
            .map(|s| s.clone())
            .collect();
        write!(f, "{}", parts.join(" → "))
    }
}

impl std::error::Error for CyclicDependency {}
impl std::error::Error for DependencyViolation {}

// ---------------------------------------------------------------------------
// ImpactSet
// ---------------------------------------------------------------------------

/// The set of invariants affected when a given invariant's result changes.
///
/// When an invariant is re-verified and produces a different result,
/// any invariant that depends on it (directly or transitively) may also
/// need re-verification.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ImpactSet {
    /// Invariants that directly depend on the changed invariant.
    pub directly_affected: HashSet<String>,
    /// Invariants reachable by following two or more dependency edges.
    pub transitively_affected: HashSet<String>,
    /// All invariants (direct + transitive) that need re-verification,
    /// listed in a valid topological order.
    pub re_verification_needed: Vec<String>,
}

impl fmt::Display for ImpactSet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "ImpactSet:")?;
        writeln!(
            f,
            "  Directly affected  : {{ {} }}",
            self.directly_affected
                .iter()
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
        )?;
        writeln!(
            f,
            "  Transitively affected: {{ {} }}",
            self.transitively_affected
                .iter()
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
        )?;
        writeln!(
            f,
            "  Re-verification order: {}",
            self.re_verification_needed.join(" → ")
        )?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// ReVerificationStep / ReVerificationPlan
// ---------------------------------------------------------------------------

/// A single step in a re-verification plan.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReVerificationStep {
    /// The invariant to re-verify in this step.
    pub invariant: String,
    /// Why this invariant needs re-verification.
    pub reason: String,
    /// Which invariants this step's result depends on (must already be
    /// re-verified in earlier steps).
    pub depends_on: Vec<String>,
}

/// A plan for incrementally re-verifying a set of changed invariants and
/// all of their dependents.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReVerificationPlan {
    /// The re-verification steps, in the order they should be executed.
    pub steps: Vec<ReVerificationStep>,
    /// Estimated relative cost (0.0 = free, 1.0 = most expensive).
    /// Computed as a weighted sum based on dependency depth and the
    /// number of hard vs. conditional/soft edges.
    pub estimated_cost: f64,
}

impl fmt::Display for ReVerificationPlan {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "ReVerificationPlan (estimated cost: {:.2}):", self.estimated_cost)?;
        for (i, step) in self.steps.iter().enumerate() {
            write!(f, "  {}. {}", i + 1, step.invariant)?;
            if !step.depends_on.is_empty() {
                write!(f, " (depends on: {})", step.depends_on.join(", "))?;
            }
            writeln!(f, " — {}", step.reason)?;
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// InvariantDependencyGraph
// ---------------------------------------------------------------------------

/// A directed graph representing dependencies between VUMA invariants.
///
/// Each edge `(A → B)` means *A depends on B* — B must be verified before
/// A can be meaningfully checked.
///
/// The default graph encodes the known VUMA dependencies:
///
/// - **interpretation** depends on **exclusivity** (conditional: concurrent accesses)
/// - **exclusivity** depends on **liveness** (hard)
/// - **cleanup** depends on **liveness** (hard)
/// - **origin** depends on **liveness** (hard)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InvariantDependencyGraph {
    /// Adjacency list: for each invariant, the set of invariants it
    /// depends on, together with edge metadata.
    edges: HashMap<String, Vec<DependencyEdge>>,
}

impl Default for InvariantDependencyGraph {
    fn default() -> Self {
        let mut graph = Self {
            edges: HashMap::new(),
        };

        // Ensure all five invariants have entries (even if no edges).
        for name in &["liveness", "exclusivity", "interpretation", "origin", "cleanup"] {
            graph.edges.insert((*name).to_string(), Vec::new());
        }

        // interpretation depends on exclusivity (conditional: concurrent accesses)
        graph.add_edge(DependencyEdge {
            from: "interpretation".into(),
            to: "exclusivity".into(),
            strength: DependencyStrength::Conditional("concurrent_accesses".into()),
            reason: "Can't check BD compatibility without knowing aliasing".into(),
        });

        // exclusivity depends on liveness (hard)
        graph.add_edge(DependencyEdge {
            from: "exclusivity".into(),
            to: "liveness".into(),
            strength: DependencyStrength::Hard,
            reason: "Can't check conflicts if memory is freed".into(),
        });

        // cleanup depends on liveness (hard)
        graph.add_edge(DependencyEdge {
            from: "cleanup".into(),
            to: "liveness".into(),
            strength: DependencyStrength::Hard,
            reason: "Can't track lifecycle if liveness is unknown".into(),
        });

        // origin depends on liveness (hard)
        graph.add_edge(DependencyEdge {
            from: "origin".into(),
            to: "liveness".into(),
            strength: DependencyStrength::Hard,
            reason: "Can't trace derivation chains if source is freed".into(),
        });

        graph
    }
}

impl InvariantDependencyGraph {
    /// Create an empty dependency graph with no invariants.
    pub fn new() -> Self {
        Self {
            edges: HashMap::new(),
        }
    }

    /// Add a dependency edge to the graph.
    ///
    /// Both the `from` and `to` invariants are automatically registered
    /// if they do not already exist.
    pub fn add_edge(&mut self, edge: DependencyEdge) {
        self.edges
            .entry(edge.from.clone())
            .or_default()
            .push(edge.clone());

        // Ensure the target also has an entry.
        self.edges.entry(edge.to.clone()).or_default();
    }

    /// Add an invariant node with no edges.
    pub fn add_invariant(&mut self, name: &str) {
        self.edges.entry(name.to_string()).or_default();
    }

    /// Return all invariant names in the graph.
    pub fn invariants(&self) -> HashSet<String> {
        self.edges.keys().cloned().collect()
    }

    /// Return all edges originating from the given invariant.
    pub fn dependencies_of(&self, invariant: &str) -> &[DependencyEdge] {
        self.edges
            .get(invariant)
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }

    /// Return all edges in the graph.
    pub fn all_edges(&self) -> Vec<&DependencyEdge> {
        self.edges.values().flat_map(|v| v.iter()).collect()
    }

    // -----------------------------------------------------------------------
    // Execution-order validation
    // -----------------------------------------------------------------------

    /// Validate that the given execution order respects all hard
    /// dependencies.
    ///
    /// Returns `Ok(())` if the order is valid, or `Err(DependencyViolation)`
    /// describing the first violation found.
    pub fn validate_execution_order(
        &self,
        order: &[String],
    ) -> Result<(), DependencyViolation> {
        self.validate_execution_order_with_conditions(order, &HashSet::new())
    }

    /// Validate execution order, considering conditional dependencies
    /// whose conditions are in `active_conditions`.
    pub fn validate_execution_order_with_conditions(
        &self,
        order: &[String],
        active_conditions: &HashSet<String>,
    ) -> Result<(), DependencyViolation> {
        let position: HashMap<&String, usize> = order
            .iter()
            .enumerate()
            .map(|(i, inv)| (inv, i))
            .collect();

        for (invariant, edges) in &self.edges {
            let inv_pos = match position.get(&invariant) {
                Some(&p) => p,
                None => continue, // invariant not in order; skip
            };

            for edge in edges {
                let active = match &edge.strength {
                    DependencyStrength::Hard => true,
                    DependencyStrength::Conditional(cond) => {
                        active_conditions.contains(cond)
                    }
                    DependencyStrength::Soft => false,
                };

                if !active {
                    continue;
                }

                match position.get(&edge.to) {
                    Some(&dep_pos) if dep_pos > inv_pos => {
                        return Err(DependencyViolation {
                            invariant: invariant.clone(),
                            depends_on: edge.to.clone(),
                            reason: edge.reason.clone(),
                        });
                    }
                    None => {
                        // Dependency not in order at all — this is a
                        // violation if the edge is hard.
                        if edge.strength.is_hard() {
                            return Err(DependencyViolation {
                                invariant: invariant.clone(),
                                depends_on: edge.to.clone(),
                                reason: format!(
                                    "required dependency '{}' is missing from the execution order",
                                    edge.to
                                ),
                            });
                        }
                    }
                    _ => {}
                }
            }
        }

        Ok(())
    }

    // -----------------------------------------------------------------------
    // Topological sort
    // -----------------------------------------------------------------------

    /// Return a valid topological ordering of all invariants, respecting
    /// hard dependencies.
    ///
    /// Uses Kahn's algorithm. Returns `Err(CyclicDependency)` if a cycle
    /// is detected.
    pub fn topological_order(&self) -> Result<Vec<String>, CyclicDependency> {
        self.topological_order_with_conditions(&HashSet::new())
    }

    /// Topological sort considering both hard and active conditional
    /// dependencies.
    pub fn topological_order_with_conditions(
        &self,
        active_conditions: &HashSet<String>,
    ) -> Result<Vec<String>, CyclicDependency> {
        let all_invariants: Vec<String> = self.edges.keys().cloned().collect();

        // Build adjacency list and in-degree map for active edges only.
        let mut in_degree: HashMap<&String, usize> =
            all_invariants.iter().map(|k| (k, 0)).collect();
        let mut adj: HashMap<&String, Vec<&String>> =
            all_invariants.iter().map(|k| (k, Vec::new())).collect();

        for (invariant, edges) in &self.edges {
            for edge in edges {
                let active = edge.strength.is_active(active_conditions);
                if active {
                    // edge: invariant → edge.to  (invariant depends on edge.to)
                    // So edge.to must come before invariant in topo order.
                    // In adjacency: edge.to → invariant
                    adj.entry(&edge.to).or_default().push(invariant);
                    *in_degree.entry(invariant).or_insert(0) += 1;
                }
            }
        }

        // Kahn's algorithm
        let mut queue: Vec<&String> = in_degree
            .iter()
            .filter(|(_, &deg)| deg == 0)
            .map(|(&k, _)| k)
            .collect();

        // Sort the initial queue for deterministic output (alphabetical).
        queue.sort();

        let mut result = Vec::with_capacity(all_invariants.len());

        while let Some(node) = queue.pop() {
            result.push(node.clone());

            let mut neighbors: Vec<&String> = adj
                .get(node)
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .collect();
            // Sort for determinism.
            neighbors.sort();

            for &neighbor in &neighbors {
                let deg = in_degree.get_mut(neighbor).unwrap();
                *deg -= 1;
                if *deg == 0 {
                    queue.push(neighbor);
                }
            }

            // Re-sort for determinism (we use a Vec as a "sorted stack").
            queue.sort();
        }

        if result.len() != all_invariants.len() {
            // Find a cycle for the error.
            let remaining: HashSet<&String> = all_invariants
                .iter()
                .filter(|inv| !result.contains(inv))
                .collect();

            let cycle = self.find_cycle(&remaining);
            return Err(CyclicDependency { cycle });
        }

        Ok(result)
    }

    /// Find one cycle among the remaining (unvisited) invariants.
    fn find_cycle(&self, remaining: &HashSet<&String>) -> Vec<String> {
        // Convert to owned set for simpler lifetime management.
        let remaining_owned: HashSet<String> =
            remaining.iter().map(|s| (*s).clone()).collect();

        let mut visited: HashSet<String> = HashSet::new();
        let mut stack: HashSet<String> = HashSet::new();
        let mut path: Vec<String> = Vec::new();

        for start in &remaining_owned {
            if visited.contains(start) {
                continue;
            }
            if self.dfs_cycle_owned(start, &remaining_owned, &mut visited, &mut stack, &mut path) {
                return path;
            }
        }

        // Fallback — should not happen if cycle truly exists.
        remaining_owned.into_iter().collect()
    }

    fn dfs_cycle_owned(
        &self,
        node: &str,
        remaining: &HashSet<String>,
        visited: &mut HashSet<String>,
        stack: &mut HashSet<String>,
        path: &mut Vec<String>,
    ) -> bool {
        visited.insert(node.to_string());
        stack.insert(node.to_string());

        if let Some(edges) = self.edges.get(node) {
            for edge in edges {
                if !remaining.contains(&edge.to) {
                    continue;
                }
                if stack.contains(&edge.to) {
                    // Found a cycle.
                    path.clear();
                    path.push(edge.to.clone());
                    path.push(node.to_string());
                    path.push(edge.to.clone());
                    return true;
                }
                if !visited.contains(&edge.to) {
                    if self.dfs_cycle_owned(&edge.to, remaining, visited, stack, path) {
                        return true;
                    }
                }
            }
        }

        stack.remove(node);
        false
    }

    // -----------------------------------------------------------------------
    // Impact analysis
    // -----------------------------------------------------------------------

    /// Compute the set of invariants affected when the given invariant's
    /// result changes.
    ///
    /// The impact set includes:
    /// - **directly affected**: invariants that have a direct dependency
    ///   edge *from* them *to* the changed invariant (i.e., they depend
    ///   on it).
    /// - **transitively affected**: invariants reachable by following
    ///   dependency edges two or more hops.
    /// - **re_verification_needed**: all affected invariants in a valid
    ///   topological order.
    pub fn impact_of_change(&self, invariant: &str) -> ImpactSet {
        // Build the *reverse* graph: for each invariant B, which
        // invariants depend on B?
        let mut reverse: HashMap<&String, Vec<&DependencyEdge>> = HashMap::new();
        for (_inv, edges) in &self.edges {
            for edge in edges {
                reverse.entry(&edge.to).or_default().push(edge);
            }
        }

        // BFS from the changed invariant through the reverse graph.
        let mut directly_affected = HashSet::new();
        let mut transitively_affected = HashSet::new();
        let mut all_affected = HashSet::new();
        let mut queue = vec![invariant];
        let mut visited = HashSet::new();
        visited.insert(invariant.to_string());

        // First hop: direct dependents.
        if let Some(dependents) = reverse.get(&invariant.to_string()) {
            for edge in dependents {
                if !visited.contains(&edge.from) {
                    directly_affected.insert(edge.from.clone());
                    all_affected.insert(edge.from.clone());
                    visited.insert(edge.from.clone());
                    queue.push(&edge.from);
                }
            }
        }

        // Subsequent hops: transitive dependents.
        let mut frontier: Vec<String> = directly_affected.iter().cloned().collect();
        while !frontier.is_empty() {
            let mut next_frontier = Vec::new();
            for inv in &frontier {
                if let Some(dependents) = reverse.get(inv) {
                    for edge in dependents {
                        if !visited.contains(&edge.from) {
                            transitively_affected.insert(edge.from.clone());
                            all_affected.insert(edge.from.clone());
                            visited.insert(edge.from.clone());
                            next_frontier.push(edge.from.clone());
                            queue.push(&edge.from);
                        }
                    }
                }
            }
            frontier = next_frontier;
        }

        // Compute re-verification order: the changed invariant first,
        // then all affected invariants in topological order.
        let mut re_verification_needed = vec![invariant.to_string()];
        if !all_affected.is_empty() {
            // Use the graph's topological order but filter to only
            // the changed + affected invariants.
            if let Ok(full_order) = self.topological_order() {
                for inv in &full_order {
                    if all_affected.contains(inv) && !re_verification_needed.contains(inv) {
                        re_verification_needed.push(inv.clone());
                    }
                }
            }
        }

        ImpactSet {
            directly_affected,
            transitively_affected,
            re_verification_needed,
        }
    }

    // -----------------------------------------------------------------------
    // Incremental re-verification planning
    // -----------------------------------------------------------------------

    /// Plan an incremental re-verification for the given set of changed
    /// invariants.
    ///
    /// The plan includes:
    /// 1. The changed invariants themselves (in topological order).
    /// 2. All directly and transitively affected invariants (also in
    ///    topological order).
    ///
    /// Each step records *why* the re-verification is needed and which
    /// earlier steps it depends on.
    pub fn plan_re_verification(&self, changed_invariants: &[String]) -> ReVerificationPlan {
        // Collect all impacted invariants.
        let mut all_impacted: HashSet<String> = changed_invariants.iter().cloned().collect();
        let mut reasons: HashMap<String, String> = HashMap::new();

        for inv in changed_invariants {
            reasons.insert(
                inv.clone(),
                "result changed — must re-verify".to_string(),
            );
            let impact = self.impact_of_change(inv);
            for affected in impact
                .directly_affected
                .iter()
                .chain(impact.transitively_affected.iter())
            {
                if !all_impacted.contains(affected) {
                    reasons.insert(
                        affected.clone(),
                        format!("depends on changed invariant '{}'", inv),
                    );
                }
                all_affected_insert(&mut all_impacted, affected.clone());
            }
        }

        // Get topological order and filter.
        let full_order = self.topological_order().unwrap_or_default();
        let ordered: Vec<String> = full_order
            .into_iter()
            .filter(|inv| all_impacted.contains(inv))
            .collect();

        // Build steps.
        let mut steps = Vec::with_capacity(ordered.len());
        let mut completed: HashSet<String> = HashSet::new();

        for inv in &ordered {
            let deps: Vec<String> = self
                .dependencies_of(inv)
                .iter()
                .filter(|edge| all_impacted.contains(&edge.to))
                .filter(|edge| edge.strength.is_hard())
                .map(|edge| edge.to.clone())
                .filter(|dep| completed.contains(dep))
                .collect();

            let reason = reasons
                .get(inv)
                .cloned()
                .unwrap_or_else(|| "propagated change".to_string());

            steps.push(ReVerificationStep {
                invariant: inv.clone(),
                reason,
                depends_on: deps,
            });

            completed.insert(inv.clone());
        }

        // Estimate cost: base cost per step + cost per hard dependency
        // traversal. Scale to 0.0–1.0 range.
        let hard_edge_count = self
            .all_edges()
            .iter()
            .filter(|e| e.strength.is_hard() && all_impacted.contains(&e.from))
            .count();
        let cond_edge_count = self
            .all_edges()
            .iter()
            .filter(|e| e.strength.is_conditional() && all_impacted.contains(&e.from))
            .count();
        let step_count = steps.len();

        let raw_cost = step_count as f64 * 1.0
            + hard_edge_count as f64 * 0.5
            + cond_edge_count as f64 * 0.25;
        let max_possible = 5.0 * 1.0 + 4.0 * 0.5 + 1.0 * 0.25; // all invariants + edges
        let estimated_cost = if max_possible > 0.0 {
            (raw_cost / max_possible).min(1.0)
        } else {
            0.0
        };

        ReVerificationPlan {
            steps,
            estimated_cost,
        }
    }
}

/// Helper: insert into a HashSet (avoids naming conflict with the field).
fn all_affected_insert(set: &mut HashSet<String>, val: String) {
    set.insert(val);
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // 1. Default dependency graph construction
    #[test]
    fn test_default_graph_construction() {
        let graph = InvariantDependencyGraph::default();
        let invariants = graph.invariants();

        assert!(invariants.contains("liveness"));
        assert!(invariants.contains("exclusivity"));
        assert!(invariants.contains("interpretation"));
        assert!(invariants.contains("origin"));
        assert!(invariants.contains("cleanup"));
        assert_eq!(invariants.len(), 5);

        // Check edges: exclusivity → liveness, cleanup → liveness,
        // origin → liveness, interpretation → exclusivity
        let exclusivity_deps = graph.dependencies_of("exclusivity");
        assert_eq!(exclusivity_deps.len(), 1);
        assert_eq!(exclusivity_deps[0].to, "liveness");
        assert!(exclusivity_deps[0].strength.is_hard());

        let interpretation_deps = graph.dependencies_of("interpretation");
        assert_eq!(interpretation_deps.len(), 1);
        assert_eq!(interpretation_deps[0].to, "exclusivity");
        assert!(interpretation_deps[0].strength.is_conditional());

        let cleanup_deps = graph.dependencies_of("cleanup");
        assert_eq!(cleanup_deps.len(), 1);
        assert_eq!(cleanup_deps[0].to, "liveness");

        let origin_deps = graph.dependencies_of("origin");
        assert_eq!(origin_deps.len(), 1);
        assert_eq!(origin_deps[0].to, "liveness");

        // Liveness has no dependencies
        let liveness_deps = graph.dependencies_of("liveness");
        assert!(liveness_deps.is_empty());
    }

    // 2. Topological sort produces valid order
    #[test]
    fn test_topological_sort_valid_order() {
        let graph = InvariantDependencyGraph::default();
        let order = graph.topological_order().unwrap();

        // Liveness must come before exclusivity, cleanup, and origin.
        let liveness_pos = order.iter().position(|x| x == "liveness").unwrap();
        let exclusivity_pos = order.iter().position(|x| x == "exclusivity").unwrap();
        let cleanup_pos = order.iter().position(|x| x == "cleanup").unwrap();
        let origin_pos = order.iter().position(|x| x == "origin").unwrap();

        assert!(liveness_pos < exclusivity_pos, "liveness must come before exclusivity");
        assert!(liveness_pos < cleanup_pos, "liveness must come before cleanup");
        assert!(liveness_pos < origin_pos, "liveness must come before origin");

        // All 5 invariants must be present
        assert_eq!(order.len(), 5);
    }

    // 3. Invalid order detection
    #[test]
    fn test_invalid_order_detection() {
        let graph = InvariantDependencyGraph::default();

        // This order puts exclusivity before liveness — invalid because
        // exclusivity hard-depends on liveness.
        let bad_order = vec![
            "exclusivity".into(),
            "liveness".into(),
            "origin".into(),
            "interpretation".into(),
            "cleanup".into(),
        ];

        let result = graph.validate_execution_order(&bad_order);
        assert!(result.is_err());
        let violation = result.unwrap_err();
        assert_eq!(violation.invariant, "exclusivity");
        assert_eq!(violation.depends_on, "liveness");
    }

    // Valid order passes
    #[test]
    fn test_valid_order_passes() {
        let graph = InvariantDependencyGraph::default();
        let good_order = vec![
            "liveness".into(),
            "origin".into(),
            "exclusivity".into(),
            "interpretation".into(),
            "cleanup".into(),
        ];
        assert!(graph.validate_execution_order(&good_order).is_ok());
    }

    // 4. Impact of liveness change
    #[test]
    fn test_impact_of_liveness_change() {
        let graph = InvariantDependencyGraph::default();
        let impact = graph.impact_of_change("liveness");

        // Liveness has 3 direct dependents: exclusivity, cleanup, origin.
        assert!(impact.directly_affected.contains("exclusivity"));
        assert!(impact.directly_affected.contains("cleanup"));
        assert!(impact.directly_affected.contains("origin"));
        assert_eq!(impact.directly_affected.len(), 3);

        // Interpretation depends on exclusivity, which depends on
        // liveness — so it's transitively affected.
        assert!(impact.transitively_affected.contains("interpretation"));

        // Re-verification includes liveness first.
        assert_eq!(impact.re_verification_needed[0], "liveness");
        assert!(impact.re_verification_needed.contains(&"exclusivity".to_string()));
        assert!(impact.re_verification_needed.contains(&"cleanup".to_string()));
        assert!(impact.re_verification_needed.contains(&"origin".to_string()));
        assert!(impact.re_verification_needed.contains(&"interpretation".to_string()));
    }

    // 5. Impact of exclusivity change
    #[test]
    fn test_impact_of_exclusivity_change() {
        let graph = InvariantDependencyGraph::default();
        let impact = graph.impact_of_change("exclusivity");

        // Interpretation depends on exclusivity.
        assert!(impact.directly_affected.contains("interpretation"));
        assert_eq!(impact.directly_affected.len(), 1);

        // No transitive dependents beyond interpretation.
        assert!(impact.transitively_affected.is_empty());

        // Re-verification includes exclusivity and interpretation.
        assert!(impact.re_verification_needed.contains(&"exclusivity".to_string()));
        assert!(impact.re_verification_needed.contains(&"interpretation".to_string()));
    }

    // 6. Re-verification planning for single change
    #[test]
    fn test_re_verification_single_change() {
        let graph = InvariantDependencyGraph::default();
        let plan = graph.plan_re_verification(&["liveness".into()]);

        // First step should be liveness itself.
        assert_eq!(plan.steps[0].invariant, "liveness");
        assert!(plan.steps[0].depends_on.is_empty());

        // All 5 invariants should be in the plan.
        let inv_names: Vec<&str> = plan.steps.iter().map(|s| s.invariant.as_str()).collect();
        assert!(inv_names.contains(&"exclusivity"));
        assert!(inv_names.contains(&"cleanup"));
        assert!(inv_names.contains(&"origin"));
        assert!(inv_names.contains(&"interpretation"));

        // Exclusivity step should depend on liveness.
        let exclusivity_step = plan
            .steps
            .iter()
            .find(|s| s.invariant == "exclusivity")
            .unwrap();
        assert!(exclusivity_step.depends_on.contains(&"liveness".to_string()));

        // Cost should be > 0
        assert!(plan.estimated_cost > 0.0);
    }

    // 7. Re-verification planning for multiple changes
    #[test]
    fn test_re_verification_multiple_changes() {
        let graph = InvariantDependencyGraph::default();
        let plan = graph.plan_re_verification(&[
            "liveness".into(),
            "exclusivity".into(),
        ]);

        // Both liveness and exclusivity are changed, so interpretation
        // is affected (depends on exclusivity) and cleanup/origin
        // are affected (depend on liveness).
        let inv_names: Vec<&str> = plan.steps.iter().map(|s| s.invariant.as_str()).collect();
        assert!(inv_names.contains(&"liveness"));
        assert!(inv_names.contains(&"exclusivity"));
        assert!(inv_names.contains(&"interpretation"));
        assert!(inv_names.contains(&"cleanup"));
        assert!(inv_names.contains(&"origin"));

        // Steps should be in topological order.
        let liveness_pos = inv_names.iter().position(|&x| x == "liveness").unwrap();
        let exclusivity_pos = inv_names.iter().position(|&x| x == "exclusivity").unwrap();
        assert!(liveness_pos < exclusivity_pos);
    }

    // 8. Conditional dependency evaluation
    #[test]
    fn test_conditional_dependency_evaluation() {
        let graph = InvariantDependencyGraph::default();

        // Without the concurrent_accesses condition, the interpretation→exclusivity
        // edge is inactive, so this order should be valid even though
        // interpretation comes before exclusivity.
        let order_without_condition = vec![
            "liveness".into(),
            "interpretation".into(),
            "origin".into(),
            "exclusivity".into(),
            "cleanup".into(),
        ];

        let result = graph.validate_execution_order(&order_without_condition);
        // Should pass — conditional edge is inactive by default.
        assert!(result.is_ok());

        // With concurrent_accesses active, the same order should fail.
        let mut active_conditions = HashSet::new();
        active_conditions.insert("concurrent_accesses".to_string());

        let result_with_condition = graph
            .validate_execution_order_with_conditions(&order_without_condition, &active_conditions);
        assert!(result_with_condition.is_err());
        let violation = result_with_condition.unwrap_err();
        assert_eq!(violation.invariant, "interpretation");
        assert_eq!(violation.depends_on, "exclusivity");
    }

    #[test]
    fn test_conditional_topological_order() {
        let graph = InvariantDependencyGraph::default();

        // Without conditions, interpretation and exclusivity have no
        // ordering constraint (conditional edge is inactive).
        let order_no_cond = graph.topological_order().unwrap();
        // Hard deps are still respected: liveness before exclusivity.
        let liveness_pos = order_no_cond.iter().position(|x| x == "liveness").unwrap();
        let exclusivity_pos = order_no_cond.iter().position(|x| x == "exclusivity").unwrap();
        assert!(liveness_pos < exclusivity_pos);

        // With concurrent_accesses, interpretation must come after exclusivity.
        let mut active = HashSet::new();
        active.insert("concurrent_accesses".to_string());
        let order_with_cond = graph.topological_order_with_conditions(&active).unwrap();
        let interp_pos = order_with_cond
            .iter()
            .position(|x| x == "interpretation")
            .unwrap();
        let excl_pos = order_with_cond
            .iter()
            .position(|x| x == "exclusivity")
            .unwrap();
        assert!(excl_pos < interp_pos);
    }

    // 9. Cycle detection
    #[test]
    fn test_cycle_detection() {
        let mut graph = InvariantDependencyGraph::new();

        // Create a cycle: A → B → C → A
        graph.add_edge(DependencyEdge {
            from: "A".into(),
            to: "B".into(),
            strength: DependencyStrength::Hard,
            reason: "A depends on B".into(),
        });
        graph.add_edge(DependencyEdge {
            from: "B".into(),
            to: "C".into(),
            strength: DependencyStrength::Hard,
            reason: "B depends on C".into(),
        });
        graph.add_edge(DependencyEdge {
            from: "C".into(),
            to: "A".into(),
            strength: DependencyStrength::Hard,
            reason: "C depends on A".into(),
        });

        let result = graph.topological_order();
        assert!(result.is_err());
        let cycle_err = result.unwrap_err();
        assert!(!cycle_err.cycle.is_empty());
    }

    // 10. Empty graph edge cases
    #[test]
    fn test_empty_graph_edge_cases() {
        let graph = InvariantDependencyGraph::new();

        // No invariants
        assert!(graph.invariants().is_empty());

        // Topological sort of empty graph succeeds with empty result.
        let order = graph.topological_order().unwrap();
        assert!(order.is_empty());

        // Validate empty order.
        assert!(graph.validate_execution_order(&[]).is_ok());

        // Impact of change on empty graph.
        let impact = graph.impact_of_change("nonexistent");
        assert!(impact.directly_affected.is_empty());
        assert!(impact.transitively_affected.is_empty());
        // re_verification_needed contains the queried invariant itself.
        assert_eq!(impact.re_verification_needed, vec!["nonexistent"]);

        // Re-verification plan for empty changes.
        let plan = graph.plan_re_verification(&[]);
        assert!(plan.steps.is_empty());
        assert_eq!(plan.estimated_cost, 0.0);
    }

    // Additional tests for thorough coverage

    #[test]
    fn test_dependency_strength_properties() {
        let hard = DependencyStrength::Hard;
        let cond = DependencyStrength::Conditional("test".into());
        let soft = DependencyStrength::Soft;

        assert!(hard.is_hard());
        assert!(!hard.is_conditional());
        assert!(!hard.is_soft());

        assert!(!cond.is_hard());
        assert!(cond.is_conditional());
        assert!(!cond.is_soft());

        assert!(!soft.is_hard());
        assert!(!soft.is_conditional());
        assert!(soft.is_soft());

        // is_active
        let mut conditions = HashSet::new();
        assert!(hard.is_active(&conditions));
        assert!(!cond.is_active(&conditions));
        assert!(!soft.is_active(&conditions));

        conditions.insert("test".to_string());
        assert!(cond.is_active(&conditions));
    }

    #[test]
    fn test_dependency_strength_display() {
        assert_eq!(format!("{}", DependencyStrength::Hard), "hard");
        assert_eq!(
            format!("{}", DependencyStrength::Conditional("foo".into())),
            "conditional(foo)"
        );
        assert_eq!(format!("{}", DependencyStrength::Soft), "soft");
    }

    #[test]
    fn test_add_invariant() {
        let mut graph = InvariantDependencyGraph::new();
        graph.add_invariant("custom_invariant");
        assert!(graph.invariants().contains("custom_invariant"));
        assert!(graph.dependencies_of("custom_invariant").is_empty());
    }

    #[test]
    fn test_violation_display() {
        let v = DependencyViolation {
            invariant: "interpretation".into(),
            depends_on: "exclusivity".into(),
            reason: "Can't check BD compatibility without knowing aliasing".into(),
        };
        let display = format!("{}", v);
        assert!(display.contains("interpretation"));
        assert!(display.contains("exclusivity"));
    }

    #[test]
    fn test_cyclic_dependency_display() {
        let c = CyclicDependency {
            cycle: vec!["A".into(), "B".into(), "A".into()],
        };
        let display = format!("{}", c);
        assert!(display.contains("A → B → A"));
    }

    #[test]
    fn test_impact_set_display() {
        let mut directly = HashSet::new();
        directly.insert("exclusivity".into());
        let mut transitively = HashSet::new();
        transitively.insert("interpretation".into());
        let impact = ImpactSet {
            directly_affected: directly,
            transitively_affected: transitively,
            re_verification_needed: vec![
                "liveness".into(),
                "exclusivity".into(),
                "interpretation".into(),
            ],
        };
        let display = format!("{}", impact);
        assert!(display.contains("exclusivity"));
        assert!(display.contains("interpretation"));
    }

    #[test]
    fn test_re_verification_plan_display() {
        let plan = ReVerificationPlan {
            steps: vec![ReVerificationStep {
                invariant: "liveness".into(),
                reason: "result changed".into(),
                depends_on: vec![],
            }],
            estimated_cost: 0.25,
        };
        let display = format!("{}", plan);
        assert!(display.contains("liveness"));
        assert!(display.contains("0.25"));
    }

    #[test]
    fn test_soft_dependency_not_enforced() {
        let mut graph = InvariantDependencyGraph::new();
        graph.add_edge(DependencyEdge {
            from: "A".into(),
            to: "B".into(),
            strength: DependencyStrength::Soft,
            reason: "recommended but not required".into(),
        });

        // A before B should be allowed with soft dependency.
        let order = vec!["A".into(), "B".into()];
        assert!(graph.validate_execution_order(&order).is_ok());

        // B before A should also be allowed with soft dependency.
        let reverse_order = vec!["B".into(), "A".into()];
        assert!(graph.validate_execution_order(&reverse_order).is_ok());
    }

    #[test]
    fn test_topological_order_deterministic() {
        let graph = InvariantDependencyGraph::default();

        // Run topological sort multiple times — should always produce
        // the same order.
        let order1 = graph.topological_order().unwrap();
        let order2 = graph.topological_order().unwrap();
        assert_eq!(order1, order2);
    }

    #[test]
    fn test_missing_hard_dependency_in_order() {
        let mut graph = InvariantDependencyGraph::new();
        graph.add_edge(DependencyEdge {
            from: "A".into(),
            to: "B".into(),
            strength: DependencyStrength::Hard,
            reason: "A needs B".into(),
        });

        // Order includes A but not B — should fail.
        let order = vec!["A".into()];
        let result = graph.validate_execution_order(&order);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().depends_on, "B");
    }
}
