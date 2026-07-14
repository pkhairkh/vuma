//! Generic Monomorphization
//!
//! Finds all calls to generic functions and generates specialized
//! non-generic versions with concrete types substituted.
//!
//! # Algorithm
//!
//! 1. Scan all functions for `IRInstr::Call` whose callee name carries a
//!    `<...>` type-argument suffix (e.g. `id<i32>`).
//! 2. For each unique `(generic_name, type_args)` pair, clone the generic
//!    function definition once and rename the clone using
//!    [`Monomorphizer::specialized_name`] (e.g. `id_i32`).
//! 3. Rewrite the original `Call` site to invoke the specialized clone
//!    directly (no `<...>` suffix).
//! 4. Drop the original generic definitions (they have no concrete callers
//!    after step 3).
//!
//! # Pipeline integration
//!
//! The free function [`monomorphize`] is the pipeline entry point — it
//! drives a [`Monomorphizer`] over every function in an [`IRProgram`]. The
//! orchestrator wires it from `pipeline.rs` immediately after SCG→IR
//! lowering, before codegen-side optimization. (Wave 34 wires the entry
//! point; pipeline.rs integration is deferred to the orchestrator's final
//! pass per the batch-3 strategy change.)

use std::collections::{HashMap, HashSet};

use crate::backend::BackendError;
use crate::ir::{IRFunction, IRInstr, IRProgram, IRValue};

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
    ///
    /// **Legacy heuristic.** In VUMA's IR, generic function *definitions*
    /// are stored under their bare name (e.g. `id`), not `id<T>`; the
    /// `<...>` suffix only appears at *call sites*. This method therefore
    /// only returns `true` for synthetic / debug names that explicitly
    /// embed the angle-bracket syntax. The pipeline entry point
    /// [`monomorphize`] uses the more accurate "is this function referenced
    /// by any `name<...>` call site" test (see [`Self::collect_call_sites`])
    /// instead of this heuristic.
    pub fn is_generic(func: &IRFunction) -> bool {
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
                        if let Some((name, type_args)) = parse_generic_callee(callee) {
                            sites.push((func.name.clone(), name, type_args));
                        }
                    }
                }
            }
        }
        sites
    }

    /// Monomorphize all functions in the program.
    ///
    /// Replaces the input function list with a new list containing:
    /// - All non-generic functions (with their call sites rewritten to use
    ///   specialized names).
    /// - One cloned specialization per unique `(generic_name, type_args)`
    ///   instantiation, named via [`Self::specialized_name`].
    ///
    /// Generic function *definitions* (those referenced by at least one
    /// `name<...>` call site) are dropped — their specialized clones
    /// supersede them.
    pub fn monomorphize(&mut self, funcs: Vec<IRFunction>) -> Vec<IRFunction> {
        // Phase 1: Collect all call sites.
        let call_sites = self.collect_call_sites(&funcs);

        // Phase 2: Register every unique (generic_name, type_args) pair.
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

        // Set of generic-definition names (the bare names referenced via
        // `name<...>` call sites). These are dropped from the result.
        let generic_def_names: HashSet<String> =
            call_sites.iter().map(|(_, name, _)| name.clone()).collect();

        // Build a lookup of generic function definitions by name (so we can
        // clone them once per instantiation).
        let generic_lookup: HashMap<String, &IRFunction> = funcs
            .iter()
            .filter(|f| generic_def_names.contains(&f.name))
            .map(|f| (f.name.clone(), f))
            .collect();

        // Phase 3: Process pending specializations — clone the generic
        // function definition once per instantiation and rename the clone.
        //
        // VUMA's IR is already concretely typed at this stage (the SCG→IR
        // lowering instantiates type parameters when it emits the call
        // site). The type_args carried by the call site therefore only
        // influence the *specialization symbol name*; no further type
        // substitution is required inside the cloned body.
        let mut specialized_funcs: Vec<IRFunction> = Vec::new();
        while let Some(key) = self.pending.pop() {
            let spec_name = self.specializations.get(&key).cloned().unwrap_or_default();
            if spec_name.is_empty() || self.done.contains(&spec_name) {
                continue;
            }
            self.done.insert(spec_name.clone());

            match generic_lookup.get(&key.func_name) {
                Some(generic_func) => {
                    let mut spec_func = (*generic_func).clone();
                    spec_func.name = spec_name;
                    specialized_funcs.push(spec_func);
                }
                None => {
                    // No definition found — the call site references an
                    // extern / runtime generic. Leave the call as-is (it
                    // will be resolved at link time).
                    vuma_log!(warn, 
                        "Monomorphizer: no definition for generic `{}` \
                         (referenced by call site `{}`); leaving extern",
                        key.func_name,
                        spec_name
                    );
                }
            }
        }

        // Phase 4: Build the result — non-generic functions + new
        // specializations. Drop generic definitions that have been
        // specialized.
        let mut result: Vec<IRFunction> = funcs
            .into_iter()
            .filter(|f| !generic_def_names.contains(&f.name))
            .collect();
        result.extend(specialized_funcs);

        // Phase 5: Rewrite every `name<...>` call site to its specialized
        // name.
        for func in &mut result {
            for block in &mut func.blocks {
                for instr in &mut block.instructions {
                    if let IRInstr::Call { func: callee, .. } = instr {
                        if let Some((name, type_args)) = parse_generic_callee(callee) {
                            let key = MonoKey { func_name: name, type_args };
                            if let Some(spec_name) = self.specializations.get(&key) {
                                *callee = spec_name.clone();
                            }
                        }
                    }
                }
            }
        }

        result
    }
}

