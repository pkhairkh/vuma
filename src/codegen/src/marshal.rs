//! FFI Marshal Pass — the FFI mode matrix for VUMA 2.0.
//!
//! Every foreign call is classified into an argument mode per argument, a
//! return mode, and optionally a callback mode. This module provides the
//! classification helpers and the marshal-result types that the codegen
//! bridge (scg_to_ir.rs, Wave 5) consumes.
//!
//! # Argument modes
//! - `Borrow` (`#[borrow]`): C reads the State's buffer directly (zero-copy),
//!   does not mutate, does not hold the pointer. State is PRESERVED after.
//! - `Invalidate` (default, no attr): C may mutate. State is INVALIDATED —
//!   must re-init before next read/write.
//! - `Marshal` (`#[marshal]`): copy the State's data to the scratchpad, pass
//!   the scratchpad pointer. State is untouched.
//! - `ForeignPass` (`#[foreign(raw)]` on the arg's layout): pass the `raw`
//!   field value (the C pointer), not the buffer pointer.
//!
//! # Return modes
//! - `Scalar` (default): the C return is a plain scalar (i64/u64/bool/Address).
//! - `Unmarshal` (`#[unmarshal(Layout)]`): copy the C return into ___pmt_buffer
//!   as a fresh State<Layout>. ALWAYS a copy, never a borrow.
//! - `ForeignWrap` (`#[foreign_return(raw)]`): wrap the C return (a raw pointer)
//!   into a State<ForeignLayout> whose `raw` field holds the pointer.
//!
//! # Sacred invariant
//! Scratchpad memory is NEVER aliased by ___pmt_buffer. The StateRead/
//! StateWrite/StateTransform verifiers never see scratchpad memory. A State<T>
//! can never alias scratchpad memory (enforced by the type system — scratch
//! pointers are Address, never State<T>).

// ── Attribute info (local, to avoid a vuma-parser dependency) ────────────

/// A lightweight view of an FFI attribute, decoupled from the parser's
/// `Attribute` type so this module stays within the codegen crate's
/// existing dependency graph (vuma-scg only).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttrInfo {
    /// The attribute name (e.g. "borrow", "marshal", "foreign").
    pub name: String,
    /// Optional single value (e.g. "raw" for #[foreign(raw)], "Response"
    /// for #[unmarshal(Response)]).
    pub value: Option<String>,
}

impl AttrInfo {
    /// Construct an attribute with no value: `#[borrow]`.
    pub fn bare(name: &str) -> Self {
        Self {
            name: name.to_string(),
            value: None,
        }
    }
    /// Construct an attribute with a single value: `#[foreign(raw)]`.
    pub fn with_value(name: &str, value: &str) -> Self {
        Self {
            name: name.to_string(),
            value: Some(value.to_string()),
        }
    }
}

// ── Argument modes ───────────────────────────────────────────────────────

/// The FFI argument mode for a single extern-call argument.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArgMode {
    /// `#[borrow]` — zero-copy read-only; state preserved after the call.
    Borrow,
    /// Default (no attr) — C may mutate; state invalidated after the call.
    Invalidate,
    /// `#[marshal]` — copy to scratchpad; state untouched.
    Marshal,
    /// `#[may_retain]` — C may stash the pointer; force scratchpad routing.
    /// Same runtime behavior as Marshal, but semantically distinct (the
    /// programmer is asserting C might hold the pointer past return).
    MayRetain,
    /// `#[foreign(raw)]` on the arg's layout — pass the `raw` field value.
    ForeignPass,
}

