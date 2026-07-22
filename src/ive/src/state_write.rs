//! State Write Verifier — proves every state-field write is valid and
//! enforces linear ownership semantics for state values.
//!
//! For each `StateWrite` (field assignment on a State-typed variable), verifies:
//! 1. The field exists in the state's layout.
//! 2. field_offset + field_size ≤ layout.total_size (no out-of-bounds write).
//! 3. The write type matches the field's declared type.
//! 4. Linearity: a state is not written to after it has been consumed by a
//!    transform (states are linear — one owner, consumed on transform).

use std::collections::{HashMap, HashSet};

/// Result of verifying a single state-field write.
#[derive(Debug, Clone)]
pub struct StateWriteVerification {
    pub var_name: String,
    pub layout_name: String,
    pub field_name: String,
    pub valid: bool,
    pub error: Option<String>,
}

/// Layout info (same structure as state_read.rs — duplicated to keep modules
/// independent for parallel development; will be unified in Wave 3d).
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

/// Represents a state-field write operation in the program.
#[derive(Debug, Clone)]
pub struct StateWriteOp {
    pub var_name: String,
    pub field_name: String,
    pub value_type: String,
    /// Whether this write happens after the state was consumed by a transform.
    pub after_consume: bool,
}

/// Verify all state-field writes in a function.
///
/// `state_var_layouts` maps variable names to their layout names.
/// `layouts` maps layout names to LayoutInfo.
/// `writes` is a list of StateWriteOp representing each state-field write.
/// `consumed_vars` is the set of variables that have been consumed by transforms.
pub fn verify_state_writes(
    state_var_layouts: &HashMap<String, String>,
    layouts: &HashMap<String, LayoutInfo>,
    writes: &[StateWriteOp],
    consumed_vars: &HashSet<String>,
) -> Vec<StateWriteVerification> {
    let mut results = Vec::new();
    for w in writes {
        // Linearity check: state must not be written after consumption
        if w.after_consume || consumed_vars.contains(&w.var_name) {
            results.push(StateWriteVerification {
                var_name: w.var_name.clone(),
                layout_name: String::new(),
                field_name: w.field_name.clone(),
                valid: false,
                error: Some(format!(
                    "linearity violation: state '{}' written after being consumed by a transform",
                    w.var_name
                )),
            });
            continue;
        }
        let layout_name = match state_var_layouts.get(&w.var_name) {
            Some(l) => l.clone(),
            None => {
                results.push(StateWriteVerification {
                    var_name: w.var_name.clone(),
                    layout_name: String::new(),
                    field_name: w.field_name.clone(),
                    valid: false,
                    error: Some(format!("variable '{}' is not state-typed", w.var_name)),
                });
                continue;
            }
        };
        let layout = match layouts.get(&layout_name) {
            Some(l) => l.clone(),
            None => {
                results.push(StateWriteVerification {
                    var_name: w.var_name.clone(),
                    layout_name: layout_name.clone(),
                    field_name: w.field_name.clone(),
                    valid: false,
                    error: Some(format!("layout '{}' not found", layout_name)),
                });
                continue;
            }
        };
        let field_info = layout.fields.iter().find(|f| f.name == w.field_name);
        match field_info {
            Some(fi) => {
                if fi.offset + fi.size > layout.total_size {
                    results.push(StateWriteVerification {
                        var_name: w.var_name.clone(),
                        layout_name: layout_name.clone(),
                        field_name: w.field_name.clone(),
                        valid: false,
                        error: Some(format!(
                            "field '{}' at offset {} size {} exceeds layout total_size {}",
                            w.field_name, fi.offset, fi.size, layout.total_size
                        )),
                    });
                } else if fi.type_name != w.value_type {
                    results.push(StateWriteVerification {
                        var_name: w.var_name.clone(),
                        layout_name: layout_name.clone(),
                        field_name: w.field_name.clone(),
                        valid: false,
                        error: Some(format!(
                            "type mismatch: field '{}' is '{}' but written as '{}'",
                            w.field_name, fi.type_name, w.value_type
                        )),
                    });
                } else {
                    results.push(StateWriteVerification {
                        var_name: w.var_name.clone(),
                        layout_name: layout_name.clone(),
                        field_name: w.field_name.clone(),
                        valid: true,
                        error: None,
                    });
                }
            }
            None => {
                results.push(StateWriteVerification {
                    var_name: w.var_name.clone(),
                    layout_name: layout_name.clone(),
                    field_name: w.field_name.clone(),
                    valid: false,
                    error: Some(format!(
                        "field '{}' not found in layout '{}'", w.field_name, layout_name
                    )),
                });
            }
        }
    }
    results
}

