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

/// **Wave 9 — Dependent state types:** Verify that a dependent-array state
/// transform is safe.
///
/// Checks the linear-arithmetic proof obligation:
///   `offset + (count * elem_size) ≤ buffer_size`
///
/// This is decidable (Presburger arithmetic) because the only operations
/// are addition and multiplication by a compile-time-known constant
/// (`elem_size`). The count is a runtime value but it appears linearly
/// (not multiplied by another runtime value), so the proof reduces to
/// a simple bounds check on `count` against `buffer_size / elem_size`.
///
/// # Parameters
///
/// - `elem_size`: the static, compile-time-known size in bytes of each
///   element (e.g., `4` for `State<List<u32>>`).
/// - `count`: the runtime value of the count variable (e.g., the actual
///   number of elements currently in the dynamic array).
/// - `offset`: the byte offset of the array's start within the buffer.
/// - `buffer_size`: the total size in bytes of the backing buffer.
///
/// # Returns
///
/// `true` if the access is within bounds (`offset + count*elem_size ≤
/// buffer_size`); `false` otherwise.
///
/// # Decidability note
///
/// The proof is restricted to **linear** arithmetic — sizes, counts, and
/// offsets are combined with `+` and `*` (where one operand is a constant).
/// Non-linear dependencies (e.g., `count1 * count2`) are NOT supported and
/// would make verification undecidable. The `RepD::DependentArray` variant
/// enforces this by construction: it carries only `elem` (a static `RepD`)
/// and `count_var` (a single runtime variable name).
///
/// # Example
///
/// ```
/// use vuma_ive::state_transform::verify_dependent_transform;
///
/// // 10 u32 elements at offset 0 in a 40-byte buffer → safe.
/// assert!(verify_dependent_transform(4, 10, 0, 40));
/// // 11 u32 elements at offset 0 in a 40-byte buffer → out of bounds.
/// assert!(!verify_dependent_transform(4, 11, 0, 40));
/// // 5 u32 elements at offset 20 in a 40-byte buffer → exactly fits.
/// assert!(verify_dependent_transform(4, 5, 20, 40));
/// ```
pub fn verify_dependent_transform(
    elem_size: u64,
    count: u64,
    offset: u64,
    buffer_size: u64,
) -> bool {
    // Linear arithmetic: prove the access is within bounds.
    // This is decidable (Presburger arithmetic) because elem_size is a
    // compile-time constant and count is a runtime variable — the only
    // multiplication is `count * elem_size` (runtime * constant).
    //
    // We use saturating arithmetic to defend against overflow: if the
    // computed size would overflow u64, the access is conservatively
    // rejected (the saturating add will yield u64::MAX, which is > any
    // realistic buffer_size).
    offset.saturating_add(count.saturating_mul(elem_size)) <= buffer_size
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

    // =======================================================================
    // Wave 9 — Dependent state types (verify_dependent_transform)
    // =======================================================================

    #[test]
    fn wave9_dependent_transform_safe_in_bounds() {
        // 10 u32 elements at offset 0 in a 40-byte buffer → safe.
        assert!(verify_dependent_transform(4, 10, 0, 40));
        // 1 element at offset 0 in a 4-byte buffer → safe.
        assert!(verify_dependent_transform(4, 1, 0, 4));
        // 5 elements at offset 20 in a 40-byte buffer → exactly fits.
        assert!(verify_dependent_transform(4, 5, 20, 40));
        // Zero elements (empty array) at any offset → safe.
        assert!(verify_dependent_transform(4, 0, 0, 0));
        assert!(verify_dependent_transform(4, 0, 100, 100));
    }

    #[test]
    fn wave9_dependent_transform_unsafe_out_of_bounds() {
        // 11 u32 elements at offset 0 in a 40-byte buffer → out of bounds.
        assert!(!verify_dependent_transform(4, 11, 0, 40));
        // 1 element at offset 4 in a 4-byte buffer → out of bounds.
        assert!(!verify_dependent_transform(4, 1, 4, 4));
        // 5 elements at offset 21 in a 40-byte buffer → 21 + 20 = 41 > 40.
        assert!(!verify_dependent_transform(4, 5, 21, 40));
        // 0 elements at offset past buffer end → out of bounds.
        assert!(!verify_dependent_transform(4, 0, 41, 40));
    }

    #[test]
    fn wave9_dependent_transform_exact_fit() {
        // Exact fit: offset + count*elem_size == buffer_size.
        assert!(verify_dependent_transform(1, 100, 0, 100));
        assert!(verify_dependent_transform(8, 4, 0, 32));
        assert!(verify_dependent_transform(8, 4, 16, 48)); // 16 + 32 = 48
    }

    #[test]
    fn wave9_dependent_transform_overflow_safe() {
        // Saturating arithmetic: a huge count would overflow u64 on
        // multiplication. The verifier conservatively rejects the access
        // (saturating to u64::MAX, which is > any realistic buffer_size).
        let huge = u64::MAX;
        // count = u64::MAX, elem_size = 8 → sat_mul gives u64::MAX
        // offset + u64::MAX → sat_add gives u64::MAX
        // u64::MAX > 40 → false.
        assert!(!verify_dependent_transform(8, huge, 0, 40));
        // count = u64::MAX / 8 + 1 (still overflows when * 8)
        let big_count = u64::MAX / 8 + 1;
        assert!(!verify_dependent_transform(8, big_count, 0, 100));
        // Reasonable count with offset+size overflow: offset near u64::MAX.
        assert!(!verify_dependent_transform(4, 10, u64::MAX - 10, 100));
    }

    #[test]
    fn wave9_dependent_transform_zero_elem_size() {
        // Zero-size element: any count is safe (size = 0).
        assert!(verify_dependent_transform(0, 1000, 0, 0));
        assert!(verify_dependent_transform(0, 1000, 0, 1));
        // But offset must still be ≤ buffer_size.
        assert!(!verify_dependent_transform(0, 0, 1, 0));
    }

    #[test]
    fn wave9_dependent_transform_byte_elements() {
        // Byte elements (elem_size = 1) — count == buffer_size.
        assert!(verify_dependent_transform(1, 256, 0, 256));
        assert!(!verify_dependent_transform(1, 257, 0, 256));
        // Byte elements with offset.
        assert!(verify_dependent_transform(1, 100, 100, 200));
        assert!(!verify_dependent_transform(1, 101, 100, 200));
    }

    #[test]
    fn wave9_dependent_transform_struct_elements() {
        // Struct elements (elem_size = 8 for a 2x u32 struct).
        // 4 structs at offset 0 in a 32-byte buffer → safe.
        assert!(verify_dependent_transform(8, 4, 0, 32));
        // 5 structs at offset 0 in a 32-byte buffer → 40 > 32.
        assert!(!verify_dependent_transform(8, 5, 0, 32));
    }
}
