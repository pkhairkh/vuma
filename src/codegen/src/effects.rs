//! Effect System — Track and Verify Function Effects
//!
//! Each function has an effect set that describes what it does:
//! - `Pure`: No side effects, no I/O, no allocation
//! - `Alloc`: Allocates memory
//! - `IO`: Performs I/O (read, write, etc.)
//! - `Modifies`: Modifies memory through pointers
//! - `Diverges`: May not terminate
//!
//! Effects are inferred from the IR and can be annotated in source.
//! The compiler can optimize pure functions (CSE, memoization, etc.).

use std::collections::{HashMap, HashSet};
use crate::ir::{IRFunction, IRInstr, IRTerminator};

/// Effects that a function may have.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Effect {
    /// Allocates memory (allocate/mmap).
    Alloc,
    /// Frees memory (free/munmap).
    Free,
    /// Performs I/O (read, write, open, close).
    IO,
    /// Modifies memory through pointers (Store).
    Modifies,
    /// Performs atomic operations.
    Atomic,
    /// Calls an extern function (unknown effects).
    ExternCall,
}

/// The full effect set of a function.
#[derive(Debug, Clone, Default)]
pub struct EffectSet {
    pub effects: HashSet<Effect>,
}

impl EffectSet {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_pure(&self) -> bool {
        self.effects.is_empty()
    }

    pub fn add(&mut self, effect: Effect) {
        self.effects.insert(effect);
    }

    pub fn contains(&self, effect: Effect) -> bool {
        self.effects.contains(&effect)
    }

    pub fn union(&mut self, other: &EffectSet) {
        for e in &other.effects {
            self.effects.insert(*e);
        }
    }
}

impl std::fmt::Display for EffectSet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.effects.is_empty() {
            return write!(f, "Pure");
        }
        let mut effects: Vec<&str> = self
            .effects
            .iter()
            .map(|e| match e {
                Effect::Alloc => "Alloc",
                Effect::Free => "Free",
                Effect::IO => "IO",
                Effect::Modifies => "Modifies",
                Effect::Atomic => "Atomic",
                Effect::ExternCall => "ExternCall",
            })
            .collect();
        effects.sort();
        write!(f, "{}", effects.join(" + "))
    }
}

/// Infer the effects of a function from its IR.
pub fn infer_effects(func: &IRFunction) -> EffectSet {
    let mut effects = EffectSet::new();

    for block in &func.blocks {
        for instr in &block.instructions {
            match instr {
                IRInstr::Alloc { .. } => {
                    effects.add(Effect::Alloc);
                }
                IRInstr::Store { .. } => {
                    effects.add(Effect::Modifies);
                }
                IRInstr::AtomicLoad { .. }
                | IRInstr::AtomicStore { .. }
                | IRInstr::AtomicCas { .. } => {
                    effects.add(Effect::Atomic);
                    effects.add(Effect::Modifies);
                }
                IRInstr::Call { func: fname, .. } => {
                    // Check for known extern functions
                    match fname.as_str() {
                        "write" | "read" | "open" | "close" | "exit" => {
                            effects.add(Effect::IO);
                        }
                        "__vuma_alloc" | "allocate" => {
                            effects.add(Effect::Alloc);
                        }
                        "__vuma_free" | "free" => {
                            effects.add(Effect::Free);
                        }
                        _ => {
                            // Unknown function — could have any effect
                            effects.add(Effect::ExternCall);
                        }
                    }
                }
                _ => {}
            }
        }
    }

    effects
}

/// Build a map of all functions' effects.
pub fn analyze_program_effects(funcs: &[IRFunction]) -> HashMap<String, EffectSet> {
    let mut map = HashMap::new();
    for func in funcs {
        let effects = infer_effects(func);
        map.insert(func.name.clone(), effects);
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_function_is_pure() {
        let func = IRFunction::new("test".to_string());
        let effects = infer_effects(&func);
        assert!(effects.is_pure());
    }

    #[test]
    fn test_effect_display() {
        let mut effects = EffectSet::new();
        effects.add(Effect::IO);
        effects.add(Effect::Alloc);
        assert_eq!(format!("{}", effects), "Alloc + IO");
    }
}