pub fn all_valid(results: &[StateWriteVerification]) -> bool {
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
            fields: fields.iter().map(|(fn_, sz, _off, ty)| {
                let f = FieldInfo {
                    name: fn_.to_string(),
                    offset,
                    size: *sz,
                    type_name: ty.to_string(),
                };
                offset += sz;
                f
            }).collect(),
        }
    }

    #[test]
    fn test_valid_write() {
        let layouts = HashMap::from([
            ("Point".to_string(), make_layout("Point", &[("x", 4, 0, "u32"), ("y", 4, 0, "u32")])),
        ]);
        let var_layouts = HashMap::from([("p".to_string(), "Point".to_string())]);
        let writes = vec![StateWriteOp {
            var_name: "p".to_string(),
            field_name: "x".to_string(),
            value_type: "u32".to_string(),
            after_consume: false,
        }];
        let consumed = HashSet::new();
        let results = verify_state_writes(&var_layouts, &layouts, &writes, &consumed);
        assert!(all_valid(&results));
    }

    #[test]
    fn test_linearity_violation() {
        let layouts = HashMap::from([
            ("Point".to_string(), make_layout("Point", &[("x", 4, 0, "u32")])),
        ]);
        let var_layouts = HashMap::from([("p".to_string(), "Point".to_string())]);
        let writes = vec![StateWriteOp {
            var_name: "p".to_string(),
            field_name: "x".to_string(),
            value_type: "u32".to_string(),
            after_consume: true,
        }];
        let consumed = HashSet::new();
        let results = verify_state_writes(&var_layouts, &layouts, &writes, &consumed);
        assert!(!all_valid(&results));
        assert!(results[0].error.as_ref().unwrap().contains("linearity"));
    }

    #[test]
    fn test_write_after_consume_in_set() {
        let layouts = HashMap::from([
            ("Point".to_string(), make_layout("Point", &[("x", 4, 0, "u32")])),
        ]);
        let var_layouts = HashMap::from([("p".to_string(), "Point".to_string())]);
        let writes = vec![StateWriteOp {
            var_name: "p".to_string(),
            field_name: "x".to_string(),
            value_type: "u32".to_string(),
            after_consume: false,
        }];
        let consumed = HashSet::from(["p".to_string()]);
        let results = verify_state_writes(&var_layouts, &layouts, &writes, &consumed);
        assert!(!all_valid(&results));
        assert!(results[0].error.as_ref().unwrap().contains("consumed"));
    }

    #[test]
    fn test_type_mismatch() {
        let layouts = HashMap::from([
            ("Point".to_string(), make_layout("Point", &[("x", 4, 0, "u32")])),
        ]);
        let var_layouts = HashMap::from([("p".to_string(), "Point".to_string())]);
        let writes = vec![StateWriteOp {
            var_name: "p".to_string(),
            field_name: "x".to_string(),
            value_type: "u64".to_string(),
            after_consume: false,
        }];
        let consumed = HashSet::new();
        let results = verify_state_writes(&var_layouts, &layouts, &writes, &consumed);
        assert!(!all_valid(&results));
        assert!(results[0].error.as_ref().unwrap().contains("type mismatch"));
    }
}