/// Parse a generic callee name of the form `name<T1, T2, ...>` into its
/// base name and comma-separated type arguments.
///
/// Returns `None` if the callee does not contain a balanced `<...>` suffix.
/// Safe against malformed names (missing closing `>`, empty angle brackets,
/// trailing garbage after `>`).
fn parse_generic_callee(callee: &str) -> Option<(String, Vec<String>)> {
    let start = callee.find('<')?;
    let end = callee.rfind('>')?;
    if end <= start {
        return None;
    }
    let name = callee[..start].to_string();
    if name.is_empty() {
        return None;
    }
    let type_str = &callee[start + 1..end];
    // Reject trailing content after the closing '>' (e.g. `id<i32>.foo`).
    if !callee[end + 1..].is_empty() {
        return None;
    }
    let type_args: Vec<String> = type_str
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    if type_args.is_empty() {
        return None;
    }
    Some((name, type_args))
}

// ===========================================================================
// Pipeline entry point
// ===========================================================================

/// Pipeline entry point for monomorphization.
///
/// Called from `pipeline.rs` immediately after SCG→IR lowering (and before
/// codegen-side optimization), this pass:
///
/// 1. Scans every function for `IRInstr::Call` instructions whose callee
///    name carries a `<...>` type-argument suffix (e.g. `id<i32>`).
/// 2. For each unique `(generic_name, type_args)` pair, clones the generic
///    function definition once per instantiation and renames the clone
///    using [`Monomorphizer::specialized_name`] (e.g. `id_i32`).
/// 3. Rewrites every generic call site to invoke the specialized clone
///    directly.
/// 4. Drops the original generic definitions (they have no concrete
///    callers after step 3).
///
/// Returns `Ok(())` on success. The `Result` wrapper is reserved for
/// future hard-error cases (e.g. a malformed `<...>` suffix that prevents
/// call-site rewriting); today the lowerer is permissive — missing generic
/// definitions fall back to extern linkage.
pub fn monomorphize(program: &mut IRProgram) -> Result<(), BackendError> {
    let mut mono = Monomorphizer::new();
    let funcs = std::mem::take(&mut program.functions);
    program.functions = mono.monomorphize(funcs);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{IRProgram, IRType, IRTerminator};

    #[test]
    fn test_specialized_name() {
        assert_eq!(
            Monomorphizer::specialized_name("push", &["u32".to_string()]),
            "push_u32"
        );
        assert_eq!(
            Monomorphizer::specialized_name(
                "insert",
                &["String".to_string(), "u32".to_string()]
            ),
            "insert_String_u32"
        );
    }

    #[test]
    fn test_non_generic_name() {
        assert_eq!(Monomorphizer::specialized_name("foo", &[]), "foo");
    }

    #[test]
    fn test_parse_generic_callee() {
        assert_eq!(
            parse_generic_callee("id<i32>"),
            Some(("id".to_string(), vec!["i32".to_string()]))
        );
        assert_eq!(
            parse_generic_callee("map<K, V>"),
            Some(("map".to_string(), vec!["K".to_string(), "V".to_string()]))
        );
        // No angle brackets.
        assert_eq!(parse_generic_callee("foo"), None);
        // Empty type args.
        assert_eq!(parse_generic_callee("id<>"), None);
        // Trailing content after '>'.
        assert_eq!(parse_generic_callee("id<i32>.bar"), None);
        // Reversed / malformed.
        assert_eq!(parse_generic_callee("id>i32<"), None);
    }

    /// Wave 34 inline test: a generic `fn id<T>(x: T) -> T` called with
    /// `id<i32>(5)` and `id<i64>(7)` is monomorphized into two specialized
    /// clones (`id_i32`, `id_i64`), the original `id` is dropped, and the
    /// call sites are rewritten to the specialized names.
    #[test]
    fn test_monomorphize_id_function() {
        // ── generic fn id<T>(x: T) -> T { return x; } ──
        let mut id_fn = IRFunction::new("id");
        id_fn.params.push(IRValue::Register(0));
        id_fn.param_types.push(IRType::I64);
        id_fn.results.push(IRValue::Register(0));
        id_fn.result_types.push(IRType::I64);
        id_fn.blocks[0].terminator = IRTerminator::Return(vec![IRValue::Register(0)]);

        // ── caller: fn caller() -> i64 { let a = id<i32>(5); let b = id<i64>(7); return b; } ──
        let mut caller = IRFunction::new("caller");
        caller.results.push(IRValue::Register(2));
        caller.result_types.push(IRType::I64);
        let blk = caller.current_block();
        blk.push(IRInstr::Call {
            dst: Some(IRValue::Register(1)),
            func: "id<i32>".to_string(),
            args: vec![IRValue::Immediate(5)],
            is_extern: false,
        });
        blk.push(IRInstr::Call {
            dst: Some(IRValue::Register(2)),
            func: "id<i64>".to_string(),
            args: vec![IRValue::Immediate(7)],
            is_extern: false,
        });
        blk.terminator = IRTerminator::Return(vec![IRValue::Register(2)]);

        let mut prog = IRProgram::new();
        prog.functions.push(id_fn);
        prog.functions.push(caller);

        monomorphize(&mut prog).expect("monomorphize should succeed");

        // After monomorphization:
        //  - the original generic `id` is dropped,
        //  - two specialized clones (`id_i32`, `id_i64`) are added,
        //  - the caller survives and its call sites reference the
        //    specialized names.
        let names: Vec<&str> =
            prog.functions.iter().map(|f| f.name.as_str()).collect();
        assert!(
            names.contains(&"id_i32"),
            "missing `id_i32` specialization (got {:?})",
            names
        );
        assert!(
            names.contains(&"id_i64"),
            "missing `id_i64` specialization (got {:?})",
            names
        );
        assert!(
            !names.contains(&"id"),
            "generic `id` should have been dropped (got {:?})",
            names
        );
        assert!(names.contains(&"caller"));

        // The two specializations are distinct clones (not the same clone
        // reused for both instantiations).
        let id_i32 = prog
            .functions
            .iter()
            .find(|f| f.name == "id_i32")
            .expect("id_i32 should exist");
        let id_i64 = prog
            .functions
            .iter()
            .find(|f| f.name == "id_i64")
            .expect("id_i64 should exist");
        assert_ne!(id_i32.name, id_i64.name);

        // Both specialized clones inherit the body of the generic `id`
        // (a single-block function that returns its parameter).
        assert_eq!(id_i32.blocks.len(), 1);
        assert!(matches!(
            &id_i32.blocks[0].terminator,
            IRTerminator::Return(_)
        ));
        assert_eq!(id_i64.blocks.len(), 1);
        assert!(matches!(
            &id_i64.blocks[0].terminator,
            IRTerminator::Return(_)
        ));

        // The caller's call sites are rewritten to use the specialized
        // names — no `<...>` suffix survives.
        let caller = prog
            .functions
            .iter()
            .find(|f| f.name == "caller")
            .expect("caller should exist");
        let call_names: Vec<String> = caller.blocks[0]
            .instructions
            .iter()
            .filter_map(|i| {
                if let IRInstr::Call { func, .. } = i {
                    Some(func.clone())
                } else {
                    None
                }
            })
            .collect();
        assert!(
            call_names.contains(&"id_i32".to_string()),
            "caller should call id_i32 (got {:?})",
            call_names
        );
        assert!(
            call_names.contains(&"id_i64".to_string()),
            "caller should call id_i64 (got {:?})",
            call_names
        );
        assert!(
            !call_names.iter().any(|n| n.contains('<')),
            "no call site should retain a `<...>` suffix (got {:?})",
            call_names
        );
    }

    /// Wave 34 inline test: a single generic function called twice with
    /// the *same* type args produces exactly ONE specialization (not two).
    #[test]
    fn test_monomorphize_dedupes_same_instantiation() {
        let mut id_fn = IRFunction::new("id");
        id_fn.params.push(IRValue::Register(0));
        id_fn.param_types.push(IRType::I64);
        id_fn.results.push(IRValue::Register(0));
        id_fn.result_types.push(IRType::I64);
        id_fn.blocks[0].terminator = IRTerminator::Return(vec![IRValue::Register(0)]);

        let mut caller = IRFunction::new("caller");
        caller.results.push(IRValue::Register(2));
        caller.result_types.push(IRType::I64);
        let blk = caller.current_block();
        blk.push(IRInstr::Call {
            dst: Some(IRValue::Register(1)),
            func: "id<i32>".to_string(),
            args: vec![IRValue::Immediate(5)],
            is_extern: false,
        });
        blk.push(IRInstr::Call {
            dst: Some(IRValue::Register(2)),
            func: "id<i32>".to_string(),
            args: vec![IRValue::Immediate(9)],
            is_extern: false,
        });
        blk.terminator = IRTerminator::Return(vec![IRValue::Register(2)]);

        let mut prog = IRProgram::new();
        prog.functions.push(id_fn);
        prog.functions.push(caller);

        monomorphize(&mut prog).expect("monomorphize should succeed");

        let count = prog
            .functions
            .iter()
            .filter(|f| f.name == "id_i32")
            .count();
        assert_eq!(
            count, 1,
            "expected exactly one `id_i32` specialization, got {}",
            count
        );
    }
}
