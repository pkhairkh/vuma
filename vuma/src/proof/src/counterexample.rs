//! # Counterexample Generation
//!
//! When a proof fails or an invariant is violated, a counterexample provides
//! a concrete execution trace that demonstrates the violation. This module
//! provides data structures for representing counterexamples and methods for
//! constructing minimal ones.

use serde::{Deserialize, Serialize};

use crate::proof::{InvariantName, ProgramPoint};

// ---------------------------------------------------------------------------
// Step — execution trace step
// ---------------------------------------------------------------------------

/// A single step in an execution trace that constitutes a counterexample.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum Step {
    /// Allocate a memory region.
    Alloc {
        /// The region being allocated.
        region: u64,
    },

    /// Free a memory region.
    Free {
        /// The region being freed.
        region: u64,
    },

    /// Read from an address within a region.
    Read {
        /// The address being read.
        addr: u64,
        /// The region being read from.
        region: u64,
    },

    /// Write a value to an address within a region.
    Write {
        /// The address being written to.
        addr: u64,
        /// The region being written into.
        region: u64,
        /// The value being written (simplified as a u64 for the scaffold).
        value: u64,
    },

    /// Take one branch of a conditional.
    Branch {
        /// Which branch was taken (true = then, false = else).
        taken: bool,
    },
}

impl std::fmt::Display for Step {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Step::Alloc { region } => write!(f, "alloc r{}", region),
            Step::Free { region } => write!(f, "free r{}", region),
            Step::Read { addr, region } => write!(f, "read [0x{:x}] from r{}", addr, region),
            Step::Write {
                addr,
                region,
                value,
            } => {
                write!(f, "write 0x{:x} to [0x{:x}] in r{}", value, addr, region)
            }
            Step::Branch { taken } => write!(f, "branch({})", if *taken { "then" } else { "else" }),
        }
    }
}

// ---------------------------------------------------------------------------
// ViolationPoint
// ---------------------------------------------------------------------------

/// The point at which an invariant is violated in a program.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ViolationPoint {
    /// The invariant that is violated.
    pub invariant: InvariantName,
    /// Human-readable description of the violation.
    pub description: String,
    /// The program point (e.g. instruction offset) where the violation occurs.
    pub location: ProgramPoint,
}

impl ViolationPoint {
    /// Create a new violation point.
    pub fn new(
        invariant: InvariantName,
        description: impl Into<String>,
        location: ProgramPoint,
    ) -> Self {
        Self {
            invariant,
            description: description.into(),
            location,
        }
    }
}

// ---------------------------------------------------------------------------
// CounterExample
// ---------------------------------------------------------------------------

/// A counterexample: an execution trace that leads to an invariant violation.
///
/// Counterexamples are the dual of proofs — where a proof shows that an
/// invariant *always* holds, a counterexample demonstrates a specific scenario
/// where it *does not*.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CounterExample {
    /// The execution trace leading to the violation.
    pub execution: Vec<Step>,
    /// The point at which the invariant is violated.
    pub violation: ViolationPoint,
}

impl CounterExample {
    /// Create a counterexample from a violation point, with an empty execution
    /// trace.
    pub fn from_violation(_msg: &str, violation: ViolationPoint) -> Self {
        Self {
            execution: Vec::new(),
            violation,
        }
    }

    /// Construct a minimal counterexample by applying delta-debugging style
    /// trace minimization.
    ///
    /// Starting from the violation point, traces backwards through the proof
    /// steps to find a minimal set of steps that still produce the violation.
    /// Steps that don't contribute to the violation are removed using a
    /// delta-debugging approach:
    ///
    /// 1. Collect all steps from the counterexample.
    /// 2. Start with all steps as "necessary".
    /// 3. For each step, try removing it and check if the violation still
    ///    holds.
    /// 4. If yes, mark it as unnecessary and keep it removed.
    /// 5. Return the minimized set.
    pub fn minimal(&self) -> CounterExample {
        // If the trace is empty, produce a minimal trace from the violation.
        if self.execution.is_empty() {
            return CounterExample {
                execution: self.infer_minimal_trace(),
                violation: self.violation.clone(),
            };
        }

        // Delta-debugging: try removing each step one by one. If the
        // violation still holds after removal, the step is unnecessary.
        let mut necessary: Vec<Step> = self.execution.clone();

        let mut i = 0;
        while i < necessary.len() {
            let candidate: Vec<Step> = necessary
                .iter()
                .enumerate()
                .filter(|(idx, _)| *idx != i)
                .map(|(_, s)| s.clone())
                .collect();

            if self.violation_still_holds(&candidate) {
                // The step at index i is unnecessary — keep it removed.
                necessary = candidate;
                // Don't increment i: the next element has shifted into
                // position i, so we re-check the same index.
            } else {
                // The step is necessary; move on to the next one.
                i += 1;
            }
        }

        CounterExample {
            execution: necessary,
            violation: self.violation.clone(),
        }
    }

