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
    ///
    /// The model is conservative with respect to `Unique` allocations vs
    /// typed pointers: a typed pointer (`U8`/`U32`/`U64`) may have been
    /// derived from a `Unique` allocation via `Cast` + pointer arithmetic
    /// (e.g. the bootstrap's `Address` buffers are cast to `u32*`/`u64*`
    /// before load/store). We cannot prove they don't alias, so we
    /// conservatively report `true` for `(Unique, typed)` pairs.
    pub fn may_alias(&self, other: &AliasClass) -> bool {
        match (self, other) {
            (AliasClass::Any, _) | (_, AliasClass::Any) => true,
            (AliasClass::Unique(a), AliasClass::Unique(b)) => a == b,
            // A typed pointer might have been derived from a Unique
            // allocation via Cast + pointer arithmetic. We can't prove
            // they don't alias.
            (AliasClass::Unique(_), _) | (_, AliasClass::Unique(_)) => true,
            // Two typed pointers of the same type might alias.
            (a, b) if a == b => true,
            // Different typed pointers (U8 vs U32) don't alias.
            _ => false,
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
                            let class = match *ty {
                                crate::ir::IRType::U8 => AliasClass::U8,
                                crate::ir::IRType::U32 => AliasClass::U32,
                                crate::ir::IRType::U64 => AliasClass::U64,
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

                    // Cast inherits source's class. This is the key fix
                    // for the O2 bootstrap SIGSEGV: when the bootstrap
                    // casts an `Address` (void*) derived from a `Unique`
                    // allocation to a typed pointer (`u32*`/`u64*`), the
                    // Cast result keeps the source's `Unique` class so the
                    // scheduler doesn't incorrectly conclude that a Load
                    // through the typed pointer can't alias a Store to the
                    // original allocation.
                    IRInstr::Cast { dst, src, .. } => {
                        if let (Some(vreg), Some(src_vreg)) =
                            (dst.as_register(), src.as_register())
                        {
                            if let Some(class) = classes.get(&src_vreg) {
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

    /// Regression for the O2 bootstrap SIGSEGV: a typed pointer derived
    /// from a `Unique` allocation via `Cast` + pointer arithmetic MUST
    /// alias the original allocation. The old `may_alias` returned
    /// `false` here, which let the scheduler reorder a Load through the
    /// typed pointer past a Store to the same allocation.
    #[test]
    fn test_unique_aliases_typed_pointer() {
        assert!(
            AliasClass::Unique(1).may_alias(&AliasClass::U8),
            "Unique allocation may alias a u8 pointer derived via Cast"
        );
        assert!(
            AliasClass::Unique(1).may_alias(&AliasClass::U32),
            "Unique allocation may alias a u32 pointer derived via Cast"
        );
        assert!(
            AliasClass::Unique(1).may_alias(&AliasClass::U64),
            "Unique allocation may alias a u64 pointer derived via Cast"
        );
        // Symmetric direction.
        assert!(AliasClass::U32.may_alias(&AliasClass::Unique(1)));
        assert!(AliasClass::U64.may_alias(&AliasClass::Unique(1)));
    }

    /// Cast must propagate the source's alias class. A Cast from a
    /// `Unique`-classified pointer keeps the `Unique` class so that
    /// `may_alias` between the Cast result and the original allocation
    /// returns `true`.
    #[test]
    fn test_cast_inherits_source_class() {
        use crate::ir::{CastKind, IRBlock, IRFunction, IRInstr, IRType, IRValue};

        // Build a tiny function:
        //   v1 = alloc 16                  → Unique(1)
        //   v2 = cast bitcast v1: u8* → u32*  → should inherit Unique(1)
        let mut func = IRFunction::new("__test_cast__");
        let mut block = IRBlock::new("entry");
        block.instructions.push(IRInstr::Alloc {
            dst: IRValue::Register(1),
            size: 16,
        });
        block.instructions.push(IRInstr::Cast {
            kind: CastKind::BitCast,
            dst: IRValue::Register(2),
            src: IRValue::Register(1),
            from_ty: Some(IRType::U8),
            to_ty: Some(IRType::U32),
        });
        func.blocks = vec![block];

        let aa = AliasAnalysis::analyze(&func);
        assert_eq!(
            aa.classes.get(&1),
            Some(&AliasClass::Unique(1)),
            "Alloc must assign Unique(1) to v1"
        );
        assert_eq!(
            aa.classes.get(&2),
            Some(&AliasClass::Unique(1)),
            "Cast must propagate the source's Unique(1) class to v2"
        );
        // The Cast result aliases the original allocation.
        assert!(
            aa.may_alias(1, 2),
            "Cast result must alias the original allocation"
        );
    }
}