/// Classify an extern-call argument's mode from its attributes and layout.
///
/// `param_attrs` — the attributes on this parameter (e.g. `#[borrow]`).
/// `layout_is_foreign` — true if the argument's layout has `#[foreign(raw)]`.
///
/// Precedence: `#[may_retain]` > `#[marshal]` > `#[borrow]` > `#[foreign]` >
/// default (Invalidate). Only one mode applies; the first matching attribute
/// wins. If the layout is `#[foreign(raw)]` and no param attr overrides, the
/// mode is ForeignPass.
pub fn classify_arg_mode(param_attrs: &[AttrInfo], layout_is_foreign: bool) -> ArgMode {
    let has_attr = |name: &str| param_attrs.iter().any(|a| a.name == name);
    // Precedence order.
    if has_attr("may_retain") {
        return ArgMode::MayRetain;
    }
    if has_attr("marshal") {
        return ArgMode::Marshal;
    }
    if has_attr("borrow") {
        return ArgMode::Borrow;
    }
    if layout_is_foreign {
        return ArgMode::ForeignPass;
    }
    ArgMode::Invalidate
}

// ── Return modes ─────────────────────────────────────────────────────────

/// The FFI return mode for an extern function.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReturnMode {
    /// Default — the C return is a plain scalar.
    Scalar,
    /// `#[unmarshal(Layout)]` — copy the C return into ___pmt_buffer.
    /// Carries the target layout name.
    Unmarshal(String),
    /// `#[foreign_return(raw)]` — wrap the C pointer into State<ForeignLayout>.
    /// Carries the field name (typically "raw").
    ForeignWrap(String),
}

/// Classify an extern fn's return mode from its declaration attributes.
///
/// `decl_attrs` — the attributes on the extern fn itself (not on params).
pub fn classify_return_mode(decl_attrs: &[AttrInfo]) -> ReturnMode {
    for attr in decl_attrs {
        if attr.name == "unmarshal" {
            return ReturnMode::Unmarshal(
                attr.value.clone().unwrap_or_default(),
            );
        }
        if attr.name == "foreign_return" {
            return ReturnMode::ForeignWrap(
                attr.value.clone().unwrap_or_else(|| "raw".to_string()),
            );
        }
    }
    ReturnMode::Scalar
}

// ── Callback mode ────────────────────────────────────────────────────────

/// Whether an extern fn is a callback-capable function (has `#[callback]`).
pub fn is_callback_fn(decl_attrs: &[AttrInfo]) -> bool {
    decl_attrs.iter().any(|a| a.name == "callback")
}

/// Whether an extern fn consumes its State argument (has `#[foreign_consume]`).
/// Returns the field name if so (typically "raw").
pub fn foreign_consume_field(decl_attrs: &[AttrInfo]) -> Option<String> {
    decl_attrs.iter().find_map(|a| {
        if a.name == "foreign_consume" {
            Some(a.value.clone().unwrap_or_else(|| "raw".to_string()))
        } else {
            None
        }
    })
}

// ── Marshal result ───────────────────────────────────────────────────────

/// Result of marshalling a state for an FFI call.
///
/// This is what the codegen bridge consumes to decide how to emit the
/// argument at the call site.
#[derive(Debug, Clone)]
pub struct MarshalResult {
    /// The mode determined for this argument.
    pub mode: ArgMode,
    /// The raw pointer expression to pass to the foreign function.
    /// - Borrow/Invalidate: `___pmt_buffer_base + offset` (the state's buffer).
    /// - Marshal/MayRetain: `___ffi_scratch_alloc(N)` (scratchpad pointer).
    /// - ForeignPass: the value of the `raw` field (a u64 C pointer).
    pub ptr_expr: String,
    /// Whether the state is preserved (true) or invalidated (false) after
    /// the call. Borrow → true; Invalidate → false; Marshal/MayRetain → true
    /// (the state was never handed to C); ForeignPass → depends on whether
    /// the fn is also #[foreign_consume].
    pub preserved: bool,
}

/// Marshal a state-typed variable for an FFI call.
///
/// This is the legacy API kept for backward compatibility with existing
/// callers. New code should use `marshal_arg` with an explicit `ArgMode`.
pub fn marshal_state_for_ffi(
    state_var: &str,
    layout_size: u64,
    is_pure: bool,
) -> MarshalResult {
    let _ = layout_size;
    let mode = if is_pure {
        ArgMode::Borrow
    } else {
        ArgMode::Invalidate
    };
    MarshalResult {
        mode,
        ptr_expr: state_var.to_string(),
        preserved: is_pure,
    }
}

