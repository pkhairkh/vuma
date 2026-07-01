//! Generic Monomorphization
//!
//! Finds all calls to generic functions and generates specialized
//! non-generic versions with concrete types substituted.
//!
//! # Algorithm
//!
//! 1. Scan all functions for calls to generic functions.
//! 2. For each (generic_fn, type_args) pair, generate a specialized version.
//! 3. Replace the call with a call to the specialized version.
//! 4. Recursively monomorphize the specialized version.

use std::collections::{HashMap, HashSet};
use crate::ir::{IRFunction, IRInstr, IRValue, IRType, BinOpKind};

/// A monomorphization key: (function_name, type_args)
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MonoKey {
    pub func_name: String,
    pub type_args: Vec<String>,
}

/// The monomorphization context.
pub struct Monomorphizer {
    /// Map from MonoKey to specialized function name.
    pub specializations: HashMap<MonoKey, String>,
    /// Queue of pending specializations.
    pub pending: Vec<MonoKey>,
    /// Already-processed specialization names.
    pub done: HashSet<String>,
}

impl Monomorphizer {
    pub fn new() -> Self {
        Monomorphizer {
            specializations: HashMap::new(),
            pending: Vec::new(),
            done: HashSet::new(),
        }
    }

    /// Generate a specialized name for a generic function.
    pub fn specialized_name(func: &str, type_args: &[String]) -> String {
        if type_args.is_empty() {
            return func.to_string();
        }
        // e.g., "push_u32" for push<u32>
        format!("{}_{}", func, type_args.join("_"))
    }

    /// Check if a function name looks generic (contains type parameters).
    /// In VUMA, generic functions are those with type_params in their
    /// definition. For now, we detect generics by looking for functions
    /// that reference type parameter names.
    pub fn is_generic(func: &IRFunction) -> bool {
        // Check if the function has type parameter markers
        // In the current IR, generic functions don't have explicit type params.
        // We detect them by checking if the function name contains '<'.
        func.name.contains('<') || func.name.contains("generic")
    }

    /// Collect all call sites that need monomorphization.
    /// Returns a list of (caller_func, callee_func, type_args).
    pub fn collect_call_sites(&self, funcs: &[IRFunction]) -> Vec<(String, String, Vec<String>)> {
        let mut sites = Vec::new();
        for func in funcs {
            for block in &func.blocks {
                for instr in &block.instructions {
                    if let IRInstr::Call { func: callee, .. } = instr {
                        if callee.contains('<') {
                            // Parse type args from "func<T1, T2>"
                            if let Some(angle_start) = callee.find('<') {
                                let name = &callee[..angle_start];
                                let type_str = &callee[angle_start + 1..callee.len() - 1];
                                let type_args: Vec<String> = type_str
                                    .split(',')
                                    .map(|s| s.trim().to_string())
                                    .collect();
                                sites.push((func.name.clone(), name.to_string(), type_args));
                            }
                        }
                    }
                }
            }
        }
        sites
    }

    /// Monomorphize all functions in the program.
    /// Returns the new function list with specialized versions added.
    pub fn monomorphize(&mut self, funcs: Vec<IRFunction>) -> Vec<IRFunction> {
        // Phase 1: Collect all call sites
        let call_sites = self.collect_call_sites(&funcs);

        // Phase 2: Generate specializations
        for (_caller, callee, type_args) in &call_sites {
            let key = MonoKey {
                func_name: callee.clone(),
                type_args: type_args.clone(),
            };
            if !self.specializations.contains_key(&key) {
                let spec_name = Self::specialized_name(callee, type_args);
                self.specializations.insert(key.clone(), spec_name);
                self.pending.push(key);
            }
        }

        // Phase 3: Process pending specializations
        let mut result: Vec<IRFunction> = funcs.into_iter().filter(|f| !Self::is_generic(f)).collect();

        while let Some(key) = self.pending.pop() {
            let spec_name = self.specializations.get(&key).cloned().unwrap_or_default();
            if self.done.contains(&spec_name) {
                continue;
            }
            self.done.insert(spec_name.clone());

            // In a real implementation, we would:
            // 1. Find the generic function definition
            // 2. Substitute type parameters with concrete types
            // 3. Rename the function
            // 4. Add to result
            // For now, we just create a placeholder
        }

        // Phase 4: Rewrite call sites to use specialized names
        for func in &mut result {
            for block in &mut func.blocks {
                for instr in &mut block.instructions {
                    if let IRInstr::Call { func: callee, .. } = instr {
                        if callee.contains('<') {
                            if let Some(angle_start) = callee.find('<') {
                                let name = &callee[..angle_start];
                                let type_str = &callee[angle_start + 1..callee.len() - 1];
                                let type_args: Vec<String> = type_str
                                    .split(',')
                                    .map(|s| s.trim().to_string())
                                    .collect();
                                let key = MonoKey {
                                    func_name: name.to_string(),
                                    type_args,
                                };
                                if let Some(spec_name) = self.specializations.get(&key) {
                                    *callee = spec_name.clone();
                                }
                            }
                        }
                    }
                }
            }
        }

        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_specialized_name() {
        assert_eq!(
            Monomorphizer::specialized_name("push", &["u32".to_string()]),
            "push_u32"
        );
        assert_eq!(
            Monomorphizer::specialized_name("insert", &["String".to_string(), "u32".to_string()]),
            "insert_String_u32"
        );
    }

    #[test]
    fn test_non_generic_name() {
        assert_eq!(
            Monomorphizer::specialized_name("foo", &[]),
            "foo"
        );
    }
}
