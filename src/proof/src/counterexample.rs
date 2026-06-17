//! # Counterexample Generation
//!
//! When a proof fails or an invariant is violated, a counterexample provides
//! a concrete execution trace that demonstrates the violation. This module
//! provides data structures for representing counterexamples and methods for
//! constructing minimal ones.
//!
//! ## Soundness note (W9)
//!
//! Real counterexamples must come from the **IVE verifiers** — W7 (Liveness,
//! Origin) and W8 (Exclusivity, Interpretation) — which populate the
//! [`CounterExample::execution`] field with a real SCG path. The previous
//! implementation of [`CounterExample::minimal`] *fabricated* a template
//! trace from the invariant name (e.g. `[Free{region:0}, Read{addr:0,
//! region:0}]` for every Liveness violation) regardless of the actual
//! program. That fabrication has been **removed**. When `execution` is
//! empty, `minimal()` returns the counterexample unchanged (no fabrication)
//! and emits a `log::warn!`; callers that prefer an explicit error should
//! use [`CounterExample::try_minimal`], which returns
//! [`MinimalError::NoRealTrace`].

use serde::{Deserialize, Serialize};
use thiserror::Error;

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
// MinimalError
// ---------------------------------------------------------------------------

/// Errors that can arise during counterexample minimization.
///
/// Returned by [`CounterExample::try_minimal`] when the counterexample does
/// not carry a real execution trace.
#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum MinimalError {
    /// `CounterExample::try_minimal` was called on a counterexample with an
    /// empty `execution` trace.
    ///
    /// Real counterexamples must come from the IVE verifiers — W7 for
    /// Liveness/Origin, W8 for Exclusivity/Interpretation — which populate
    /// `execution` with a real SCG path. The previous `minimal()`
    /// implementation fabricated a template trace from the invariant name
    /// (e.g. `[Free{region:0}, Read{addr:0,region:0}]` for every Liveness
    /// violation); that fabrication was unsound and has been removed (W9).
    #[error(
        "cannot construct a minimal counterexample without the actual SCG \
         path — use the verifier's built-in counterexample extraction \
         instead (invariant: {invariant})"
    )]
    NoRealTrace {
        /// The invariant for which the counterexample was requested.
        invariant: InvariantName,
    },
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
    ///
    /// **Note (W9):** an empty `execution` means no real SCG path is
    /// available. [`Self::minimal`] will refuse to fabricate one; callers
    /// should populate `execution` with a real path from the IVE verifiers
    /// (W7/W8) before invoking `minimal()`.
    pub fn from_violation(_msg: &str, violation: ViolationPoint) -> Self {
        Self {
            execution: Vec::new(),
            violation,
        }
    }

    /// Construct a minimal counterexample by applying delta-debugging style
    /// trace minimization.
    ///
    /// Starting from a **real** execution trace (populated by the IVE
    /// verifiers — W7/W8), this method tries removing each step one by one
    /// and checks whether the violation is still demonstrated. Steps that
    /// don't contribute to the violation are removed.
    ///
    /// # Soundness note (W9)
    ///
    /// **This method does not fabricate a counterexample trace from the
    /// invariant name.** The previous implementation called
    /// `infer_minimal_trace()` which produced a template trace (e.g.
    /// `[Free{region:0}, Read{addr:0,region:0}]` for every Liveness
    /// violation) regardless of the actual program — this was unsound
    /// because the fabricated trace was not a real execution of any
    /// program. That fabrication has been **removed**.
    ///
    /// When `self.execution` is **empty**, this method returns a clone of
    /// `self` (with the empty trace preserved) and emits a `log::warn!`
    /// directing the caller to the verifier's built-in counterexample
    /// extraction. Callers that prefer an explicit error should use
    /// [`Self::try_minimal`].
    ///
    /// # Algorithm
    ///
    /// 1. If the trace is empty, return `self` unchanged (no fabrication).
    /// 2. Start with all steps as "necessary".
    /// 3. For each step, try removing it and check if the violation still
    ///    holds.
    /// 4. If yes, keep it removed; otherwise restore it.
    /// 5. Return the minimized set.
    pub fn minimal(&self) -> CounterExample {
        // If the trace is empty, we REFUSE to fabricate one. Return self
        // unchanged (with the empty trace) and log a warning.
        if self.execution.is_empty() {
            log::warn!(
                "CounterExample::minimal() called with an empty execution \
                 trace for invariant '{}'. Refusing to fabricate a template \
                 trace — real counterexamples must come from the IVE \
                 verifiers (W7/W8). Use the verifier's built-in \
                 counterexample extraction instead, or use \
                 CounterExample::try_minimal() for an explicit error.",
                self.violation.invariant
            );
            return self.clone();
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

    /// Like [`Self::minimal`], but returns an explicit error when no real
    /// execution trace is available, rather than silently returning an empty
    /// counterexample.
    ///
    /// # Errors
    ///
    /// Returns [`MinimalError::NoRealTrace`] when `self.execution` is empty.
    /// In that case the caller should obtain a real counterexample from the
    /// IVE verifiers (W7 for Liveness/Origin, W8 for Exclusivity/
    /// Interpretation) instead.
    ///
    /// # Example
    ///
    /// ```ignore
    /// use vuma_proof::counterexample::{CounterExample, ViolationPoint};
    /// use vuma_proof::proof::{InvariantName};
    ///
    /// let ce = CounterExample::from_violation(
    ///     "err",
    ///     ViolationPoint::new(InvariantName::Liveness, "uaf", 0x100),
    /// );
    /// // No real trace available — explicit error:
    /// assert!(ce.try_minimal().is_err());
    /// ```
    pub fn try_minimal(&self) -> Result<CounterExample, MinimalError> {
        if self.execution.is_empty() {
            return Err(MinimalError::NoRealTrace {
                invariant: self.violation.invariant,
            });
        }
        Ok(self.minimal())
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
                has_post_free_access
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
        // W9: with an empty execution trace, minimal() must REFUSE to
        // fabricate a template trace. It returns the counterexample with
        // an empty trace (and logs a warning). The previous behavior —
        // fabricating [Free, Read] — was unsound and has been removed.
        let violation = ViolationPoint::new(InvariantName::Liveness, "use after free", 0x100);
        let ce = CounterExample::from_violation("err", violation);
        let min = ce.minimal();
        assert!(
            min.is_empty(),
            "minimal() on an empty trace must not fabricate steps; got {} steps",
            min.execution.len()
        );
        assert_eq!(min.violation.invariant, InvariantName::Liveness);
    }

    #[test]
    fn test_counterexample_minimal_exclusivity() {
        // W9: same as above — no fabrication for Exclusivity.
        let violation = ViolationPoint::new(InvariantName::Exclusivity, "data race", 0x200);
        let ce = CounterExample::from_violation("err", violation);
        let min = ce.minimal();
        assert!(
            min.is_empty(),
            "minimal() on an empty trace must not fabricate steps; got {} steps",
            min.execution.len()
        );
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
        ce.add_step(Step::Read {
            addr: 0x10,
            region: 1,
        }); // necessary
        ce.add_step(Step::Write {
            addr: 0x20,
            region: 99,
            value: 0,
        }); // unnecessary

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

    // -- W9 soundness tests --------------------------------------------------

    #[test]
    fn test_try_minimal_empty_trace_errors() {
        // try_minimal() on an empty trace must return an explicit error,
        // not a fabricated counterexample.
        let violation = ViolationPoint::new(InvariantName::Liveness, "uaf", 0x100);
        let ce = CounterExample::from_violation("err", violation);
        let result = ce.try_minimal();
        assert!(result.is_err());
        match result {
            Err(MinimalError::NoRealTrace { invariant }) => {
                assert_eq!(invariant, InvariantName::Liveness);
            }
            other => panic!("expected NoRealTrace error, got {:?}", other),
        }
    }

    #[test]
    fn test_try_minimal_empty_trace_error_message() {
        // The error message must point the caller at the IVE verifiers.
        let violation = ViolationPoint::new(InvariantName::Exclusivity, "race", 0x200);
        let ce = CounterExample::from_violation("err", violation);
        let err = ce.try_minimal().unwrap_err();
        let msg = format!("{}", err);
        assert!(
            msg.contains("verifier's built-in counterexample extraction"),
            "error should direct caller to verifier, got: {}",
            msg
        );
        assert!(msg.contains("exclusivity"));
    }

    #[test]
    fn test_try_minimal_real_trace_succeeds() {
        // try_minimal() on a real (non-empty) trace must succeed and
        // return a minimized counterexample.
        let violation = ViolationPoint::new(InvariantName::Liveness, "uaf", 0x100);
        let mut ce = CounterExample::from_violation("err", violation);
        ce.add_step(Step::Free { region: 7 });
        ce.add_step(Step::Read {
            addr: 0,
            region: 7,
        });

        let min = ce.try_minimal().expect("real trace should minimize ok");
        // Both steps are necessary for the Liveness violation (Free + Read
        // on the same region), so minimization keeps them.
        assert_eq!(min.execution.len(), 2);
    }

    #[test]
    fn test_minimal_does_not_fabricate_for_any_invariant() {
        // For every invariant, an empty trace must yield an empty trace
        // after minimal() — no fabrication. This is the regression test
        // for the W9 soundness fix.
        for inv in [
            InvariantName::Liveness,
            InvariantName::Exclusivity,
            InvariantName::Cleanup,
            InvariantName::Origin,
            InvariantName::Interpretation,
        ] {
            let violation = ViolationPoint::new(inv, "test", 0);
            let ce = CounterExample::from_violation("err", violation);
            let min = ce.minimal();
            assert_eq!(
                min.execution.len(),
                0,
                "minimal() fabricated {} step(s) for {:?} — fabrication must be removed (W9)",
                min.execution.len(),
                inv
            );
            assert_eq!(min.violation.invariant, inv);
        }
    }

    #[test]
    fn test_minimal_error_no_real_trace_display() {
        // The MinimalError must be Display-able and include the invariant.
        let err = MinimalError::NoRealTrace {
            invariant: InvariantName::Cleanup,
        };
        let s = format!("{}", err);
        assert!(s.contains("cleanup"));
        assert!(s.contains("counterexample"));
    }
}
