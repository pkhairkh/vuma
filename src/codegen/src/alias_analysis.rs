//! Type-Based Alias Analysis (TBAA)
//!
//! Determines whether two pointers can alias each other based on their
//! types. This is the simplest form of alias analysis and enables:
//! - Load/store reordering
//! - Dead store elimination
//! - CSE across memory operations
//!
//! # Model
//!
//! Each pointer is assigned an "alias class" based on its type:
//! - `u8*` → AliasClass::U8
//! - `u32*` → AliasClass::U32
//! - `u64*` → AliasClass::U64
//! - `Address` (void*) → AliasClass::Any
//! - Stack allocations → unique AliasClass per allocation
//!
//! Two pointers alias if and only if their alias classes overlap.

use std::collections::HashMap;
use crate::ir::{IRFunction, IRInstr, IRValue};

/// Alias class for a pointer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AliasClass {
    /// Unknown / any type — may alias with everything.
    Any,
    /// u8 pointer.
    U8,
    /// u32 pointer.
    U32,
    /// u64 pointer.
    U64,
    /// Unique per-allocation (stack or heap with known bounds).
    Unique(u32),
}

impl AliasClass {
    /// Returns true if two alias classes may overlap.
    pub fn may_alias(&self, other: &AliasClass) -> bool {
        match (self, other) {
            (AliasClass::Any, _) | (_, AliasClass::Any) => true,
            (AliasClass::Unique(a), AliasClass::Unique(b)) => a == b,
            (AliasClass::Unique(_), _) | (_, AliasClass::Unique(_)) => false,
            (a, b) => a == b,
        }
    }
}

/// Alias analysis result for a function.
pub struct AliasAnalysis {
    /// Map from vreg to alias class.
    pub classes: HashMap<u32, AliasClass>,
}

impl AliasAnalysis {
    /// Run alias analysis on a function.
    pub fn analyze(func: &IRFunction) -> Self {
        let mut classes = HashMap::new();

        for block in &func.blocks {
            for instr in &block.instructions {
                match instr {
                    // Alloc creates a unique alias class
                    IRInstr::Alloc { dst, .. } => {
                        if let Some(vreg) = dst.as_register() {
                            classes.insert(vreg, AliasClass::Unique(vreg));
                        }
                    }

                    // BinOp (pointer arithmetic) inherits base's class
                    IRInstr::BinOp { dst, lhs, .. } => {
                        if let (Some(vreg), Some(lhs_vreg)) =
                            (dst.as_register(), lhs.as_register())
                        {
                            if let Some(class) = classes.get(&lhs_vreg) {
                                classes.insert(vreg, *class);
                            }
                        }
                    }

                    // Load: result type depends on load type
                    IRInstr::Load { dst, ty, .. } => {
                        if let Some(vreg) = dst.as_register() {
                            let class = match ty {
                                &crate::ir::IRType::U8 => AliasClass::U8,
                                &crate::ir::IRType::U32 => AliasClass::U32,
                                &crate::ir::IRType::U64 => AliasClass::U64,
                                _ => AliasClass::Any,
                            };
                            classes.insert(vreg, class);
                        }
                    }

                    // Offset inherits base's class
                    IRInstr::Offset { dst, base, .. } => {
                        if let (Some(vreg), Some(base_vreg)) =
                            (dst.as_register(), base.as_register())
                        {
                            if let Some(class) = classes.get(&base_vreg) {
                                classes.insert(vreg, *class);
                            }
                        }
                    }

                    // Phi: join alias classes from incoming
                    IRInstr::Phi { dst, incoming } => {
                        if let Some(vreg) = dst.as_register() {
                            let mut combined = AliasClass::Any;
                            for (val, _) in incoming {
                                if let IRValue::Register(src_vreg) = val {
                                    if let Some(class) = classes.get(src_vreg) {
                                        combined = if combined == AliasClass::Any {
                                            *class
                                        } else if combined.may_alias(class) {
                                            AliasClass::Any
                                        } else {
                                            *class
                                        };
                                    }
                                }
                            }
                            classes.insert(vreg, combined);
                        }
                    }

                    _ => {}
                }
            }
        }

        AliasAnalysis { classes }
    }

    /// Check if two vregs may alias.
    pub fn may_alias(&self, a: u32, b: u32) -> bool {
        let class_a = self.classes.get(&a).unwrap_or(&AliasClass::Any);
        let class_b = self.classes.get(&b).unwrap_or(&AliasClass::Any);
        class_a.may_alias(class_b)
    }

    /// Check if two IR values may alias.
    pub fn values_may_alias(&self, a: &IRValue, b: &IRValue) -> bool {
        match (a, b) {
            (IRValue::Register(va), IRValue::Register(vb)) => self.may_alias(*va, *vb),
            _ => true, // Conservatively assume immediates don't alias
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_unique_does_not_alias() {
        assert!(!AliasClass::Unique(1).may_alias(&AliasClass::Unique(2)));
    }

    #[test]
    fn test_same_type_aliases() {
        assert!(AliasClass::U8.may_alias(&AliasClass::U8));
    }

    #[test]
    fn test_different_types_dont_alias() {
        assert!(!AliasClass::U8.may_alias(&AliasClass::U32));
    }

    #[test]
    fn test_any_aliases_everything() {
        assert!(AliasClass::Any.may_alias(&AliasClass::U8));
        assert!(AliasClass::Any.may_alias(&AliasClass::Unique(1)));
    }
}
