//! BD Error Reporting
//!
//! This module provides human-readable error messages for BD system errors,
//! including source locations and contextual information.

use crate::capd::Capability;
use crate::inference::InferenceError;
use crate::unify::UnificationError;
use std::fmt;

// ---------------------------------------------------------------------------
// BdError — unified error enum
// ---------------------------------------------------------------------------

/// Errors that can arise in the BD system.
#[derive(Debug, Clone, PartialEq)]
pub enum BdError {
    /// Inference failed.
    Inference(InferenceError),
    /// Unification failed.
    Unification(UnificationError),
    /// Trait compatibility check failed.
    TraitIncompatible {
        /// Description of what the trait requires.
        trait_requires: String,
        /// Description of what the impl provides.
        impl_provides: String,
    },
    /// RepD incompatibility at a specific location.
    RepDIncompatible {
        /// Expected representation description.
        expected: String,
        /// Actual representation description.
        actual: String,
        /// Source location, if known.
        location: Option<SourceLocation>,
    },
    /// CapD violation — missing capability.
    CapDMissing {
        /// The required capability.
        required: Capability,
        /// Source location, if known.
        location: Option<SourceLocation>,
    },
    /// RelD inconsistency.
    RelDInconsistent {
        /// Detail message.
        detail: String,
        /// Source location, if known.
        location: Option<SourceLocation>,
    },
    /// Generic instantiation error.
    GenericInstantiation {
        /// The type parameter name.
        param_name: String,
        /// Why it failed.
        reason: String,
    },
    /// Incremental re-inference error.
    Incremental {
        /// Nodes that could not be re-inferred.
        failed_nodes: Vec<u64>,
    },
    /// Widening convergence failure.
    WideningFailed {
        /// Detail message.
        detail: String,
    },
    /// Invalid operation on a RepD — e.g., field access on a non-Struct variant,
    /// or field index out of bounds.
    InvalidOperation {
        /// Description of the invalid operation.
        operation: String,
        /// Detail message explaining why it failed.
        detail: String,
    },
}

impl fmt::Display for BdError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BdError::Inference(e) => write!(f, "BD inference error: {e}"),
            BdError::Unification(e) => write!(f, "BD unification error: {e}"),
            BdError::TraitIncompatible {
                trait_requires,
                impl_provides,
            } => write!(
                f,
                "trait compatibility error: requires [{trait_requires}], provides [{impl_provides}]"
            ),
            BdError::RepDIncompatible {
                expected,
                actual,
                location,
            } => {
                if let Some(loc) = location {
                    write!(f, "RepD incompatibility at {loc}: expected {expected}, got {actual}")
                } else {
                    write!(f, "RepD incompatibility: expected {expected}, got {actual}")
                }
            }
            BdError::CapDMissing {
                required,
                location,
            } => {
                if let Some(loc) = location {
                    write!(f, "CapD violation at {loc}: missing {required:?}")
                } else {
                    write!(f, "CapD violation: missing {required:?}")
                }
            }
            BdError::RelDInconsistent { detail, location } => {
                if let Some(loc) = location {
                    write!(f, "RelD inconsistency at {loc}: {detail}")
                } else {
                    write!(f, "RelD inconsistency: {detail}")
                }
            }
            BdError::GenericInstantiation { param_name, reason } => {
                write!(f, "generic instantiation error: parameter '{param_name}': {reason}")
            }
            BdError::Incremental { failed_nodes } => {
                let nodes: Vec<String> = failed_nodes.iter().map(|n| format!("NodeId({n})")).collect();
                write!(f, "incremental re-inference error: failed for nodes [{}]", nodes.join(", "))
            }
            BdError::WideningFailed { detail } => {
                write!(f, "widening convergence failure: {detail}")
            }
            BdError::InvalidOperation { operation, detail } => {
                write!(f, "invalid operation: {operation}: {detail}")
            }
        }
    }
}

impl std::error::Error for BdError {}