    /// Check whether the violation is still demonstrated by the given subset
    /// of steps.
    ///
    /// A violation still holds if the reduced trace contains at least one
    /// step that is *relevant* to the violated invariant — i.e. the step
    /// type matches the invariant category and the trace can still reach the
    /// violation point.
    fn violation_still_holds(&self, steps: &[Step]) -> bool {
        if steps.is_empty() {
            return false;
        }

        match self.violation.invariant {
            InvariantName::Liveness => {
                // Liveness (use-after-free): need a Free followed by a Read
                // or Write referencing the same region, or at minimum a Free
                // step that demonstrates the liveness issue.
                let has_free = steps.iter().any(|s| matches!(s, Step::Free { .. }));
                if !has_free {
                    return false;
                }
                // Check if there's a Read/Write after a Free on the same
                // region — if so, the violation is clearly demonstrated.
                let free_regions: std::collections::HashSet<u64> = steps
                    .iter()
                    .filter_map(|s| match s {
                        Step::Free { region } => Some(*region),
                        _ => None,
                    })
                    .collect();
                let has_post_free_access = steps.iter().any(|s| match s {
                    Step::Read { region, .. } => free_regions.contains(region),
                    Step::Write { region, .. } => free_regions.contains(region),
                    _ => false,
                });
                has_post_free_access || steps.len() == 1
            }
            InvariantName::Exclusivity => {
                // Exclusivity (data race): need at least a Write step that
                // demonstrates conflicting access.
                steps.iter().any(|s| matches!(s, Step::Write { .. }))
            }
            InvariantName::Cleanup => {
                // Cleanup: need an Alloc without a corresponding Free.
                let alloc_regions: std::collections::HashSet<u64> = steps
                    .iter()
                    .filter_map(|s| match s {
                        Step::Alloc { region } => Some(*region),
                        _ => None,
                    })
                    .collect();
                let free_regions: std::collections::HashSet<u64> = steps
                    .iter()
                    .filter_map(|s| match s {
                        Step::Free { region } => Some(*region),
                        _ => None,
                    })
                    .collect();
                // Violation holds if there's an alloc without a matching free.
                !alloc_regions.is_empty() && !alloc_regions.is_subset(&free_regions)
            }
            InvariantName::Origin | InvariantName::Interpretation => {
                // Origin / Interpretation: violation holds as long as the
                // trace is non-empty — any step could contribute to the
                // provenance or representation issue.
                !steps.is_empty()
            }
        }
    }

    /// Infer a minimal trace from the violation type when the original
    /// execution trace is empty.
    fn infer_minimal_trace(&self) -> Vec<Step> {
        match self.violation.invariant {
            InvariantName::Liveness => {
                // Use-after-free: Free then Read on the same region.
                vec![
                    Step::Free { region: 0 },
                    Step::Read {
                        addr: 0,
                        region: 0,
                    },
                ]
            }
            InvariantName::Exclusivity => {
                // Data race: two writes to the same address.
                vec![
                    Step::Write {
                        addr: 0,
                        region: 0,
                        value: 1,
                    },
                    Step::Write {
                        addr: 0,
                        region: 0,
                        value: 2,
                    },
                ]
            }
            InvariantName::Cleanup => {
                // Leak: alloc without free.
                vec![Step::Alloc { region: 0 }]
            }
            InvariantName::Origin | InvariantName::Interpretation => {
                vec![Step::Alloc { region: 0 }]
            }
        }
    }

    /// Add a step to the execution trace.
    pub fn add_step(&mut self, step: Step) {
        self.execution.push(step);
    }

    /// Return the number of steps in the execution trace.
    pub fn len(&self) -> usize {
        self.execution.len()
    }

    /// Return true if the execution trace is empty.
    pub fn is_empty(&self) -> bool {
        self.execution.is_empty()
    }
}

