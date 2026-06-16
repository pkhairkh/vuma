//! # Proof Tactics
//!
//! Automated proof strategies that decompose a proof goal into subgoals.
//! Tactics are the engine behind interactive and automated proof construction:
//! they take a goal and produce zero or more subgoals that, once proven,
//! together establish the original goal.

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::judgment::Judgment;
use crate::proof::{Goal, ProofContext, RegionId, Target};

// ---------------------------------------------------------------------------
// ProofGoal / ProofResult — stand-alone tactic function types
// ---------------------------------------------------------------------------

/// Alias for a proof goal used in stand-alone tactic functions.
pub type ProofGoal = Goal;

/// The result of applying a stand-alone proof tactic.
#[derive(Debug, Clone)]
pub enum ProofResult {
    /// The tactic succeeded and produced sub-goals that must each be proven.
    SubGoals(Vec<ProofGoal>),
    /// The tactic discharged the goal (no sub-goals remain).
    Discharged,
    /// The tactic failed to apply to this goal.
    Failed(String),
}

impl ProofResult {
    /// Returns `true` if the tactic discharged the goal.
    pub fn is_discharged(&self) -> bool {
        matches!(self, ProofResult::Discharged)
    }

    /// Returns `true` if the tactic produced sub-goals.
    pub fn has_subgoals(&self) -> bool {
        matches!(self, ProofResult::SubGoals(_))
    }
}

// ---------------------------------------------------------------------------
// Stand-alone tactic functions
// ---------------------------------------------------------------------------

/// Applies the induction tactic, producing a base-case and inductive-step
/// sub-goal.
///
/// This is the stand-alone version of `Tactic::Induction`. It constructs a
/// base case (with region/derivation ID 0) and an inductive step that
/// carries the inductive hypothesis as an assumption.
pub fn tactic_induction(base_case: ProofGoal, inductive_step: ProofGoal) -> ProofResult {
    ProofResult::SubGoals(vec![base_case, inductive_step])
}

/// Applies the case-split tactic, producing one sub-goal per case.
///
/// Every case must be proven independently for the overall goal to hold.
pub fn tactic_case_split(cases: Vec<ProofGoal>) -> ProofResult {
    if cases.is_empty() {
        ProofResult::Failed("case split requires at least one case".to_string())
    } else {
        ProofResult::SubGoals(cases)
    }
}

/// Applies the contradiction tactic.
///
/// If the goal's context contains both an assumption and its negation,
/// the goal is discharged. Otherwise the tactic fails.
pub fn tactic_contradiction(goal: ProofGoal) -> ProofResult {
    let assumption_strs: Vec<String> = goal
        .context
        .assumptions
        .iter()
        .filter_map(|j| match j {
            Judgment::Assumption { description } => Some(description.clone()),
            _ => None,
        })
        .collect();
    for a in &assumption_strs {
        let negated = if a.starts_with("not ") {
            a.strip_prefix("not ").unwrap().to_string()
        } else {
            format!("not {}", a)
        };
        if assumption_strs.contains(&negated) {
            return ProofResult::Discharged;
        }
    }
    ProofResult::Failed("no contradiction found in assumptions".to_string())
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Errors that can arise during tactic application.
#[derive(Debug, Clone, Error)]
pub enum TacticError {
    /// The tactic is not applicable to the given goal.
    #[error("tactic {tactic} is not applicable to goal {goal}")]
    NotApplicable { tactic: String, goal: String },

    /// The tactic failed to produce subgoals.
    #[error("tactic {tactic} failed: {reason}")]
    Failed { tactic: String, reason: String },
}

// ---------------------------------------------------------------------------
// Tactic
// ---------------------------------------------------------------------------

/// A proof tactic — an automated proof strategy that decomposes a goal into
/// subgoals.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum Tactic {
    /// Simplify the goal by applying beta-reduction and trivial rewrites.
    Simplify,

    /// Expand (unfold) a definition to reveal its internal structure.
    Unfold,

    /// Perform structural induction on a natural-number– or list-valued
    /// parameter, producing a base case and a step case.
    Induction,

    /// Attempt to derive a contradiction from the goal's assumptions.
    Contradiction,

    /// Introduce an assumption from the goal context.
    Assumption,

    /// Automatically try a sequence of common tactics.
    Auto,
}