/// A source location for error reporting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceLocation {
    /// File name or path.
    pub file: Option<String>,
    /// Line number (1-based).
    pub line: Option<u64>,
    /// Column number (1-based).
    pub column: Option<u64>,
}

impl fmt::Display for SourceLocation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (self.file.as_deref(), self.line, self.column) {
            (Some(file), Some(line), Some(col)) => write!(f, "{file}:{line}:{col}"),
            (Some(file), Some(line), None) => write!(f, "{file}:{line}"),
            (Some(file), None, None) => write!(f, "{file}"),
            (None, Some(line), Some(col)) => write!(f, "line {line}:{col}"),
            (None, Some(line), None) => write!(f, "line {line}"),
            _ => write!(f, "<unknown>"),
        }
    }
}

impl SourceLocation {
    /// Create a source location from optional fields.
    pub fn new(file: Option<String>, line: Option<u64>, column: Option<u64>) -> Self {
        Self { file, line, column }
    }

    /// Create an unknown source location.
    pub fn unknown() -> Self {
        Self {
            file: None,
            line: None,
            column: None,
        }
    }
}

// ---------------------------------------------------------------------------
// format_bd_error
// ---------------------------------------------------------------------------

/// Format a [`BdError`] into a human-readable error message with source
/// location context.
///
/// The `source` parameter provides the original source text (or a relevant
/// excerpt) for context. When available, the error message includes the
/// source line and a caret pointing to the relevant position.
pub fn format_bd_error(error: &BdError, source: &str) -> String {
    let location_str = match error {
        BdError::Inference(ref e) => {
            // Try to extract location from inference error
            match e {
                InferenceError::RepDIncompatible { source, .. } => {
                    format!(" at node {source}")
                }
                InferenceError::CapDViolation { node, .. } => {
                    format!(" at node {node}")
                }
                InferenceError::RelDInconsistent { node, .. } => {
                    format!(" at node {node}")
                }
                InferenceError::UninferredNode(node) => {
                    format!(" at node {node}")
                }
                InferenceError::SecurityDowngrade { source, .. } => {
                    format!(" from node {source}")
                }
                InferenceError::CircularOutlives { node } => {
                    format!(" involving node {node}")
                }
                _ => String::new(),
            }
        }
        BdError::RepDIncompatible { location, .. } => location
            .as_ref()
            .map(|l| format!(" at {l}"))
            .unwrap_or_default(),
        BdError::CapDMissing { location, .. } => location
            .as_ref()
            .map(|l| format!(" at {l}"))
            .unwrap_or_default(),
        BdError::RelDInconsistent { location, .. } => location
            .as_ref()
            .map(|l| format!(" at {l}"))
            .unwrap_or_default(),
        _ => String::new(),
    };

    let main_msg = match error {
        BdError::Inference(e) => format!("BD inference error{location_str}: {e}"),
        BdError::Unification(e) => format!("BD unification error{location_str}: {e}"),
        BdError::TraitIncompatible {
            trait_requires,
            impl_provides,
        } => format!(
            "trait compatibility error{location_str}: trait requires [{trait_requires}] but impl provides [{impl_provides}]"
        ),
        BdError::RepDIncompatible {
            expected,
            actual,
            location,
        } => {
            let loc = location
                .as_ref()
                .map(|l| format!(" at {l}"))
                .unwrap_or_default();
            format!("RepD incompatibility{loc}: expected {expected}, got {actual}")
        }
        BdError::CapDMissing {
            required,
            location,
        } => {
            let loc = location
                .as_ref()
                .map(|l| format!(" at {l}"))
                .unwrap_or_default();
            format!("CapD violation{loc}: missing required capability {required:?}")
        }
        BdError::RelDInconsistent { detail, location } => {
            let loc = location
                .as_ref()
                .map(|l| format!(" at {l}"))
                .unwrap_or_default();
            format!("RelD inconsistency{loc}: {detail}")
        }
        BdError::GenericInstantiation { param_name, reason } => {
            format!("generic instantiation error: parameter '{param_name}': {reason}")
        }
        BdError::Incremental { failed_nodes } => {
            let nodes: Vec<String> = failed_nodes.iter().map(|n| format!("NodeId({n})")).collect();
            format!(
                "incremental re-inference error: failed for nodes [{}]",
                nodes.join(", ")
            )
        }
        BdError::WideningFailed { detail } => {
            format!("widening convergence failure{location_str}: {detail}")
        }
        BdError::InvalidOperation { operation, detail } => {
            format!("invalid operation: {operation}: {detail}")
        }
    };

    // Add source context if we have a line number
    if let Some(line_num) = extract_line_number(error) {
        if let Some(source_line) = source.lines().nth(line_num.saturating_sub(1) as usize) {
            return format!(
                "{main_msg}\n  --> line {line_num}\n  | {source_line}\n  | {}^",
                " ".repeat(0)
            );
        }
    }

    main_msg
}

