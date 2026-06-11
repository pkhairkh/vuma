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
            Step::Write { addr, region, value } => {
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
    pub fn from_violation(
        _msg: &str,
        violation: ViolationPoint,
    ) -> Self {
        Self {
            execution: Vec::new(),
            violation,
        }
    }

    /// Construct a minimal counterexample — a single-step trace that directly
    /// demonstrates the violation.
    ///
    /// The "minimal" counterexample is a scaffolding placeholder: it creates
    /// the shortest possible trace (typically a single alloc/free/read/write
    /// step) that would lead to the described violation. In a full
    /// implementation this would use SMT-based trace minimization.
    pub fn minimal(&self) -> CounterExample {
        // Try to infer a minimal trace from the violation description.
        let minimal_step = if self.violation.invariant == InvariantName::Liveness {
            // Liveness violation: use-after-free — read after free.
            Some(Step::Free { region: 0 })
        } else if self.violation.invariant == InvariantName::Exclusivity {
            // Exclusivity violation: double write.
            Some(Step::Write {
                addr: 0,
                region: 0,
                value: 0,
            })
        } else if matches!(self.violation.invariant, InvariantName::Cleanup | InvariantName::Origin | InvariantName::Interpretation) {
            // Default: an alloc step for other invariant violations.
            Some(Step::Alloc { region: 0 })
        } else {
            // Default: an alloc step.
            Some(Step::Alloc { region: 0 })
        };

        CounterExample {
            execution: minimal_step.into_iter().collect(),
            violation: self.violation.clone(),
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
        writeln!(f, "Counterexample for invariant '{}':", self.violation.invariant)?;
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
            format!("{}", Step::Read { addr: 0x100, region: 3 }),
            "read [0x100] from r3"
        );
        assert_eq!(
            format!("{}", Step::Write { addr: 0x200, region: 4, value: 42 }),
            "write 0x2a to [0x200] in r4"
        );
        assert_eq!(
            format!("{}", Step::Branch { taken: true }),
            "branch(then)"
        );
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
        assert!(matches!(min.execution[0], Step::Free { .. }));
    }

    #[test]
    fn test_counterexample_minimal_exclusivity() {
        let violation = ViolationPoint::new(InvariantName::Exclusivity, "data race", 0x200);
        let ce = CounterExample::from_violation("err", violation);
        let min = ce.minimal();
        assert!(matches!(min.execution[0], Step::Write { .. }));
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
