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
        assert!(results[0].error.as_ref().unwrap().contains("not state-typed"));
    }
}
