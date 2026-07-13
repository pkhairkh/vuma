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
//!
//! # Wave 32: Interprocedural Effect Propagation
//!
//! [`analyze_program_effects`] now performs fixpoint propagation across
//! call edges: if `f` calls `g`, then `f` inherits all of `g`'s
//! effects.  This means a function that only calls other pure
//! functions is itself pure (`fn f(){ g(); }` where `g` is Pure → `f`
//! is Pure), and a function that calls a memory-writing function is
//! itself impure.  Truly-extern calls (to functions not in the
//! program) still get the conservative `ExternCall` marker.

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
///
/// This is the **intra-function** analysis: every `Call` to a function
/// not in the well-known runtime set (`write`, `__vuma_alloc`, etc.)
/// is conservatively marked `ExternCall`.  Use
/// [`analyze_program_effects`] to resolve effects of calls to other
/// functions defined in the same program (interprocedural
/// propagation).
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

/// Intra-function effect inference that does NOT mark calls to known
/// local functions as `ExternCall` — those effects are resolved by
/// [`analyze_program_effects`]'s interprocedural fixpoint.
fn infer_effects_with_local_set(
    func: &IRFunction,
    local_funcs: &HashSet<&str>,
) -> EffectSet {
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
                IRInstr::Call { func: fname, .. } => match fname.as_str() {
                    "write" | "read" | "open" | "close" | "exit" => {
                        effects.add(Effect::IO);
                    }
                    "__vuma_alloc" | "allocate" => {
                        effects.add(Effect::Alloc);
                    }
                    "__vuma_free" | "free" => {
                        effects.add(Effect::Free);
                    }
                    other if local_funcs.contains(other) => {
                        // Local function — defer to interprocedural
                        // propagation.  Do NOT add `ExternCall`.
                    }
                    _ => {
                        effects.add(Effect::ExternCall);
                    }
                },
                _ => {}
            }
        }
        // A tail-call to a local function is also a call edge.
        if let IRTerminator::TailCall { func: fname, .. } = &block.terminator {
            if !matches!(
                fname.as_str(),
                "write" | "read" | "open" | "close" | "exit"
                    | "__vuma_alloc" | "allocate"
                    | "__vuma_free" | "free"
            ) && !local_funcs.contains(fname.as_str())
            {
                effects.add(Effect::ExternCall);
            }
        }
    }

    effects
}

/// Collect the set of local (defined-in-this-program) function names
/// called by `func`, via both `IRInstr::Call` and `IRTerminator::TailCall`.
fn local_callees<'a>(func: &'a IRFunction, local_funcs: &HashSet<&str>) -> Vec<&'a str> {
    let mut out = Vec::new();
    for block in &func.blocks {
        for instr in &block.instructions {
            if let IRInstr::Call { func: fname, .. } = instr {
                if local_funcs.contains(fname.as_str()) {
                    out.push(fname.as_str());
                }
            }
        }
        if let IRTerminator::TailCall { func: fname, .. } = &block.terminator {
            if local_funcs.contains(fname.as_str()) {
                out.push(fname.as_str());
            }
        }
    }
    out
}