/// Marshal an argument given its classified mode.
///
/// `state_var` — the name of the State variable being passed.
/// `buffer_offset` — the byte offset of the State within ___pmt_buffer
///   (for Borrow/Invalidate modes).
/// `layout_size` — the size of the State's layout (for Marshal mode, to know
///   how many bytes to copy to the scratchpad).
pub fn marshal_arg(
    state_var: &str,
    mode: ArgMode,
    buffer_offset: u64,
    layout_size: u64,
) -> MarshalResult {
    let (ptr_expr, preserved) = match mode {
        ArgMode::Borrow => {
            // Zero-copy: pass ___pmt_buffer_base + offset. State preserved.
            (format!("___pmt_buffer_base + {}", buffer_offset), true)
        }
        ArgMode::Invalidate => {
            // Pass ___pmt_buffer_base + offset. State invalidated (C may mutate).
            (format!("___pmt_buffer_base + {}", buffer_offset), false)
        }
        ArgMode::Marshal => {
            // Copy to scratchpad, pass scratch pointer. State untouched.
            (
                format!("___ffi_scratch_alloc({}, {})", state_var, layout_size),
                true,
            )
        }
        ArgMode::MayRetain => {
            // Same as Marshal — scratchpad routing. State untouched.
            (
                format!("___ffi_scratch_alloc({}, {})", state_var, layout_size),
                true,
            )
        }
        ArgMode::ForeignPass => {
            // Pass the `raw` field value (the C pointer), not the buffer.
            // The state is preserved unless the fn is also #[foreign_consume]
            // (handled separately by the ForeignConsume SCG node).
            (format!("{}.raw", state_var), true)
        }
    };
    MarshalResult {
        mode,
        ptr_expr,
        preserved,
    }
}

// ── Layout foreign-attribute helper ──────────────────────────────────────

/// Check if a layout has the #[foreign(raw)] attribute.
/// `layout_attrs` — the attributes on the layout declaration.
/// Returns the field name if so (typically "raw").
pub fn foreign_layout_field(layout_attrs: &[AttrInfo]) -> Option<String> {
    layout_attrs.iter().find_map(|a| {
        if a.name == "foreign" {
            Some(a.value.clone().unwrap_or_else(|| "raw".to_string()))
        } else {
            None
        }
    })
}

// ── Legacy #[pure] helper (kept for backward compat) ─────────────────────

/// Check if a function is declared #[pure] (the legacy name for #[borrow]).
pub fn is_pure_extern(attrs: &[String]) -> bool {
    attrs.iter().any(|a| a == "pure")
}

// ── Builtin function names ───────────────────────────────────────────────

/// The builtin function name for marshalling a State<String> to a NUL-terminated
/// C string in the scratchpad. Returns an Address into ___ffi_scratch.
pub const MARSHAL_CSTR_FN: &str = "marshal_cstr";

/// The builtin function name for unmarshalling a C pointer + length into a
/// fresh State<T> in ___pmt_buffer. ALWAYS a copy, never a borrow.
pub const UNMARSHAL_FN: &str = "unmarshal";

