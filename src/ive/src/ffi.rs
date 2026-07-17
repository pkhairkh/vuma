//! FFI Safety Verifier — proves states are not read after invalidation by foreign calls.
//!
//! After a non-pure foreign function call that receives a state pointer,
//! the state is "invalidated" — it must be re-initialized before any
//! subsequent read or write. This verifier checks that no invalidated
//! state is accessed without re-initialization.

use std::collections::HashSet;

/// Result of FFI safety verification.
#[derive(Debug, Clone)]
pub struct FfiVerification {
    pub var_name: String,
    pub valid: bool,
    pub error: Option<String>,
}

/// Verify that no invalidated state is accessed.
///
/// `invalidated_vars` is the set of variables invalidated by foreign calls.
/// `accessed_vars` is the set of variables accessed (read/written) after the call.
pub fn verify_ffi_safety(
    invalidated_vars: &HashSet<String>,
    accessed_vars: &HashSet<String>,
) -> Vec<FfiVerification> {
    let mut results = Vec::new();
    for var in accessed_vars {
        if invalidated_vars.contains(var) {
            results.push(FfiVerification {
                var_name: var.clone(),
                valid: false,
                error: Some(format!(
                    "state '{}' accessed after invalidation by foreign call (re-initialize or mark the function #[pure])",
                    var
                )),
            });
        } else {
            results.push(FfiVerification {
                var_name: var.clone(),
                valid: true,
                error: None,
            });
        }
    }
    results
}

pub fn all_valid(results: &[FfiVerification]) -> bool {
    results.iter().all(|r| r.valid)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid_access() {
        let inv = HashSet::new();
        let acc = HashSet::from(["p".to_string()]);
        let results = verify_ffi_safety(&inv, &acc);
        assert!(all_valid(&results));
    }

    #[test]
    fn test_invalidated_access() {
        let inv = HashSet::from(["p".to_string()]);
        let acc = HashSet::from(["p".to_string()]);
        let results = verify_ffi_safety(&inv, &acc);
        assert!(!all_valid(&results));
        assert!(results[0].error.as_ref().unwrap().contains("invalidation"));
    }

    #[test]
    fn test_partial_invalidation() {
        let inv = HashSet::from(["p".to_string()]);
        let acc = HashSet::from(["p".to_string(), "q".to_string()]);
        let results = verify_ffi_safety(&inv, &acc);
        assert!(!all_valid(&results));
        // p is invalid, q is valid
        assert_eq!(results.len(), 2);
    }
}