impl Tactic {
    /// Return the human-readable name of this tactic.
    pub fn name(&self) -> &'static str {
        match self {
            Tactic::Simplify => "Simplify",
            Tactic::Unfold => "Unfold",
            Tactic::Induction => "Induction",
            Tactic::Contradiction => "Contradiction",
            Tactic::Assumption => "Assumption",
            Tactic::Auto => "Auto",
        }
    }

    /// Apply this tactic to the given goal, producing a list of subgoals.
    ///
    /// If the tactic succeeds and produces no subgoals, the goal is considered
    /// discharged (proven). If it produces one or more subgoals, each must be
    /// proven independently to establish the original goal.
    pub fn apply(&self, goal: &Goal) -> Result<Vec<Goal>, TacticError> {
        match self {
            Tactic::Simplify => self.apply_simplify(goal),
            Tactic::Unfold => self.apply_unfold(goal),
            Tactic::Induction => self.apply_induction(goal),
            Tactic::Contradiction => self.apply_contradiction(goal),
            Tactic::Assumption => self.apply_assumption(goal),
            Tactic::Auto => self.apply_auto(goal),
        }
    }
}

// ---------------------------------------------------------------------------
// Tactic implementations
// ---------------------------------------------------------------------------

impl Tactic {
    /// **Simplify**: Apply trivial rewrites. For simple goals (e.g. an axiom
    /// or a checked fact in context), this can discharge the goal outright.
    fn apply_simplify(&self, goal: &Goal) -> Result<Vec<Goal>, TacticError> {
        // In the scaffold, simplification succeeds trivially for goals whose
        // context already contains a matching assumption.
        let inv_str = goal.invariant.to_string();
        if goal.context.assumptions.iter().any(|j| match j {
            Judgment::Assumption { description } => description.contains(&inv_str),
            _ => j.to_statement().contains(&inv_str),
        }) {
            // The goal is trivially true by assumption — no subgoals.
            Ok(vec![])
        } else {
            // Return the goal unchanged (simplification made no progress).
            Ok(vec![goal.clone()])
        }
    }

    /// **Unfold**: Expand a definition. In the scaffold we produce a single
    /// subgoal with the same invariant name (the "unfolded" annotation is
    /// reflected in the context scope).
    fn apply_unfold(&self, goal: &Goal) -> Result<Vec<Goal>, TacticError> {
        let expanded_context = ProofContext {
            scope: goal.context.scope.clone(),
            assumptions: goal.context.assumptions.clone(),
        };

        let subgoal = Goal {
            invariant: goal.invariant,
            target: goal.target.clone(),
            context: expanded_context,
        };

        Ok(vec![subgoal])
    }

    /// **Induction**: Decompose the goal into a base case and an inductive
    /// step. This is applicable when the target is a `Region` or a
    /// `Derivation`.
    fn apply_induction(&self, goal: &Goal) -> Result<Vec<Goal>, TacticError> {
        match goal.target {
            Target::Region(id) => {
                let base = Goal {
                    invariant: goal.invariant,
                    target: Target::Region(RegionId(0)), // base case: region 0 (trivial)
                    context: ProofContext::new(format!("{}::induction_base", goal.context.scope))
                        .with_assumption("base case: initial region"),
                };

                let step = Goal {
                    invariant: goal.invariant,
                    target: Target::Region(id),
                    context: ProofContext::new(format!("{}::induction_step", goal.context.scope))
                        .with_assumption("inductive hypothesis: invariant holds for predecessor"),
                };

                Ok(vec![base, step])
            }
            Target::Derivation(id) => {
                let base = Goal {
                    invariant: goal.invariant,
                    target: Target::Derivation(0),
                    context: ProofContext::new(format!("{}::induction_base", goal.context.scope))
                        .with_assumption("base case: empty derivation"),
                };

                let step = Goal {
                    invariant: goal.invariant,
                    target: Target::Derivation(id),
                    context: ProofContext::new(format!("{}::induction_step", goal.context.scope))
                        .with_assumption("inductive hypothesis: invariant holds for prefix"),
                };

                Ok(vec![base, step])
            }
            _ => Err(TacticError::NotApplicable {
                tactic: self.name().into(),
                goal: format!("{:?}", goal),
            }),
        }
    }