impl std::fmt::Display for CounterExample {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(
            f,
            "Counterexample for invariant '{}':",
            self.violation.invariant
        )?;
        writeln!(f, "  Violation: {}", self.violation.description)?;
        writeln!(f, "  Location: 0x{:x}", self.violation.location)?;
        writeln!(f, "  Trace ({} steps):", self.execution.len())?;
        for (i, step) in self.execution.iter().enumerate() {
            writeln!(f, "    {}: {}", i, step)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_step_display() {
        assert_eq!(format!("{}", Step::Alloc { region: 1 }), "alloc r1");
        assert_eq!(format!("{}", Step::Free { region: 2 }), "free r2");
        assert_eq!(
            format!(
                "{}",
                Step::Read {
                    addr: 0x100,
                    region: 3
                }
            ),
            "read [0x100] from r3"
        );
        assert_eq!(
            format!(
                "{}",
                Step::Write {
                    addr: 0x200,
                    region: 4,
                    value: 42
                }
            ),
            "write 0x2a to [0x200] in r4"
        );
        assert_eq!(format!("{}", Step::Branch { taken: true }), "branch(then)");
    }

    #[test]
    fn test_counterexample_from_violation() {
        let violation = ViolationPoint::new(
            InvariantName::Liveness,
            "use after free of region 42",
            0x1000,
        );
        let ce = CounterExample::from_violation("error", violation);
        assert!(ce.is_empty());
        assert_eq!(ce.violation.invariant, InvariantName::Liveness);
    }

    #[test]
    fn test_counterexample_minimal_liveness() {
        let violation = ViolationPoint::new(InvariantName::Liveness, "use after free", 0x100);
        let ce = CounterExample::from_violation("err", violation);
        let min = ce.minimal();
        assert!(!min.is_empty());
        // For an empty trace, infer_minimal_trace produces [Free, Read].
        assert!(matches!(min.execution[0], Step::Free { .. }));
        assert_eq!(min.execution.len(), 2);
    }

    #[test]
    fn test_counterexample_minimal_exclusivity() {
        let violation = ViolationPoint::new(InvariantName::Exclusivity, "data race", 0x200);
        let ce = CounterExample::from_violation("err", violation);
        let min = ce.minimal();
        // For an empty trace, infer_minimal_trace produces two Write steps.
        assert!(matches!(min.execution[0], Step::Write { .. }));
        assert_eq!(min.execution.len(), 2);
    }

    #[test]
    fn test_minimal_counterexample_removes_unnecessary_steps() {
        // Create a counterexample with 5 steps where 2 are unnecessary.
        //
        // For a Liveness violation, only the Free and a subsequent Read on the
        // same region are necessary. An Alloc, a Branch, and a Write to a
        // different region are unnecessary.
        let violation = ViolationPoint::new(InvariantName::Liveness, "use after free", 0x100);
        let mut ce = CounterExample::from_violation("err", violation);
        ce.add_step(Step::Alloc { region: 1 }); // unnecessary
        ce.add_step(Step::Free { region: 1 }); // necessary
        ce.add_step(Step::Branch { taken: true }); // unnecessary
        ce.add_step(Step::Read { addr: 0x10, region: 1 }); // necessary
        ce.add_step(Step::Write { addr: 0x20, region: 99, value: 0 }); // unnecessary

        assert_eq!(ce.len(), 5);

        let min = ce.minimal();
        // After minimization, only Free and Read on the same region remain.
        assert_eq!(min.execution.len(), 2);
        assert!(matches!(min.execution[0], Step::Free { region: 1 }));
        assert!(matches!(min.execution[1], Step::Read { region: 1, .. }));
    }

    #[test]
    fn test_counterexample_add_step() {
        let violation = ViolationPoint::new(InvariantName::Liveness, "test", 0);
        let mut ce = CounterExample::from_violation("", violation);
        ce.add_step(Step::Alloc { region: 1 });
        ce.add_step(Step::Free { region: 1 });
        assert_eq!(ce.len(), 2);
    }

    #[test]
    fn test_counterexample_display() {
        let violation = ViolationPoint::new(InvariantName::Liveness, "use after free", 0x100);
        let mut ce = CounterExample::from_violation("", violation);
        ce.add_step(Step::Alloc { region: 1 });
        let s = format!("{}", ce);
        assert!(s.contains("Counterexample"));
        assert!(s.contains("liveness"));
    }
}
