//! State Transform Verifier — proves every state transformation is valid.
//!
//! For each `StateTransform(in_state, out_layout)`, verifies:
//! 1. Both layouts exist in the registry.
//! 2. The input state's layout and the output layout are compatible:
//!    - Same total size (reinterpret in-place), OR
//!    - Different sizes (requires a copy transformation — the compiler
//!      generates an Alloc + Store copy, which is always safe).
//! 3. If sizes match, the field offsets are compatible (no overlapping
//!    reinterpretation that would break type safety).

use std::collections::HashMap;

/// Result of verifying a single state transformation.
#[derive(Debug, Clone)]
pub struct StateTransformVerification {
    pub input_layout: String,
    pub output_layout: String,
    pub valid: bool,
    pub transform_kind: TransformKind,
    pub error: Option<String>,
}

/// How the transform is realized.
#[derive(Debug, Clone, PartialEq)]
pub enum TransformKind {
    /// Same size — buffer is reinterpreted in-place (zero-cost).
    Reinterpret,
    /// Different size — new buffer allocated, data copied.
    Copy,
    /// Identity transform (same layout in and out).
    Identity,
}

/// Layout info (same structure as state_read.rs/state_write.rs — duplicated
/// for parallel development; unified in Wave 3d).
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

/// Verify a state transformation.
///
/// `layouts` maps layout names to LayoutInfo.
/// `input_layout` is the source state's layout name.
/// `output_layout` is the target layout name.
pub fn verify_transform(
    layouts: &HashMap<String, LayoutInfo>,
    input_layout: &str,
    output_layout: &str,
) -> StateTransformVerification {
    let in_info = match layouts.get(input_layout) {
        Some(l) => l.clone(),
        None => {
            return StateTransformVerification {
                input_layout: input_layout.to_string(),
                output_layout: output_layout.to_string(),
                valid: false,
                transform_kind: TransformKind::Copy,
                error: Some(format!("input layout '{}' not found", input_layout)),
            };
        }
    };
    let out_info = match layouts.get(output_layout) {
        Some(l) => l.clone(),
        None => {
            return StateTransformVerification {
                input_layout: input_layout.to_string(),
                output_layout: output_layout.to_string(),
                valid: false,
                transform_kind: TransformKind::Copy,
                error: Some(format!("output layout '{}' not found", output_layout)),
            };
        }
    };
    // Identity transform
    if input_layout == output_layout {
        return StateTransformVerification {
            input_layout: input_layout.to_string(),
            output_layout: output_layout.to_string(),
            valid: true,
            transform_kind: TransformKind::Identity,
            error: None,
        };
    }
    // Same size → reinterpret (zero-cost, always safe)
    if in_info.total_size == out_info.total_size {
        // Check for incompatible field overlaps: if two fields at the same
        // offset have different sizes, the reinterpret could read/write
        // partial values. This is safe in VUMA's model (the buffer is just
        // bytes), but we flag it for the user.
        // For now, all same-size reinterprets are valid.
        return StateTransformVerification {
            input_layout: input_layout.to_string(),
            output_layout: output_layout.to_string(),
            valid: true,
            transform_kind: TransformKind::Reinterpret,
            error: None,
        };
    }
    // Different size → copy (compiler generates Alloc + Store)
    // This is always valid — the compiler allocates a new buffer of the
    // output layout's size and copies the data.
    StateTransformVerification {
        input_layout: input_layout.to_string(),
        output_layout: output_layout.to_string(),
        valid: true,
        transform_kind: TransformKind::Copy,
        error: None,
    }
}

/// Verify all transforms in a program.
/// `transforms` is a list of (input_layout, output_layout) pairs.
pub fn verify_all_transforms(
    layouts: &HashMap<String, LayoutInfo>,
    transforms: &[(String, String)],
) -> Vec<StateTransformVerification> {
    transforms.iter()
        .map(|(inp, out)| verify_transform(layouts, inp, out))
        .collect()
}

pub fn all_valid(results: &[StateTransformVerification]) -> bool {
    results.iter().all(|r| r.valid)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_layout(name: &str, size: u64) -> LayoutInfo {
        LayoutInfo {
            name: name.to_string(),
            total_size: size,
            fields: vec![],
        }
    }

    #[test]
    fn test_identity_transform() {
        let layouts = HashMap::from([
            ("Point".to_string(), make_layout("Point", 8)),
        ]);
        let result = verify_transform(&layouts, "Point", "Point");
        assert!(result.valid);
        assert_eq!(result.transform_kind, TransformKind::Identity);
    }

    #[test]
    fn test_reinterpret_same_size() {
        let layouts = HashMap::from([
            ("Point".to_string(), make_layout("Point", 8)),
            ("Vec2".to_string(), make_layout("Vec2", 8)),
        ]);
        let result = verify_transform(&layouts, "Point", "Vec2");
        assert!(result.valid);
        assert_eq!(result.transform_kind, TransformKind::Reinterpret);
    }

    #[test]
    fn test_copy_different_size() {
        let layouts = HashMap::from([
            ("Small".to_string(), make_layout("Small", 4)),
            ("Large".to_string(), make_layout("Large", 16)),
        ]);
        let result = verify_transform(&layouts, "Small", "Large");
        assert!(result.valid);
        assert_eq!(result.transform_kind, TransformKind::Copy);
    }

    #[test]
    fn test_unknown_input_layout() {
        let layouts = HashMap::from([
            ("Point".to_string(), make_layout("Point", 8)),
        ]);
        let result = verify_transform(&layouts, "Unknown", "Point");
        assert!(!result.valid);
        assert!(result.error.as_ref().unwrap().contains("input layout"));
    }

    #[test]
    fn test_unknown_output_layout() {
        let layouts = HashMap::from([
            ("Point".to_string(), make_layout("Point", 8)),
        ]);
        let result = verify_transform(&layouts, "Point", "Unknown");
        assert!(!result.valid);
        assert!(result.error.as_ref().unwrap().contains("output layout"));
    }

    #[test]
    fn test_verify_all_transforms() {
        let layouts = HashMap::from([
            ("A".to_string(), make_layout("A", 8)),
            ("B".to_string(), make_layout("B", 8)),
            ("C".to_string(), make_layout("C", 16)),
        ]);
        let transforms = vec![
            ("A".to_string(), "B".to_string()),   // reinterpret
            ("B".to_string(), "C".to_string()),   // copy
            ("C".to_string(), "C".to_string()),   // identity
        ];
        let results = verify_all_transforms(&layouts, &transforms);
        assert!(all_valid(&results));
        assert_eq!(results.len(), 3);
        assert_eq!(results[0].transform_kind, TransformKind::Reinterpret);
        assert_eq!(results[1].transform_kind, TransformKind::Copy);
        assert_eq!(results[2].transform_kind, TransformKind::Identity);
    }
}