/// Returns true if `name` is one of the FFI marshal builtins.
pub fn is_marshal_builtin(name: &str) -> bool {
    name == MARSHAL_CSTR_FN || name == UNMARSHAL_FN
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_classify_arg_mode_borrow() {
        let attrs = vec![AttrInfo::bare("borrow")];
        assert_eq!(classify_arg_mode(&attrs, false), ArgMode::Borrow);
    }

    #[test]
    fn test_classify_arg_mode_default_invalidate() {
        assert_eq!(classify_arg_mode(&[], false), ArgMode::Invalidate);
    }

    #[test]
    fn test_classify_arg_mode_marshal() {
        let attrs = vec![AttrInfo::bare("marshal")];
        assert_eq!(classify_arg_mode(&attrs, false), ArgMode::Marshal);
    }

    #[test]
    fn test_classify_arg_mode_may_retain_precedence() {
        // #[may_retain] takes precedence over #[borrow].
        let attrs = vec![AttrInfo::bare("borrow"), AttrInfo::bare("may_retain")];
        assert_eq!(classify_arg_mode(&attrs, false), ArgMode::MayRetain);
    }

    #[test]
    fn test_classify_arg_mode_foreign_pass() {
        // No param attr, but layout is #[foreign(raw)].
        assert_eq!(classify_arg_mode(&[], true), ArgMode::ForeignPass);
    }

    #[test]
    fn test_classify_arg_mode_borrow_overrides_foreign() {
        // #[borrow] on the param overrides the layout's #[foreign(raw)].
        let attrs = vec![AttrInfo::bare("borrow")];
        assert_eq!(classify_arg_mode(&attrs, true), ArgMode::Borrow);
    }

    #[test]
    fn test_classify_return_mode_scalar() {
        assert_eq!(classify_return_mode(&[]), ReturnMode::Scalar);
    }

    #[test]
    fn test_classify_return_mode_unmarshal() {
        let attrs = vec![AttrInfo::with_value("unmarshal", "Response")];
        assert_eq!(
            classify_return_mode(&attrs),
            ReturnMode::Unmarshal("Response".to_string())
        );
    }

    #[test]
    fn test_classify_return_mode_foreign_wrap() {
        let attrs = vec![AttrInfo::with_value("foreign_return", "raw")];
        assert_eq!(
            classify_return_mode(&attrs),
            ReturnMode::ForeignWrap("raw".to_string())
        );
    }

    #[test]
    fn test_is_callback_fn() {
        assert!(is_callback_fn(&[AttrInfo::bare("callback")]));
        assert!(!is_callback_fn(&[]));
    }

    #[test]
    fn test_foreign_consume_field() {
        assert_eq!(
            foreign_consume_field(&[AttrInfo::with_value("foreign_consume", "raw")]),
            Some("raw".to_string())
        );
        assert!(foreign_consume_field(&[]).is_none());
    }

    #[test]
    fn test_marshal_arg_borrow() {
        let r = marshal_arg("p", ArgMode::Borrow, 16, 8);
        assert!(r.preserved);
        assert!(r.ptr_expr.contains("___pmt_buffer_base"));
        assert!(r.ptr_expr.contains("16"));
    }

    #[test]
    fn test_marshal_arg_invalidate() {
        let r = marshal_arg("p", ArgMode::Invalidate, 16, 8);
        assert!(!r.preserved);
        assert!(r.ptr_expr.contains("___pmt_buffer_base"));
    }

    #[test]
    fn test_marshal_arg_marshal_uses_scratchpad() {
        let r = marshal_arg("p", ArgMode::Marshal, 16, 8);
        assert!(r.preserved);
        assert!(r.ptr_expr.contains("___ffi_scratch_alloc"));
        assert!(r.ptr_expr.contains("p"));
    }

    #[test]
    fn test_marshal_arg_foreign_pass() {
        let r = marshal_arg("p", ArgMode::ForeignPass, 16, 8);
        assert!(r.preserved);
        assert_eq!(r.ptr_expr, "p.raw");
    }

    #[test]
    fn test_foreign_layout_field() {
        let attrs = vec![AttrInfo::with_value("foreign", "raw")];
        assert_eq!(foreign_layout_field(&attrs), Some("raw".to_string()));
        assert!(foreign_layout_field(&[]).is_none());
    }

    #[test]
    fn test_is_marshal_builtin() {
        assert!(is_marshal_builtin("marshal_cstr"));
        assert!(is_marshal_builtin("unmarshal"));
        assert!(!is_marshal_builtin("write"));
    }

    #[test]
    fn test_legacy_marshal_state_for_ffi() {
        let r = marshal_state_for_ffi("p", 8, true);
        assert!(r.preserved);
        assert_eq!(r.mode, ArgMode::Borrow);
        let r2 = marshal_state_for_ffi("p", 8, false);
        assert!(!r2.preserved);
        assert_eq!(r2.mode, ArgMode::Invalidate);
    }

    #[test]
    fn test_is_pure_extern_legacy() {
        assert!(is_pure_extern(&["pure".to_string()]));
        assert!(!is_pure_extern(&[]));
    }
}