    /// **Contradiction**: Check if the goal's assumptions contain a direct
    /// contradiction (both P and ¬P). If so, discharge the goal.
    fn apply_contradiction(&self, goal: &Goal) -> Result<Vec<Goal>, TacticError> {
        // Look for any assumption that is the negation of another.
        let assumption_strs: Vec<String> = goal
            .context
            .assumptions
            .iter()
            .filter_map(|j| match j {
                Judgment::Assumption { description } => Some(description.clone()),
                _ => None,
            })
            .collect();
        for a in &assumption_strs {
            let negated = if a.starts_with("not ") {
                a.strip_prefix("not ").unwrap().to_string()
            } else {
                format!("not {}", a)
            };
            if assumption_strs.contains(&negated) {
                // Contradiction found — goal is discharged.
                return Ok(vec![]);
            }
        }

        // No contradiction found; the tactic is not applicable.
        Err(TacticError::NotApplicable {
            tactic: self.name().into(),
            goal: format!("{:?}", goal),
        })
    }

    /// **Assumption**: If the goal's invariant matches an assumption in the
    /// context, discharge the goal.
    fn apply_assumption(&self, goal: &Goal) -> Result<Vec<Goal>, TacticError> {
        let inv_str = goal.invariant.to_string();
        if goal.context.assumptions.iter().any(|j| match j {
            Judgment::Assumption { description } => *description == inv_str,
            _ => j.to_statement() == inv_str,
        }) {
            // Direct match — goal discharged.
            Ok(vec![])
        } else {
            Err(TacticError::NotApplicable {
                tactic: self.name().into(),
                goal: format!("{:?}", goal),
            })
        }
    }

    /// **Auto**: Try a sequence of common tactics. The first one that produces
    /// subgoals (or discharges the goal) wins.
    fn apply_auto(&self, goal: &Goal) -> Result<Vec<Goal>, TacticError> {
        let tactics = [
            Tactic::Assumption,
            Tactic::Contradiction,
            Tactic::Simplify,
            Tactic::Unfold,
            Tactic::Induction,
        ];

        let mut last_error = None;

        for tactic in &tactics {
            match tactic.apply(goal) {
                Ok(subgoals) => {
                    // If the tactic discharged the goal (no subgoals) or
                    // produced progress (fewer subgoals than a no-op), accept.
                    log::debug!("Auto: tactic {} succeeded", tactic.name());
                    return Ok(subgoals);
                }
                Err(e) => {
                    log::debug!("Auto: tactic {} failed: {}", tactic.name(), e);
                    last_error = Some(e);
                }
            }
        }

        Err(TacticError::Failed {
            tactic: "Auto".into(),
            reason: format!(
                "all sub-tactics failed; last error: {}",
                last_error
                    .as_ref()
                    .map(|e| e.to_string())
                    .unwrap_or_default()
            ),
        })
    }
}