/// Build a map of all functions' effects, with interprocedural
/// propagation across call edges.
///
/// Algorithm:
/// 1. Compute initial intra-function effect sets, treating calls to
///    other functions *in the same program* as deferred (no
///    `ExternCall` marker added for them).
/// 2. Iterate to fixpoint: for each function `f`, for each local
///    callee `g`, union `g`'s effect set into `f`'s.
///
/// Convergence: the effect set of every function grows monotonically,
/// and there are only finitely many effects (6), so the fixpoint is
/// reached in at most O(|functions| * |effects|) iterations.
pub fn analyze_program_effects(funcs: &[IRFunction]) -> HashMap<String, EffectSet> {
    let local_funcs: HashSet<&str> = funcs.iter().map(|f| f.name.as_str()).collect();

    // Step 1: initial intra-function effects (local callees deferred).
    let mut map: HashMap<String, EffectSet> = HashMap::new();
    let mut callee_map: HashMap<String, Vec<&str>> = HashMap::new();
    for func in funcs {
        map.insert(func.name.clone(), infer_effects_with_local_set(func, &local_funcs));
        callee_map.insert(func.name.clone(), local_callees(func, &local_funcs));
    }

    // Step 2: fixpoint propagation across call edges.
    let mut changed = true;
    while changed {
        changed = false;
        // Snapshot the callee effect sets so we can mutate callers in-place.
        let snapshot: HashMap<String, EffectSet> = map
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        for func in funcs {
            let callees = &callee_map[&func.name];
            if callees.is_empty() {
                continue;
            }
            let caller = map.get_mut(&func.name).expect("caller must exist");
            for callee in callees {
                if let Some(callee_effects) = snapshot.get(*callee) {
                    let before = caller.effects.len();
                    caller.union(callee_effects);
                    if caller.effects.len() != before {
                        changed = true;
                    }
                }
            }
        }
    }

    map
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{IRType, IRValue};

    /// Helper: make an `IRValue::Register(n)`.
    fn r(n: u32) -> IRValue {
        IRValue::Register(n)
    }

    /// Helper: build a function with one entry block.
    fn fn_with(name: &str, instrs: Vec<IRInstr>, term: IRTerminator) -> IRFunction {
        let mut f = IRFunction::new(name.to_string());
        f.blocks[0].instructions = instrs;
        f.blocks[0].terminator = term;
        f
    }

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

    // ── Interprocedural effect propagation tests ──────────────────────

    /// `fn f() { g(); }` where `g` is Pure → `f` is Pure.
    #[test]
    fn test_interprocedural_pure_callee_makes_caller_pure() {
        // g: pure (empty body, just ret)
        let g = fn_with("g", vec![], IRTerminator::Return(vec![]));
        // f: calls g, then ret
        let f = fn_with(
            "f",
            vec![IRInstr::Call {
                dst: None,
                func: "g".to_string(),
                args: vec![],
                is_extern: false,
            }],
            IRTerminator::Return(vec![]),
        );

        let map = analyze_program_effects(&[g, f]);
        assert!(
            map["g"].is_pure(),
            "g should be Pure (empty body), got: {}",
            map["g"]
        );
        assert!(
            map["f"].is_pure(),
            "f should be Pure (only calls pure g), got: {}",
            map["f"]
        );
    }

    /// `fn f() { g(); }` where `g` writes memory → `f` is Impure
    /// (has Modifies).
    #[test]
    fn test_interprocedural_impure_callee_propagates_modifies() {
        // g: store 42 -> (%v1, 0)   (writes memory → Modifies)
        let g = fn_with(
            "g",
            vec![
                IRInstr::Alloc {
                    dst: r(1),
                    size: 8,
                },
                IRInstr::Store {
                    value: IRValue::Immediate(42),
                    addr: r(1),
                    offset: 0,
                    ty: IRType::I64,
                },
            ],
            IRTerminator::Return(vec![]),
        );
        // f: calls g, then ret
        let f = fn_with(
            "f",
            vec![IRInstr::Call {
                dst: None,
                func: "g".to_string(),
                args: vec![],
                is_extern: false,
            }],
            IRTerminator::Return(vec![]),
        );

        let map = analyze_program_effects(&[g, f]);
        assert!(
            map["g"].contains(Effect::Modifies),
            "g should have Modifies effect, got: {}",
            map["g"]
        );
        assert!(
            map["f"].contains(Effect::Modifies),
            "f should inherit Modifies from g, got: {}",
            map["f"]
        );
        assert!(
            !map["f"].contains(Effect::ExternCall),
            "f should NOT mark local call as ExternCall, got: {}",
            map["f"]
        );
    }

    /// Truly extern (non-local) callees still get `ExternCall`.
    #[test]
    fn test_interprocedural_extern_callee_keeps_extern_marker() {
        // f: calls printf (not in program) → ExternCall
        let f = fn_with(
            "f",
            vec![IRInstr::Call {
                dst: None,
                func: "printf".to_string(),
                args: vec![],
                is_extern: true,
            }],
            IRTerminator::Return(vec![]),
        );
        let map = analyze_program_effects(&[f]);
        assert!(
            map["f"].contains(Effect::ExternCall),
            "extern call should keep ExternCall marker, got: {}",
            map["f"]
        );
    }

    /// Transitive propagation: `f → g → h` where `h` writes memory →
    /// both `g` and `f` inherit Modifies.
    #[test]
    fn test_interprocedural_transitive_propagation() {
        // h: writes memory.
        let h = fn_with(
            "h",
            vec![
                IRInstr::Alloc {
                    dst: r(1),
                    size: 8,
                },
                IRInstr::Store {
                    value: IRValue::Immediate(7),
                    addr: r(1),
                    offset: 0,
                    ty: IRType::I64,
                },
            ],
            IRTerminator::Return(vec![]),
        );
        // g: calls h.
        let g = fn_with(
            "g",
            vec![IRInstr::Call {
                dst: None,
                func: "h".to_string(),
                args: vec![],
                is_extern: false,
            }],
            IRTerminator::Return(vec![]),
        );
        // f: calls g.
        let f = fn_with(
            "f",
            vec![IRInstr::Call {
                dst: None,
                func: "g".to_string(),
                args: vec![],
                is_extern: false,
            }],
            IRTerminator::Return(vec![]),
        );

        let map = analyze_program_effects(&[h, g, f]);
        assert!(map["h"].contains(Effect::Modifies));
        assert!(
            map["g"].contains(Effect::Modifies),
            "g should inherit Modifies from h, got: {}",
            map["g"]
        );
        assert!(
            map["f"].contains(Effect::Modifies),
            "f should inherit Modifies transitively from h, got: {}",
            map["f"]
        );
    }

    /// Mutual recursion: `f → g → f`, both pure → both stay Pure.
    #[test]
    fn test_interprocedural_mutual_recursion_pure() {
        // f: if 0 then ret else call g
        // g: if 0 then ret else call f
        // (We use a dummy CondBranch that always falls through to ret
        // so neither function actually loops, but the call edges exist.)
        let f = fn_with(
            "f",
            vec![IRInstr::Call {
                dst: None,
                func: "g".to_string(),
                args: vec![],
                is_extern: false,
            }],
            IRTerminator::Return(vec![]),
        );
        let g = fn_with(
            "g",
            vec![IRInstr::Call {
                dst: None,
                func: "f".to_string(),
                args: vec![],
                is_extern: false,
            }],
            IRTerminator::Return(vec![]),
        );

        let map = analyze_program_effects(&[f, g]);
        assert!(
            map["f"].is_pure(),
            "f in mutual-recursion with pure g should be Pure, got: {}",
            map["f"]
        );
        assert!(
            map["g"].is_pure(),
            "g in mutual-recursion with pure f should be Pure, got: {}",
            map["g"]
        );
    }
}