/// Extract a line number from a BdError, if available.
fn extract_line_number(error: &BdError) -> Option<u64> {
    match error {
        BdError::RepDIncompatible { location, .. } => location.as_ref().and_then(|l| l.line),
        BdError::CapDMissing { location, .. } => location.as_ref().and_then(|l| l.line),
        BdError::RelDInconsistent { location, .. } => location.as_ref().and_then(|l| l.line),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capd::Capability;
    use crate::inference::InferenceError;
    use crate::unify::UnificationError;
    use vuma_scg::node::NodeId;

    #[test]
    fn test_format_inference_error_cycle() {
        let error = BdError::Inference(InferenceError::CycleDetected);
        let msg = format_bd_error(&error, "");
        assert!(msg.contains("BD inference error"));
        assert!(msg.contains("cycle"));
    }

    #[test]
    fn test_format_repd_incompatible_with_location() {
        let error = BdError::RepDIncompatible {
            expected: "byte(4,4)".to_string(),
            actual: "byte(8,8)".to_string(),
            location: Some(SourceLocation::new(
                Some("test.vu".to_string()),
                Some(10),
                Some(5),
            )),
        };
        let source = "line 1\nline 2\nline 3\nline 4\nline 5\nline 6\nline 7\nline 8\nline 9\nlet x: i32 = 42";
        let msg = format_bd_error(&error, source);
        assert!(msg.contains("RepD incompatibility"));
        assert!(msg.contains("test.vu:10:5"));
        assert!(msg.contains("expected byte(4,4)"));
        assert!(msg.contains("got byte(8,8)"));
    }

    #[test]
    fn test_format_capd_missing() {
        let error = BdError::CapDMissing {
            required: Capability::Write,
            location: Some(SourceLocation::new(
                Some("main.vu".to_string()),
                Some(3),
                None,
            )),
        };
        let source = "fn foo() {}\nlet x = 1\nx.field = 42";
        let msg = format_bd_error(&error, source);
        assert!(msg.contains("CapD violation"));
        assert!(msg.contains("Write"));
        assert!(msg.contains("main.vu:3"));
    }

    #[test]
    fn test_format_trait_incompatible() {
        let error = BdError::TraitIncompatible {
            trait_requires: "Read+Write".to_string(),
            impl_provides: "Read".to_string(),
        };
        let msg = format_bd_error(&error, "");
        assert!(msg.contains("trait compatibility error"));
        assert!(msg.contains("Read+Write"));
        assert!(msg.contains("Read"));
    }

    #[test]
    fn test_format_generic_instantiation_error() {
        let error = BdError::GenericInstantiation {
            param_name: "T".to_string(),
            reason: "no concrete type provided".to_string(),
        };
        let msg = format_bd_error(&error, "");
        assert!(msg.contains("generic instantiation error"));
        assert!(msg.contains("parameter 'T'"));
        assert!(msg.contains("no concrete type provided"));
    }
}