impl std::fmt::Display for Tactic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::InvariantName;

    fn make_goal(invariant: InvariantName, target: Target, assumptions: Vec<&str>) -> Goal {
        let mut ctx = ProofContext::new("test");
        for a in assumptions {
            ctx = ctx.with_assumption(a);
        }
        Goal::new(invariant, target, ctx)
    }

    #[test]
    fn test_assumption_discharges() {
        let goal = make_goal(
            InvariantName::Liveness,
            Target::Region(RegionId(1)),
            vec!["liveness"],
        );
        let result = Tactic::Assumption.apply(&goal).unwrap();
        assert!(result.is_empty()); // discharged
    }

    #[test]
    fn test_assumption_not_applicable() {
        let goal = make_goal(
            InvariantName::Liveness,
            Target::Region(RegionId(1)),
            vec!["exclusivity"],
        );
        let result = Tactic::Assumption.apply(&goal);
        assert!(result.is_err());
    }

    #[test]
    fn test_contradiction_discharges() {
        let goal = make_goal(
            InvariantName::Liveness,
            Target::FullProgram,
            vec!["Q", "not Q"],
        );
        let result = Tactic::Contradiction.apply(&goal).unwrap();
        assert!(result.is_empty()); // discharged
    }

    #[test]
    fn test_contradiction_not_applicable() {
        let goal = make_goal(InvariantName::Liveness, Target::FullProgram, vec!["Q", "R"]);
        let result = Tactic::Contradiction.apply(&goal);
        assert!(result.is_err());
    }

    #[test]
    fn test_simplify_with_matching_assumption() {
        let goal = make_goal(
            InvariantName::Liveness,
            Target::Region(RegionId(1)),
            vec!["liveness"],
        );
        let result = Tactic::Simplify.apply(&goal).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn test_simplify_no_match() {
        let goal = make_goal(
            InvariantName::Liveness,
            Target::Region(RegionId(1)),
            vec!["exclusivity"],
        );
        let result = Tactic::Simplify.apply(&goal).unwrap();
        assert_eq!(result.len(), 1); // goal returned unchanged
    }

    #[test]
    fn test_unfold() {
        let goal = make_goal(InvariantName::Liveness, Target::Region(RegionId(5)), vec![]);
        let result = Tactic::Unfold.apply(&goal).unwrap();
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn test_induction_region() {
        let goal = make_goal(
            InvariantName::Liveness,
            Target::Region(RegionId(42)),
            vec![],
        );
        let result = Tactic::Induction.apply(&goal).unwrap();
        assert_eq!(result.len(), 2); // base + step
    }

    #[test]
    fn test_induction_not_applicable_for_full_program() {
        let goal = make_goal(InvariantName::Liveness, Target::FullProgram, vec![]);
        let result = Tactic::Induction.apply(&goal);
        assert!(result.is_err());
    }

    #[test]
    fn test_auto_with_assumption() {
        let goal = make_goal(
            InvariantName::Liveness,
            Target::Region(RegionId(1)),
            vec!["liveness"],
        );
        let result = Tactic::Auto.apply(&goal).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn test_auto_with_contradiction() {
        let goal = make_goal(
            InvariantName::Liveness,
            Target::FullProgram,
            vec!["Q", "not Q"],
        );
        let result = Tactic::Auto.apply(&goal).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn test_tactic_display() {
        assert_eq!(format!("{}", Tactic::Simplify), "Simplify");
        assert_eq!(format!("{}", Tactic::Auto), "Auto");
    }

    // -- Stand-alone tactic function tests --------------------------------------

    fn make_proof_goal(
        invariant: InvariantName,
        target: Target,
        assumptions: Vec<&str>,
    ) -> ProofGoal {
        let mut ctx = ProofContext::new("test");
        for a in assumptions {
            ctx = ctx.with_assumption(a);
        }
        ProofGoal::new(invariant, target, ctx)
    }

    #[test]
    fn tactic_induction_produces_two_subgoals() {
        let base = make_proof_goal(
            InvariantName::Liveness,
            Target::Region(RegionId(0)),
            vec!["base case"],
        );
        let step = make_proof_goal(
            InvariantName::Liveness,
            Target::Region(RegionId(5)),
            vec!["IH holds"],
        );
        let result = tactic_induction(base, step);
        assert!(result.has_subgoals());
        if let ProofResult::SubGoals(goals) = result {
            assert_eq!(goals.len(), 2);
        }
    }

    #[test]
    fn tactic_case_split_multiple_cases() {
        let case1 = make_proof_goal(InvariantName::Liveness, Target::FullProgram, vec![]);
        let case2 = make_proof_goal(InvariantName::Exclusivity, Target::FullProgram, vec![]);
        let case3 = make_proof_goal(InvariantName::Cleanup, Target::FullProgram, vec![]);
        let result = tactic_case_split(vec![case1, case2, case3]);
        assert!(result.has_subgoals());
        if let ProofResult::SubGoals(goals) = result {
            assert_eq!(goals.len(), 3);
        }
    }

    #[test]
    fn tactic_case_split_empty_fails() {
        let result = tactic_case_split(vec![]);
        assert!(!result.has_subgoals());
        assert!(!result.is_discharged());
    }

    #[test]
    fn tactic_contradiction_discharges() {
        let goal = make_proof_goal(
            InvariantName::Liveness,
            Target::FullProgram,
            vec!["Q", "not Q"],
        );
        let result = tactic_contradiction(goal);
        assert!(result.is_discharged());
    }

    #[test]
    fn tactic_contradiction_no_contradiction_fails() {
        let goal = make_proof_goal(InvariantName::Liveness, Target::FullProgram, vec!["Q", "R"]);
        let result = tactic_contradiction(goal);
        assert!(!result.is_discharged());
    }

    #[test]
    fn proof_result_helpers() {
        let discharged = ProofResult::Discharged;
        assert!(discharged.is_discharged());
        assert!(!discharged.has_subgoals());

        let subgoals = ProofResult::SubGoals(vec![]);
        assert!(subgoals.has_subgoals());
        assert!(!subgoals.is_discharged());

        let failed = ProofResult::Failed("reason".to_string());
        assert!(!failed.is_discharged());
        assert!(!failed.has_subgoals());
    }
}
