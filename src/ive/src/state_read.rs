//! State Read Verifier — proves every state-field read is valid.
//!
//! For each `StateRead` (field access on a State-typed variable), verifies:
//! 1. The field exists in the state's layout.
//! 2. field_offset + field_size ≤ layout.total_size (no out-of-bounds read).
//! 3. The read type matches the field's declared type.

use std::collections::HashMap;

/// Result of verifying a single state-field read.
#[derive(Debug, Clone)]
pub struct StateReadVerification {
    pub var_name: String,
    pub layout_name: String,
    pub field_name: String,
    pub valid: bool,
    pub error: Option<String>,
}

/// Layout info needed for verification (a simplified view of LayoutRegistry).
#[derive(Debug, Clone)]
pub struct LayoutInfo {
    pub name: String,
    pub total_size: u64,
    pub fields: Vec<FieldInfo>,
}

#[derive(Debug, Clone)]
pub struct FieldInfo {
    pub name: String,
    pub offset: u64,
    pub size: u64,
    pub type_name: String,
}

/// Verify all state-field reads in a function.
///
/// `state_var_layouts` maps variable names to their layout names.
/// `layouts` maps layout names to LayoutInfo.
/// `reads` is a list of (var_name, field_name, expected_type) tuples
/// representing each state-field read in the function.
///
/// Returns a vector of verification results (one per read).
pub fn verify_state_reads(
    state_var_layouts: &HashMap<String, String>,
    layouts: &HashMap<String, LayoutInfo>,
    reads: &[(String, String, String)], // (var, field, expected_type)
) -> Vec<StateReadVerification> {
    let mut results = Vec::new();
    for (var, field, expected_type) in reads {
        let layout_name = match state_var_layouts.get(var) {
            Some(l) => l.clone(),
            None => {
                results.push(StateReadVerification {
                    var_name: var.clone(),
                    layout_name: String::new(),
                    field_name: field.clone(),
                    valid: false,
                    error: Some(format!("variable '{}' is not state-typed", var)),
                });
                continue;
            }
        };
        let layout = match layouts.get(&layout_name) {
            Some(l) => l.clone(),
            None => {
                results.push(StateReadVerification {
                    var_name: var.clone(),
                    layout_name: layout_name.clone(),
                    field_name: field.clone(),
                    valid: false,
                    error: Some(format!("layout '{}' not found", layout_name)),
                });
                continue;
            }
        };
        // Find the field
        let field_info = layout.fields.iter().find(|f| f.name == *field);
        match field_info {
            Some(fi) => {
                // Check offset + size ≤ total_size
                if fi.offset + fi.size > layout.total_size {
                    results.push(StateReadVerification {
                        var_name: var.clone(),
                        layout_name: layout_name.clone(),
                        field_name: field.clone(),
                        valid: false,
                        error: Some(format!(
                            "field '{}' at offset {} size {} exceeds layout '{}' total_size {}",
                            field, fi.offset, fi.size, layout_name, layout.total_size
                        )),
                    });
                } else if fi.type_name != *expected_type {
                    results.push(StateReadVerification {
                        var_name: var.clone(),
                        layout_name: layout_name.clone(),
                        field_name: field.clone(),
                        valid: false,
                        error: Some(format!(
                            "type mismatch: field '{}' is '{}' but read as '{}'",
                            field, fi.type_name, expected_type
                        )),
                    });
                } else {
                    results.push(StateReadVerification {
                        var_name: var.clone(),
                        layout_name: layout_name.clone(),
                        field_name: field.clone(),
                        valid: true,
                        error: None,
                    });
                }
            }
            None => {
                results.push(StateReadVerification {
                    var_name: var.clone(),
                    layout_name: layout_name.clone(),
                    field_name: field.clone(),
                    valid: false,
                    error: Some(format!(
                        "field '{}' not found in layout '{}'",
                        field, layout_name
                    )),
                });
            }
        }
    }
    results
}

