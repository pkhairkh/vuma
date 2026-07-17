//! FFI Marshal Pass — converts State<T> args to raw pointers at extern call sites.
//!
//! When calling a foreign (C) function, state references must be flattened
//! to raw pointers (the foreign function doesn't understand State types).
//! After the call:
//! - If the foreign function is declared `#[pure]`, the state is preserved.
//! - Otherwise, the state is "invalidated" — it must be re-initialized before
//!   any subsequent read/write (the foreign function may have modified it).

/// Result of marshalling a state for an FFI call.
#[derive(Debug, Clone)]
pub struct MarshalResult {
    /// The raw pointer expression to pass to the foreign function.
    pub ptr_expr: String,
    /// Whether the state is preserved (true) or invalidated (false) after the call.
    pub preserved: bool,
}

/// Marshal a state-typed variable for an FFI call.
/// Returns the raw pointer to pass + whether the state is preserved.
pub fn marshal_state_for_ffi(
    state_var: &str,
    layout_size: u64,
    is_pure: bool,
) -> MarshalResult {
    // The state's buffer pointer becomes the raw pointer.
    // If the function is pure, the state is preserved.
    // Otherwise, the state is invalidated (foreign function may modify it).
    let _ = layout_size; // layout size reserved for future ABI alignment work
    MarshalResult {
        ptr_expr: state_var.to_string(),
        preserved: is_pure,
    }
}

/// Check if a function is declared #[pure] (foreign functions only).
pub fn is_pure_extern(attrs: &[String]) -> bool {
    attrs.iter().any(|a| a == "pure")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_marshal_pure() {
        let r = marshal_state_for_ffi("p", 8, true);
        assert!(r.preserved);
        assert_eq!(r.ptr_expr, "p");
    }

    #[test]
    fn test_marshal_impure() {
        let r = marshal_state_for_ffi("p", 8, false);
        assert!(!r.preserved);
    }

    #[test]
    fn test_is_pure_extern() {
        assert!(is_pure_extern(&["pure".to_string()]));
        assert!(!is_pure_extern(&[]));
    }
}