/// Returns true if ALL reads are valid.
pub fn all_valid(results: &[StateReadVerification]) -> bool {
    results.iter().all(|r| r.valid)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_layout(name: &str, fields: &[(&str, u64, u64, &str)]) -> LayoutInfo {
        let mut offset = 0u64;
        LayoutInfo {
            name: name.to_string(),
            total_size: fields.iter().map(|(_, sz, _, _)| *sz).sum(),
            fields: fields
                .iter()
                .map(|(fn_, sz, _off, ty)| {
                    let f = FieldInfo {
                        name: fn_.to_string(),
                        offset,
                        size: *sz,
                        type_name: ty.to_string(),
                    };
                    offset += sz;
                    f
                })
                .collect(),
        }
    }

    #[test]
    fn test_valid_read() {
        let layouts = HashMap::from([(
            "Point".to_string(),
            make_layout("Point", &[("x", 4, 0, "u32"), ("y", 4, 0, "u32")]),
        )]);
        let var_layouts = HashMap::from([("p".to_string(), "Point".to_string())]);
        let reads = vec![("p".to_string(), "x".to_string(), "u32".to_string())];
        let results = verify_state_reads(&var_layouts, &layouts, &reads);
        assert!(all_valid(&results));
    }

    #[test]
    fn test_unknown_field() {
        let layouts = HashMap::from([(
            "Point".to_string(),
            make_layout("Point", &[("x", 4, 0, "u32")]),
        )]);
        let var_layouts = HashMap::from([("p".to_string(), "Point".to_string())]);
        let reads = vec![("p".to_string(), "z".to_string(), "u32".to_string())];
        let results = verify_state_reads(&var_layouts, &layouts, &reads);
        assert!(!all_valid(&results));
        assert!(results[0].error.as_ref().unwrap().contains("not found"));
    }

    #[test]
    fn test_type_mismatch() {
        let layouts = HashMap::from([(
            "Point".to_string(),
            make_layout("Point", &[("x", 4, 0, "u32")]),
        )]);
        let var_layouts = HashMap::from([("p".to_string(), "Point".to_string())]);
        let reads = vec![("p".to_string(), "x".to_string(), "u64".to_string())];
        let results = verify_state_reads(&var_layouts, &layouts, &reads);
        assert!(!all_valid(&results));
        assert!(results[0].error.as_ref().unwrap().contains("type mismatch"));
    }

    #[test]
    fn test_non_state_var() {
        let layouts = HashMap::new();
        let var_layouts = HashMap::new();
        let reads = vec![("x".to_string(), "field".to_string(), "u32".to_string())];
        let results = verify_state_reads(&var_layouts, &layouts, &reads);
        assert!(!all_valid(&results));
        assert!(results[0]
            .error
            .as_ref()
            .unwrap()
            .contains("not state-typed"));
    }

    // ── [Task 9-d / Caveats §6 row 3] Negative-path tests ──────────────
    //
    // These tests cover the PMT-state-read negative paths documented in
    // `tests/gold_standard/pmt_wave3_negative/` (bad_offset.vuma,
    // bad_type.vuma).  The verifier returns `Vec<StateReadVerification>`
    // with `valid=false` and a specific error message rather than
    // panicking, so per the task brief these tests use
    // `assert!(!all_valid(...))` plus error-message substring checks
    // instead of `#[should_panic]`.  The substring checks make the
    // tests robust against unrelated message reformatting.

    /// [PMT violation: unknown field] Reading `p.z` where `Point` has
    /// only `{x, y}` must yield an invalid result whose error message
    /// names BOTH the missing field AND the layout.  A regression that
    /// silently accepted unknown fields (returning `valid=true`) would
    /// be caught by the `!all_valid` assertion; a regression that
    /// returned the wrong field/layout name in the message would be
    /// caught by the substring checks.
    #[test]
    fn test_negative_unknown_field_error_message_is_specific() {
        let layouts = HashMap::from([(
            "Point".to_string(),
            make_layout("Point", &[("x", 4, 0, "u32"), ("y", 4, 0, "u32")]),
        )]);
        let var_layouts = HashMap::from([("p".to_string(), "Point".to_string())]);
        // Read `p.z` — 'z' is not a field of Point.
        let reads = vec![("p".to_string(), "z".to_string(), "u32".to_string())];
        let results = verify_state_reads(&var_layouts, &layouts, &reads);
        assert!(
            !all_valid(&results),
            "reading an unknown field must yield an invalid verification result"
        );
        let err = results[0]
            .error
            .as_ref()
            .expect("error message must be set on invalid result");
        assert!(
            err.contains("field 'z' not found in layout 'Point'"),
            "error message must name both the missing field ('z') and the \
             layout ('Point'); got: {}",
            err
        );
    }

    /// [PMT violation: type mismatch] Reading a u32-typed field as u64
    /// must yield an invalid result whose error message names BOTH the
    /// declared type ('u32') AND the expected type ('u64').  This
    /// covers the negative path documented in
    /// `pmt_wave3_negative/bad_type.vuma`.
    #[test]
    fn test_negative_type_mismatch_error_message_is_specific() {
        let layouts = HashMap::from([(
            "Point".to_string(),
            make_layout("Point", &[("x", 4, 0, "u32")]),
        )]);
        let var_layouts = HashMap::from([("p".to_string(), "Point".to_string())]);
        // Read `p.x` as u64 — but 'x' is declared u32.
        let reads = vec![("p".to_string(), "x".to_string(), "u64".to_string())];
        let results = verify_state_reads(&var_layouts, &layouts, &reads);
        assert!(
            !all_valid(&results),
            "type-mismatched read must yield an invalid verification result"
        );
        let err = results[0]
            .error
            .as_ref()
            .expect("error message must be set on invalid result");
        assert!(
            err.contains("type mismatch"),
            "error message must mention 'type mismatch'; got: {}",
            err
        );
        assert!(
            err.contains("'u32'") && err.contains("'u64'"),
            "error message must name both the declared ('u32') and expected \
             ('u64') types; got: {}",
            err
        );
    }
}
